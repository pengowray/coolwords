"""Remote book catalogs: sync them locally, search them, download from them.

    python -m ingest.catalog --sync all
    python -m ingest.catalog --search "moby dick" --limit 20
    python -m ingest.catalog --fetch gutenberg 2701 --dest .
    python -m ingest.catalog --grab '[{"source":"gutenberg","source_id":"2701"}]'

Two upstreams, each with its own access rules (both verified; do not "improve"
them into something that hammers a volunteer-run server):

  Project Gutenberg — their robot policy FORBIDS automated crawling of the
    www.gutenberg.org *pages*, but explicitly sanctions (a) the offline catalog
    file and (b) downloading books from a MIRROR. So the sync pulls the one
    gzipped CSV catalog from the main site, and every actual book comes from
    paths.GUTENBERG_MIRROR. Their own wget guidance is `-w 2`, hence a 2s floor.

  Standard Ebooks — the OPDS feed and bulk-download archives are behind Patrons
    Circle membership (/feeds/opds returns 401), so we do NOT touch them. The
    public HTML catalog is browsable and RDFa-annotated, and their robots.txt
    only disallows /ebooks/*/downloads/* for a handful of named SEO crawlers,
    not for User-agent: *. We parse the list view and derive download URLs.

Everything network-facing funnels through _get(), which carries a descriptive
User-Agent and a per-host throttle. A sync is an explicit user action whose
result is cached in SQLite (schema/catalog.sql); searching NEVER hits the wire.

Like ingest/import_book.py, every mode prints exactly ONE JSON object on stdout
(as UTF-8 bytes) and streams human progress lines to stderr for the Rust
background-job progress bar (ui/src/jobs.rs).
"""
import argparse
import csv
import gzip
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

from ingest.db import connect
from ingest.paths import GUTENBERG_MIRROR, STAGING_DIR

SOURCES = ("gutenberg", "standardebooks")

USER_AGENT = "coolwords/0.1 (personal word-research tool; +local)"
TIMEOUT = 60.0

GUTENBERG_CATALOG = "https://www.gutenberg.org/cache/epub/feeds/pg_catalog.csv.gz"
SE_BASE = "https://standardebooks.org"
SE_PER_PAGE = 48
# Only used to render a percentage in the progress line — the walk stops on its
# own (see sync_standardebooks), so an off-by-a-few estimate is harmless.
SE_EST_PAGES = 32
# Hard ceiling on the walk. At 48/page this is ~24k ebooks, ~16x the current
# catalog: a backstop against a pagination change, never a real limit.
SE_MAX_PAGES = 500


# ---------------------------------------------------------------- http ---- #

# host -> minimum seconds between requests. Unknown hosts get the conservative
# 2s floor, because the unknown host is almost always a Gutenberg mirror.
_MIN_INTERVAL = {
    "www.gutenberg.org": 2.0,
    "standardebooks.org": 1.0,
}
_DEFAULT_INTERVAL = 2.0
_last_request: dict[str, float] = {}


def _progress(msg: str) -> None:
    """One progress line to stderr, as UTF-8 BYTES.

    Not plain print(): on Windows sys.stderr encodes to the console's cp1252, so
    a title like "Père Goriot" (or the em-dash in an error) would reach the Rust
    job reader (ui/src/jobs.rs) as invalid UTF-8 and be mangled or dropped. Same
    reason import_book.py writes its JSON via sys.stdout.buffer."""
    sys.stderr.buffer.write(msg.encode("utf-8", "replace") + b"\n")
    sys.stderr.buffer.flush()


def _throttle(host: str) -> None:
    """Sleep out whatever remains of this host's minimum inter-request gap.

    Deliberately global to the process: a bulk --grab of 40 books must not turn
    into 40 simultaneous-ish hits just because they interleave with parsing."""
    interval = _MIN_INTERVAL.get(host, _DEFAULT_INTERVAL)
    prev = _last_request.get(host)
    now = time.monotonic()
    if prev is not None:
        wait = interval - (now - prev)
        if wait > 0:
            time.sleep(wait)
    _last_request[host] = time.monotonic()


def _open(url: str):
    """Throttled urllib GET returning the raw response (for streaming).

    Caller closes it. `Accept-Encoding: identity` keeps transport compression out
    of the way, so a .gz URL is exactly a gzip stream and an HTML page is text."""
    host = urllib.parse.urlsplit(url).netloc
    _throttle(host)
    req = urllib.request.Request(url, headers={
        "User-Agent": USER_AGENT,
        "Accept-Encoding": "identity",
    })
    return urllib.request.urlopen(req, timeout=TIMEOUT)


def _get(url: str, *, binary: bool = False):
    """Throttled GET of a whole resource -> str (utf-8, lenient) or bytes.

    Handles a server that gzips anyway (some mirrors ignore identity) and .gz
    URLs, so callers never think about content encoding."""
    with _open(url) as resp:
        data = resp.read()
        enc = (resp.headers.get("Content-Encoding") or "").lower()
    if enc == "gzip" or (url.endswith(".gz") and data[:2] == b"\x1f\x8b"):
        data = gzip.decompress(data)
    return data if binary else data.decode("utf-8", "replace")


# ------------------------------------------------------ gutenberg sync ---- #

# PG's Authors field is "Surname, Given, 1812-1870 [Role]", '; '-separated. The
# three suffixes come off in this order because a role bracket sits OUTSIDE the
# life dates ("Dickens, Charles, 1812-1870 [Author of introduction, etc.]") and
# would otherwise anchor the date pattern away from the end of the string.
_ROLE = re.compile(r"\s*\[[^\]]*\]\s*$")
_LIFE_DATES = re.compile(
    r",\s*(?:\d{3,4}\??\s*-\s*(?:\d{3,4}\??)?|-\s*\d{3,4}\??)\s*$")
# "Marie, de France, active 12th century" / "Homer, approximately 750 BCE"
_FLOURISHED = re.compile(r",\s*(?:active|fl\.?|approximately|ca\.?)\s+[^,]*$", re.I)


def normalize_author(raw: str) -> str:
    """PG's "Surname, Given, 1812-1870 [Role]" -> "Given Surname"; '; '-joined.

    Life dates and role brackets are dropped: they're bibliographic metadata, not
    part of the name, and they wreck the search box's author matching (searching
    "dickens" shouldn't rank an introduction-writer credit by how its dates sort).
    Entries we can't parse are passed through unchanged rather than mangled —
    better a weird name than no name."""
    out = []
    for part in (raw or "").split(";"):
        name = part.strip()
        name = _ROLE.sub("", name)
        name = _LIFE_DATES.sub("", name)
        name = _FLOURISHED.sub("", name).strip().strip(",").strip()
        if not name:
            continue
        # A single comma means "Surname, Given"; anything else (or none) is
        # already display order or something odd like "Various" / an org name.
        if name.count(",") == 1:
            surname, _, given = name.partition(",")
            given, surname = given.strip(), surname.strip()
            name = f"{given} {surname}".strip() if given else surname
        out.append(name)
    return "; ".join(out)


_UPSERT = """
INSERT INTO catalog_books
    (source, source_id, title, author, year, language, subjects,
     n_words, reading_ease, fmt, url, synced_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
ON CONFLICT(source, source_id) DO UPDATE SET
    title        = excluded.title,
    author       = excluded.author,
    year         = excluded.year,
    language     = excluded.language,
    subjects     = excluded.subjects,
    n_words      = excluded.n_words,
    reading_ease = excluded.reading_ease,
    fmt          = excluded.fmt,
    url          = excluded.url,
    synced_at    = excluded.synced_at
"""


def _record_sync(con, source: str, n_rows: int) -> None:
    con.execute(
        """INSERT INTO catalog_sync(source, synced_at, n_rows) VALUES (?, datetime('now'), ?)
           ON CONFLICT(source) DO UPDATE SET synced_at = excluded.synced_at,
                                             n_rows    = excluded.n_rows""",
        (source, n_rows),
    )
    con.commit()


# Mirrors tried, in order, after the configured one. Mirrors rot — pglaf's TLS
# certificate expired at some point, and aleph (Project Gutenberg's own master
# mirror, the one every other mirror pulls from) serves the cache over plain HTTP
# because its certificate is issued for a different hostname. Plain HTTP is fine
# here: these are public-domain books, there's nothing to keep confidential, and
# the alternative is a dead download. Set COOLWORDS_GUTENBERG_MIRROR to pin one.
_MIRROR_FALLBACKS = (
    "http://aleph.gutenberg.org",
    "https://gutenberg.nabasny.com",
    "https://gutenberg.pglaf.org",
)


# Mirror hosts that failed at the connection level this run — skipped for the rest
# of the process so a bulk grab doesn't re-handshake with a dead server per book.
_dead_hosts: set[str] = set()


def _mirror_bases() -> list[str]:
    """The configured mirror first, then the fallbacks, deduped."""
    out = [GUTENBERG_MIRROR]
    out += [m for m in _MIRROR_FALLBACKS if m not in out]
    return out


def gutenberg_url(pg_id: str, fmt: str = "epub", base: str = "") -> str:
    """The mirror path for a PG id. VERIFIED: <mirror>/cache/epub/2701/pg2701.epub
    (727,431 bytes) and .../pg2701.txt. Never www.gutenberg.org — see the module
    docstring: crawling their pages is against their robot policy."""
    return f"{(base or GUTENBERG_MIRROR).rstrip('/')}/cache/epub/{pg_id}/pg{pg_id}.{fmt}"


def sync_gutenberg(con, limit_rows: int | None = None) -> int:
    """Stream the gzipped PG catalog CSV and upsert the English text entries.

    ~75k rows of the ~230k-row file survive the Type=Text + Language=en filter.
    Streamed and batched inside one transaction so it lands in seconds rather
    than materialising 14 MB of CSV and 75k individual commits."""
    _progress("catalog: gutenberg fetching catalog")
    batch: list[tuple] = []
    n = 0
    with _open(GUTENBERG_CATALOG) as resp:
        # gzip.open over the live socket: the CSV is decompressed as it arrives.
        with gzip.open(resp, "rt", encoding="utf-8", newline="") as fh:
            for row in csv.DictReader(fh):
                if row.get("Type") != "Text" or row.get("Language") != "en":
                    continue
                pg_id = (row.get("Text#") or "").strip()
                if not pg_id.isdigit():
                    continue
                subjects = "; ".join(
                    s for s in ((row.get("Subjects") or "").strip(),
                                (row.get("Bookshelves") or "").strip()) if s
                )
                batch.append((
                    "gutenberg", pg_id,
                    (row.get("Title") or "").replace("\r\n", " ").replace("\n", " ").strip(),
                    normalize_author(row.get("Authors") or ""),
                    None,                      # `Issued` is the PG release date, not a pub year
                    (row.get("Language") or "").strip(),
                    subjects,
                    None, None,                # n_words / reading_ease: PG doesn't report them
                    "epub", gutenberg_url(pg_id),
                ))
                n += 1
                if len(batch) >= 2000:
                    con.executemany(_UPSERT, batch)
                    batch.clear()
                    _progress(f"catalog: gutenberg {n} rows")
                if limit_rows and n >= limit_rows:
                    break
    if batch:
        con.executemany(_UPSERT, batch)
    con.commit()
    _record_sync(con, "gutenberg", n)
    _progress(f"catalog: gutenberg {n} rows (done)")
    return n


# ------------------------------------------------ standard ebooks sync ---- #

# "57,169 words • 70.73 reading ease" from the <div class="details"> blob. Regex
# is fine HERE (a short, known detail string); the surrounding document is parsed
# with a real HTML parser, not by pattern-matching over markup.
_SE_WORDS = re.compile(r"([\d,]+)\s*words", re.I)
_SE_EASE = re.compile(r"([\d.]+)\s*reading\s*ease", re.I)

# Tags that never have an end tag, so they must not be pushed onto the stack.
_VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input",
         "link", "meta", "param", "source", "track", "wbr"}


class _SEListParser(HTMLParser):
    """Tolerant state machine over a Standard Ebooks `?view=list` page.

    The markup is RDFa-annotated and stable:

        <li typeof="schema:Book" about="/ebooks/<author>/<title>">
          <a href="…" property="schema:url"><span property="schema:name">…</span></a>
          <p class="author"><a href="…">James Branch Cabell</a></p>
          <div class="details">…<p>57,169 words • 70.73 reading ease</p></div>
          <ul class="tags"><li><a href="/subjects/comedy">Comedy</a></li>…</ul>
        </li>

    We keep a tag stack and route each text node by looking at the INNERMOST
    enclosing element that we recognise — which is what lets the nested <li>s of
    the tag list coexist with the book <li> that contains them. Any field that
    goes missing simply stays empty; we never drop an otherwise-good entry."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.entries: list[dict] = []
        self._cur: dict | None = None
        self._li_depth = 0          # nested <li> count inside the current entry
        self._stack: list[tuple[str, dict]] = []
        self._tag_buf = ""          # text of the <a> currently inside <ul class="tags">

    # -- helpers ---------------------------------------------------------- #
    @staticmethod
    def _classes(attrs: dict) -> set[str]:
        return set((attrs.get("class") or "").split())

    def _sink(self) -> str | None:
        """Which field the current text node belongs to (innermost match wins)."""
        if self._cur is None:
            return None
        for tag, a in reversed(self._stack):
            if a.get("property") == "schema:name":
                return "title"
            cls = self._classes(a)
            if tag == "ul" and "tags" in cls:
                return "tag"
            if tag == "p" and "author" in cls:
                return "author"
            if "details" in cls:
                return "details"
        return None

    # -- HTMLParser hooks ------------------------------------------------- #
    def handle_starttag(self, tag: str, attrs) -> None:
        a = {k: (v or "") for k, v in attrs}
        if tag == "li":
            if self._cur is None and a.get("typeof") == "schema:Book":
                self._cur = {"about": a.get("about", ""), "title": "",
                             "author": "", "details": "", "tags": []}
                self._li_depth = 0
            if self._cur is not None:
                self._li_depth += 1
        if tag == "a" and self._sink() == "tag":
            self._tag_buf = ""
        if tag not in _VOID:
            self._stack.append((tag, a))

    def handle_endtag(self, tag: str) -> None:
        if tag in _VOID:
            return
        if tag == "a" and self._sink() == "tag":
            t = " ".join(self._tag_buf.split())
            if t:
                self._cur["tags"].append(t)  # type: ignore[union-attr]
            self._tag_buf = ""
        # Pop back to the matching open tag; stray/unbalanced markup just gets
        # unwound rather than corrupting every entry after it.
        for i in range(len(self._stack) - 1, -1, -1):
            if self._stack[i][0] == tag:
                del self._stack[i:]
                break
        if tag == "li" and self._cur is not None:
            self._li_depth -= 1
            if self._li_depth <= 0:
                self.entries.append(self._cur)
                self._cur = None

    def handle_data(self, data: str) -> None:
        sink = self._sink()
        if sink is None or self._cur is None:
            return
        if sink == "tag":
            self._tag_buf += data
        elif sink == "title":
            self._cur["title"] += data
        elif sink == "author":
            self._cur["author"] += data
        else:
            self._cur["details"] += data


def se_download_url(about: str) -> str:
    """'/ebooks/charles-dickens/the-mystery-of-edwin-drood' -> the compatible epub.

    VERIFIED shape: .../downloads/<author-slug>_<title-slug>.epub. (There are also
    _advanced.epub / .azw3 / .kepub.epub; the plain one is what we want.)"""
    slug = about.strip("/")
    if slug.startswith("ebooks/"):
        slug = slug[len("ebooks/"):]
    parts = [p for p in slug.split("/") if p]
    if len(parts) < 2:
        return ""
    fname = "_".join(parts)
    return f"{SE_BASE}/ebooks/{'/'.join(parts)}/downloads/{fname}.epub"


def _se_source_id(about: str) -> str:
    """The canonical '<author-slug>/<title-slug>' id out of an `about` attribute."""
    slug = about.strip("/")
    if slug.startswith("ebooks/"):
        slug = slug[len("ebooks/"):]
    return slug


def sync_standardebooks(con, max_pages: int | None = None) -> int:
    """Walk the public HTML catalog until it stops showing us anything new.

    ~1500 ebooks at 48/page is ~31 pages, i.e. ~31s of throttled requests.

    The terminator is "this page contributed no ids we hadn't already seen", NOT
    "this page was empty" — asking for a page past the end does NOT 404 or return
    an empty list, it CLAMPS and serves the last page again, forever. (Verified:
    pages 31, 32, 40 and 500 all return the same 41 entries.) Getting this wrong
    means an infinite loop pounding a volunteer-run server, so there is also a
    hard page ceiling as a backstop."""
    n = 0
    page = 1
    est = SE_EST_PAGES
    seen: set[str] = set()
    while page <= SE_MAX_PAGES:
        if max_pages and page > max_pages:
            break
        est = max(est, page)
        _progress(f"catalog: standardebooks page {page}/{est}")
        url = f"{SE_BASE}/ebooks?page={page}&per-page={SE_PER_PAGE}&view=list"
        try:
            html = _get(url)
        except urllib.error.HTTPError as e:
            # A 404 past the last page is a normal terminator, not a failure.
            if e.code == 404:
                break
            raise
        p = _SEListParser()
        p.feed(html)
        p.close()
        if not p.entries:
            break
        batch = []
        for e in p.entries:
            source_id = _se_source_id(e["about"])
            if not source_id or source_id in seen:
                continue
            seen.add(source_id)
            details = e["details"]
            mw, me = _SE_WORDS.search(details), _SE_EASE.search(details)
            batch.append((
                "standardebooks", source_id,
                " ".join(e["title"].split()) or None,
                " ".join(e["author"].split()) or None,
                None,                            # the list view carries no pub year
                "en",                            # SE is English-only by charter
                "; ".join(e["tags"]) or None,
                int(mw.group(1).replace(",", "")) if mw else None,
                float(me.group(1)) if me else None,
                "epub", se_download_url(e["about"]),
            ))
        if not batch:
            break               # every id repeated: we're being served the last page
        con.executemany(_UPSERT, batch)
        con.commit()
        n += len(batch)
        page += 1
    _record_sync(con, "standardebooks", n)
    _progress(f"catalog: standardebooks {n} rows (done)")
    return n


def do_sync(which: str, max_pages: int | None, limit_rows: int | None) -> dict:
    con = connect()
    counts: dict[str, int] = {}
    try:
        if which in ("gutenberg", "all"):
            counts["gutenberg"] = sync_gutenberg(con, limit_rows=limit_rows)
        if which in ("standardebooks", "all"):
            counts["standardebooks"] = sync_standardebooks(con, max_pages=max_pages)
    finally:
        con.close()
    return {"ok": True, "counts": counts}


# --------------------------------------------------------------- search ---- #

_COLS = ("source", "source_id", "title", "author", "year", "language",
         "subjects", "n_words", "reading_ease", "fmt", "url", "synced_at")

_SORTS = {
    # "relevance" is handled separately (it needs the query text); the rest are
    # plain column orders with a stable tiebreak so paging can't repeat a row.
    "title": "c.title COLLATE NOCASE ASC",
    "author": "c.author COLLATE NOCASE ASC, c.title COLLATE NOCASE ASC",
    "year": "c.year IS NULL, c.year ASC, c.title COLLATE NOCASE ASC",
    "words": "c.n_words IS NULL, c.n_words DESC, c.title COLLATE NOCASE ASC",
}


def _where(query: str, source: str, subject: str) -> tuple[str, list]:
    clauses, params = [], []
    if source:
        clauses.append("c.source = ?")
        params.append(source)
    if query:
        # Match title OR author, case-insensitively. LIKE is already
        # case-insensitive for ASCII in SQLite, but lower() makes it explicit and
        # keeps the behaviour identical for the ORDER BY below.
        clauses.append("(lower(c.title) LIKE ? OR lower(c.author) LIKE ?)")
        like = f"%{query.lower()}%"
        params += [like, like]
    if subject:
        clauses.append("lower(c.subjects) LIKE ?")
        params.append(f"%{subject.lower()}%")
    return (" WHERE " + " AND ".join(clauses)) if clauses else "", params


def do_search(query: str, source: str, subject: str, sort: str,
              limit: int, offset: int) -> dict:
    query = (query or "").strip()
    con = connect()
    where, params = _where(query, source, subject)

    total = con.execute(f"SELECT count(*) FROM catalog_books c{where}", params).fetchone()[0]

    order_params: list = []
    if sort in _SORTS:
        order = _SORTS[sort]
    elif query:
        # Relevance-ish: exact title, then title prefix, then title substring,
        # then author-only hits — enough to float "Moby Dick" above "…Moby Dick…".
        order = ("CASE WHEN lower(c.title) = ? THEN 0 "
                 "     WHEN lower(c.title) LIKE ? THEN 1 "
                 "     WHEN lower(c.title) LIKE ? THEN 2 "
                 "     ELSE 3 END, c.title COLLATE NOCASE ASC")
        q = query.lower()
        order_params = [q, f"{q}%", f"%{q}%"]
    else:
        order = _SORTS["title"]

    cols = ", ".join(f"c.{c}" for c in _COLS)
    rows = con.execute(
        # The LEFT JOIN is the whole point of keeping the catalog in coolwords.db:
        # one query tells the UI both "here are the matches" and "these are already
        # imported" (so it can grey them out instead of offering a no-op download).
        f"SELECT {cols}, b.slug FROM catalog_books c "
        f"LEFT JOIN books b ON b.source = c.source AND b.source_id = c.source_id"
        f"{where} ORDER BY {order} LIMIT ? OFFSET ?",
        [*params, *order_params, limit, offset],
    ).fetchall()
    con.close()

    items = []
    for r in rows:
        item = {c: r[i] for i, c in enumerate(_COLS)}
        item["imported_slug"] = r[len(_COLS)]
        items.append(item)
    return {"ok": True, "total": total, "items": items}


# --------------------------------------------------------------- fetch ---- #

class CatalogError(Exception):
    """A per-item failure that should be reported, not crash a batch."""

    def __init__(self, msg: str, code: str = "CATALOG"):
        super().__init__(msg)
        self.code = code


def _row(con, source: str, source_id: str) -> dict:
    r = con.execute(
        f"SELECT {', '.join(_COLS)} FROM catalog_books WHERE source = ? AND source_id = ?",
        (source, source_id),
    ).fetchone()
    if not r:
        raise CatalogError(f"{source}:{source_id} is not in the local catalog — "
                           f"run `python -m ingest.catalog --sync {source}` first",
                           code="NOT_IN_CATALOG")
    return {c: r[i] for i, c in enumerate(_COLS)}


def _safe_name(source: str, source_id: str, fmt: str) -> str:
    """A filesystem-safe download name. SE ids contain a '/', which is not a path
    separator here — it's part of the identifier."""
    ident = re.sub(r"[^A-Za-z0-9._-]+", "_", source_id).strip("_") or "book"
    return f"{source}-{ident}.{fmt}"


def _attempts(row: dict) -> list[tuple[str, str]]:
    """Ordered (fmt, url) candidates for one catalog row.

    Gutenberg gets every mirror x {epub, txt}: the cache carries .epub for most
    texts but not all, and mirrors go down individually. Standard Ebooks gets the
    one URL plus `?source=download` — WITHOUT that query the site serves a
    "Your Download Has Started!" HTML interstitial (which meta-refreshes to the
    same URL with the query) instead of the file, and you get a 9 KB web page
    saved as an .epub. The stored `url` column keeps the plain, human-clickable
    form; the query is a fetch-time detail."""
    if row["source"] == "gutenberg":
        out = []
        for base in _mirror_bases():
            out.append(("epub", gutenberg_url(row["source_id"], "epub", base)))
            out.append(("txt", gutenberg_url(row["source_id"], "txt", base)))
        return out
    url = row.get("url") or ""
    if not url:
        return []
    if row["source"] == "standardebooks":
        url += ("&" if "?" in url else "?") + "source=download"
    return [(row.get("fmt") or "epub", url)]


def _looks_wrong(data: bytes, fmt: str) -> str:
    """Guard against a server answering 200 with something that isn't the book.

    Cheap, and it is exactly what catches an interstitial/error page saved under
    a .epub name — a silent corruption that only surfaces much later as a book
    with 400 words in it."""
    if not data:
        return "empty response"
    if fmt == "epub" and data[:2] != b"PK":
        return "not a zip (epub) — probably an error or interstitial page"
    if fmt == "txt" and data.lstrip()[:1] == b"<":
        return "looks like HTML, not plain text"
    return ""


def download(row: dict, dest_dir: Path) -> tuple[Path, str]:
    """Download one catalog row into dest_dir. Returns (path, fmt).

    Walks the candidate URLs until one yields something that actually looks like
    the requested format; a 404, a dead mirror, or a bogus body all just move on
    to the next candidate."""
    dest_dir = Path(dest_dir)
    dest_dir.mkdir(parents=True, exist_ok=True)
    last = "no candidate URLs"
    for fmt, url in _attempts(row):
        host = urllib.parse.urlsplit(url).netloc
        if host in _dead_hosts:
            continue
        try:
            data = _get(url, binary=True)
        except urllib.error.HTTPError as e:
            last = f"{url}: {e}"    # 404 — this file is missing, the mirror is fine
            continue
        except (urllib.error.URLError, OSError) as e:
            # Connection-level (expired cert, DNS, refused): the MIRROR is out, so
            # stop paying for it on every remaining book of a bulk grab. Only for
            # Gutenberg, where alternatives exist — one blip must not blacklist
            # Standard Ebooks, which has no second source.
            if row["source"] == "gutenberg":
                _dead_hosts.add(host)
            last = f"{url}: {e}"
            continue
        bad = _looks_wrong(data, fmt)
        if bad:
            last = f"{url}: {bad}"
            continue
        out = dest_dir / _safe_name(row["source"], row["source_id"], fmt)
        out.write_bytes(data)
        return out, fmt
    raise CatalogError(f"download failed for {row['source']}:{row['source_id']} — {last}",
                       code="DOWNLOAD_FAILED")


def do_fetch(source: str, source_id: str, dest: str) -> dict:
    con = connect()
    try:
        row = _row(con, source, source_id)
    finally:
        con.close()
    path, fmt = download(row, Path(dest) if dest else STAGING_DIR)
    return {"ok": True, "path": str(path), "fmt": fmt, "title": row["title"],
            "author": row["author"], "year": row["year"],
            "source": source, "source_id": source_id}


# ---------------------------------------------------------------- grab ---- #

def _slug_for(con, source: str, source_id: str, title: str) -> str:
    """A stable, readable, unique slug for a catalogued book.

    Gutenberg keeps the historical `gutenberg-<id>` shape (import_book's own
    _suggest_slug produces the same thing, so hand-dropped and catalogue-grabbed
    copies of a book agree). SE gets `se-<author>-<title>` from its id."""
    from ingest.import_book import _slugify
    if source == "gutenberg":
        base = f"gutenberg-{source_id}"
    elif source == "standardebooks":
        base = "se-" + _slugify(source_id.replace("/", "-"))
    else:
        base = _slugify(title or source_id) or "book"
    existing = {r[0] for r in con.execute("SELECT slug FROM books")}
    if base not in existing:
        return base
    for i in range(2, 1000):
        if f"{base}-{i}" not in existing:
            return f"{base}-{i}"
    return base


def do_grab(items: list[dict], dest: str) -> dict:
    """Download + import a batch. One bad item never aborts the run.

    Scoring is deliberately skipped (run_pipeline=False): a bulk grab of 40 books
    should land the text fast, and the Rust side queues `--rescore <slug>` per
    book afterwards so the expensive pipeline runs one book at a time under the
    job semaphore."""
    from ingest.import_book import do_commit

    dest_dir = Path(dest) if dest else STAGING_DIR
    imported: list[dict] = []
    skipped: list[dict] = []
    failed: list[dict] = []
    n = len(items)

    for i, it in enumerate(items, 1):
        source = str(it.get("source") or "").strip()
        source_id = str(it.get("source_id") or "").strip()
        try:
            con = connect()
            try:
                row = _row(con, source, source_id)
                already = con.execute(
                    "SELECT slug FROM books WHERE source = ? AND source_id = ?",
                    (source, source_id),
                ).fetchone()
                slug = None if already else _slug_for(con, source, source_id, row["title"])
            finally:
                con.close()

            _progress(f"grab: {i}/{n} {row['title'] or source_id}")
            if already:
                skipped.append({"source": source, "source_id": source_id,
                                "slug": already[0], "reason": "already imported"})
                _progress(f"grab: skipped {already[0]} (already imported)")
                continue

            path, fmt = download(row, dest_dir)
            res = do_commit(path, slug, row["title"] or "", row["author"] or "",
                            row["year"], path.name, run_pipeline=False,
                            source=source, source_id=source_id)
            if not res.get("ok"):
                # A content-hash collision means we already have this text under
                # another slug — that's a skip, not an error.
                if res.get("code") == "DUPLICATE":
                    skipped.append({"source": source, "source_id": source_id,
                                    "slug": None, "reason": res.get("error", "duplicate")})
                    _progress("grab: skipped (duplicate content)")
                else:
                    failed.append({"source": source, "source_id": source_id,
                                   "error": res.get("error", "import failed")})
                continue
            imported.append({"slug": res["slug"], "book_id": res["book_id"],
                             "title": res["title"], "n_tokens": res["n_tokens"]})
            _progress(f"grab: imported {res['slug']}")
        except Exception as e:  # per-item isolation: log it and keep going
            # Our own errors already read as sentences; anything else gets its
            # class name so an unexpected failure is still diagnosable.
            msg = str(e) if isinstance(e, CatalogError) else f"{type(e).__name__}: {e}"
            failed.append({"source": source, "source_id": source_id, "error": msg})
            _progress(f"grab: failed {source}:{source_id} — {msg}")

    return {"ok": True, "n": n, "imported": imported,
            "skipped": skipped, "failed": failed}


def _parse_items(raw: str) -> list[dict]:
    """Accept a JSON array of {source, source_id}, or of 'source:source_id' strings."""
    data = json.loads(raw)
    if isinstance(data, dict):
        data = data.get("items", [])
    out = []
    for it in data:
        if isinstance(it, str):
            src, _, sid = it.partition(":")
            out.append({"source": src, "source_id": sid})
        elif isinstance(it, dict):
            out.append(it)
    return out


# ----------------------------------------------------------------- cli ---- #

def main() -> None:
    ap = argparse.ArgumentParser(
        description="Sync, search, and download from the remote book catalogs.")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--sync", choices=["gutenberg", "standardebooks", "all"],
                   help="refresh the local catalog mirror from upstream")
    g.add_argument("--search", metavar="QUERY", nargs="?", const="",
                   help="search the local catalog (empty query = browse everything)")
    g.add_argument("--fetch", nargs=2, metavar=("SOURCE", "SOURCE_ID"),
                   help="download one book into --dest (does not import it)")
    g.add_argument("--fetch-id", metavar="SOURCE:SOURCE_ID", dest="fetch_id",
                   help="as --fetch, with the pair colon-joined")
    g.add_argument("--grab", metavar="JSON",
                   help='bulk download+import: \'[{"source":...,"source_id":...}]\'')
    g.add_argument("--grab-file", metavar="PATH", dest="grab_file",
                   help="as --grab, reading the JSON from a file (long lists)")

    ap.add_argument("--source", default="", choices=["", *SOURCES],
                    help="--search: restrict to one source")
    ap.add_argument("--subject", default="", help="--search: substring of the subjects field")
    ap.add_argument("--sort", default="relevance",
                    choices=["relevance", "title", "author", "year", "words"])
    ap.add_argument("--limit", type=int, default=50)
    ap.add_argument("--offset", type=int, default=0)
    ap.add_argument("--dest", default="", help="--fetch/--grab download dir (default: staging)")
    # Debug/smoke-test knobs: cap the work so a sync can be exercised against a
    # throwaway COOLWORDS_DB without pulling the whole upstream catalog.
    ap.add_argument("--max-pages", type=int, default=None, dest="max_pages",
                    help="debug: stop the Standard Ebooks walk after N pages")
    ap.add_argument("--limit-rows", type=int, default=None, dest="limit_rows",
                    help="debug: stop the Gutenberg CSV after N kept rows")
    args = ap.parse_args()

    try:
        if args.sync:
            result = do_sync(args.sync, args.max_pages, args.limit_rows)
        elif args.search is not None:
            result = do_search(args.search, args.source, args.subject,
                               args.sort, args.limit, args.offset)
        elif args.fetch or args.fetch_id:
            if args.fetch_id:
                src, _, sid = args.fetch_id.partition(":")
            else:
                src, sid = args.fetch
            result = do_fetch(src, sid, args.dest)
        else:
            raw = (Path(args.grab_file).read_text(encoding="utf-8")
                   if args.grab_file else args.grab)
            result = do_grab(_parse_items(raw), args.dest)
    except CatalogError as e:
        result = {"ok": False, "code": e.code, "error": str(e)}
    except Exception as e:  # surface any failure as JSON the UI can show
        result = {"ok": False, "code": "ERROR", "error": f"{type(e).__name__}: {e}"}

    # UTF-8 bytes straight to the buffer: titles and author names routinely carry
    # characters the Windows console's cp1252 can't encode, and the Rust caller
    # parses stdout as UTF-8.
    sys.stdout.buffer.write(json.dumps(result, ensure_ascii=False).encode("utf-8"))
    sys.stdout.buffer.write(b"\n")


if __name__ == "__main__":
    main()
