"""Format-pluggable text extraction + boilerplate segmentation for book import.

Given a dropped file (.txt / .epub), produce:
  - detected metadata (title, author, year, source, source_id)
  - an ordered list of `Segment`s labelling which spans are KEPT (real body text,
    tokenised into the word histogram) vs STRIPPED (Gutenberg header/licence, table
    of contents, credits, transcriber notes, EPUB cover/front-matter, ...).

The segmentation is what the import UI's "kept vs stripped" viewer renders, so the
heuristics are intentionally easy to read and tune here. They err toward
UNDER-stripping: an unrecognised region stays in the body (visible, fixable later)
rather than risk eating real text.

PDF and subtitles (.srt/.vtt) are future formats — add a branch in `detect_format`
and `extract` plus an `_extract_*` helper; nothing else changes.
"""
from __future__ import annotations

import re
import zipfile
from dataclasses import dataclass, field
from html.parser import HTMLParser
from pathlib import Path
from xml.etree import ElementTree as ET


@dataclass
class Segment:
    label: str          # 'body' | 'gutenberg-header' | 'gutenberg-licence' | 'toc' | ...
    kept: bool          # True = tokenised into the histogram; False = boilerplate
    text: str           # the raw span (joined kept text forms the book body)
    note: str = ""      # short human reason / title, shown in the viewer


@dataclass
class Extraction:
    fmt: str                        # 'txt' | 'epub'
    segments: list[Segment]
    title: str = ""
    author: str = ""
    year: int | None = None
    year_note: str = ""             # caveat shown by the UI (e.g. PG release ≠ publication)
    source: str = ""                # 'gutenberg' | 'epub'
    source_id: str = ""
    meta: dict = field(default_factory=dict)

    @property
    def kept_text(self) -> str:
        return "".join(s.text for s in self.segments if s.kept)


class ExtractError(Exception):
    """Extraction failed in a way worth surfacing to the user (e.g. DRM)."""

    def __init__(self, message: str, code: str = "EXTRACT_ERROR"):
        super().__init__(message)
        self.code = code


# --------------------------------------------------------------------------- #
#  format detection                                                           #
# --------------------------------------------------------------------------- #
def detect_format(path: Path) -> str:
    path = Path(path)
    ext = path.suffix.lower()
    if ext == ".epub":
        return "epub"
    if ext == ".pdf" or _is_pdf(path):
        return "pdf"
    if ext in (".txt", ".text", ""):
        # sniff a zip in case a .txt is really a renamed epub
        if _is_epub_zip(path):
            return "epub"
        return "txt"
    if _is_epub_zip(path):
        return "epub"
    return "txt"


def _is_pdf(path: Path) -> bool:
    try:
        with open(path, "rb") as f:
            return f.read(5) == b"%PDF-"
    except OSError:
        return False


def _is_epub_zip(path: Path) -> bool:
    try:
        with open(path, "rb") as f:
            if f.read(2) != b"PK":
                return False
        with zipfile.ZipFile(path) as z:
            try:
                return z.read("mimetype").strip() == b"application/epub+zip"
            except KeyError:
                return False
    except (OSError, zipfile.BadZipFile):
        return False


def extract(path: Path, pdf_ocr: dict[int, str] | None = None) -> Extraction:
    """Extract `path`. For PDFs, `pdf_ocr` (0-based page → text) replaces the
    embedded text layer with our own OCR (the re-OCR import path)."""
    path = Path(path)
    fmt = detect_format(path)
    if fmt == "epub":
        return _extract_epub(path)
    if fmt == "pdf":
        return _extract_pdf(path, pdf_ocr)
    return _extract_txt(path)


# --------------------------------------------------------------------------- #
#  plain text / Project Gutenberg                                             #
# --------------------------------------------------------------------------- #
# Accept both the modern "EBOOK" and the pre-2010 "ETEXT" wording — old etexts that
# weren't recognised as PG kept their whole header + legal footer in the histogram
# (that's where the trademark/merchantability/redistribute boilerplate leaked from).
_PG_START = re.compile(r"\*\*\*\s*START OF (?:THE|THIS) PROJECT GUTENBERG E(?:BOOK|TEXT).*?\*\*\*", re.I | re.S)
_PG_END = re.compile(r"\*\*\*\s*END OF (?:THE|THIS) PROJECT GUTENBERG E(?:BOOK|TEXT).*?\*\*\*", re.I | re.S)

_TITLE_LINE = re.compile(r"^Title:[ \t]*(.+)$", re.I | re.M)
_AUTHOR_LINE = re.compile(r"^Author:[ \t]*(.+)$", re.I | re.M)
_RELEASE_LINE = re.compile(r"^Release date:[ \t]*(.+)$", re.I | re.M)
_EBOOK_NO = re.compile(r"\[eBook\s*#(\d+)\]", re.I)
_FIRST_LINE_TITLE = re.compile(r"Project Gutenberg eBook of\s+(.+)", re.I)


def _read_text(path: Path) -> str:
    raw = Path(path).read_bytes()
    if raw[:3] == b"\xef\xbb\xbf":
        raw = raw[3:]
    for enc in ("utf-8", "cp1252", "latin-1"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", errors="replace")


def _extract_txt(path: Path) -> Extraction:
    text = _read_text(path)
    segs: list[Segment] = []

    start = _PG_START.search(text)
    end = _PG_END.search(text)
    is_pg = start is not None

    if start:
        header = text[: start.end()]
        segs.append(Segment("gutenberg-header", False, header,
                            "Project Gutenberg notice + metadata block"))
        body_start = start.end()
    else:
        body_start = 0

    body_end = end.start() if end else len(text)
    body = text[body_start:body_end]
    segs.extend(_subsegment_body(body))

    if end:
        segs.append(Segment("gutenberg-licence", False, text[end.start():],
                            "Project Gutenberg licence / legal footer"))

    ex = Extraction("txt", [s for s in segs if s.text.strip()])

    # --- metadata (Gutenberg header lines) ---
    head = text[: start.start()] if start else text[:4000]
    if m := _TITLE_LINE.search(head):
        ex.title = _clean_title(m.group(1))
    elif m := _FIRST_LINE_TITLE.search(head):
        ex.title = _clean_title(m.group(1))
    if not ex.title:
        ex.title = Path(path).stem.replace("_", " ").strip()
    if m := _AUTHOR_LINE.search(head):
        ex.author = m.group(1).strip()
    if is_pg:
        ex.source = "gutenberg"
        if m := _EBOOK_NO.search(head):
            ex.source_id = m.group(1)
        if m := _RELEASE_LINE.search(head):
            if y := re.search(r"\b(1[5-9]\d\d|20\d\d)\b", m.group(1)):
                ex.year = int(y.group(1))
                ex.year_note = ("Project Gutenberg release (digitisation) date — "
                                "not the work's publication year; please correct.")
    return ex


def _clean_title(s: str) -> str:
    s = _fix_mojibake(s).strip().strip('"').strip()
    # collapse "Title; Or, Subtitle" newlines/whitespace the header sometimes wraps
    return re.sub(r"\s+", " ", s)


# A body may begin with a "Produced by …" credit and/or a table of contents, and
# end with a "Transcriber's Note". These strippers are conservative: each only
# fires on a clearly-marked region and otherwise leaves the text in the body.
_CREDITS = re.compile(r"\A\s*((?:Produced by|E-?text prepared by|Prepared by)\b.*?)(\n[ \t]*\n)",
                      re.I | re.S)
_TOC_HEAD = re.compile(r"(?m)^[ \t]*(?:CONTENTS|TABLE OF CONTENTS)[ \t]*\r?$", re.I)
_TOC_ENTRY = re.compile(
    r"^\s*(chapter\b|letter\b|part\b|book\b|section\b|canto\b|act\b|scene\b|volume\b"
    r"|appendix\b|preface\b|introduction\b|prologue\b|epilogue\b|conclusion\b"
    r"|[ivxlcdm]+[.)]\s|\d+[.)]\s)", re.I)
_TOC_WINDOW = 30000   # only look for a CONTENTS heading in the first ~30k chars of the body
_TRANSCRIBER = re.compile(r"\n[ \t]*\r?\n[ \t]*(?:\*\s*)?Transcriber'?s?\s+Notes?\b", re.I)

# The PG legal FOOTER, as a SAFETY NET for when the `*** END OF … ***` marker is
# absent or malformed (older etexts, hand-edited files): otherwise the whole licence
# leaks into the body. These are TAIL signatures — a genuine end-of-book licence runs
# to the end of the file, so we cut from the match to the end. Each phrase appears
# ONLY in the legal block, never in real prose. (A head-of-file preamble like the
# "Small Print!" block is NOT here — see _SMALL_PRINT — and the cut is guarded below
# so a match with no real body before it can never zero out the histogram.)
_PG_LICENSE_START = re.compile(
    r"\*\*\*\s*START:?\s*FULL\s+LICEN[SC]E"                     # modern licence-block marker
    r"|THE\s+FULL\s+PROJECT\s+GUTENBERG\s+LICEN[SC]E"           # licence heading
    # first licence section — require the PG-specific tail so a law book's own
    # "Section 1. General Terms of Use" heading can't trip it.
    r"|Section\s+1\.\s+General\s+Terms\s+of\s+Use\s+and\s+Redistributing\s+Project\s+Gutenberg"
    # old end line (no ***). Accept a straight OR typographic apostrophe in "Gutenberg's".
    r"|\bEnd\s+of\s+(?:the\s+)?Project\s+Gutenberg(['’]?s\b|\s+E(?:Book|text)\b)",
    re.I)

# The pre-2005 "Small Print!" legal block is a HEAD-of-file preamble, and BOUNDED: a
# start line naming "SMALL PRINT" through the matching "END … SMALL PRINT" line. It's
# stripped IN PLACE (not cut-to-end) so it can never swallow the book body after it.
_SMALL_PRINT = re.compile(
    r"\*[^\n]*\bSMALL\s+PRINT\b.*?\bEND\b[^\n]*\bSMALL\s+PRINT\b[^\n]*",
    re.I | re.S)


def _subsegment_body(body: str) -> list[Segment]:
    """Split a Gutenberg body into kept prose + stripped credits/TOC/transcriber spans.
    Concatenating the returned segment texts reproduces `body` exactly."""
    out: list[Segment] = []
    rest = body

    if m := _CREDITS.match(rest):
        out.append(Segment("credits", False, rest[: m.end()], "transcription credit"))
        rest = rest[m.end():]

    # A "Small Print!" legal block (very old etexts) sits at the head, before the text.
    # Strip it as a BOUNDED region so the book body that follows is never eaten. Any
    # real title/text before it is kept (under-strip).
    if sp := _SMALL_PRINT.search(rest):
        # keep whatever precedes exactly (empty prefixes are dropped by Extraction);
        # this preserves the "segments concatenate back to `body`" contract.
        out.append(Segment("body", True, rest[: sp.start()]))
        out.append(Segment("gutenberg-licence", False, rest[sp.start(): sp.end()],
                           "Project Gutenberg 'Small Print!' legal block"))
        rest = rest[sp.end():]

    toc = _find_toc(rest)
    if toc:
        ts, te = toc
        if rest[:ts].strip():               # title block before CONTENTS — keep (under-strip)
            out.append(Segment("body", True, rest[:ts]))
        out.append(Segment("toc", False, rest[ts:te], "table of contents"))
        rest = rest[te:]

    # Tail boilerplate: a transcriber's note and/or the PG legal footer. Cut from the
    # EARLIEST such marker to the end. The licence net matters most when the `*** END
    # OF … ***` marker never fired (so `body` still holds the whole legal block).
    tn = _TRANSCRIBER.search(rest)
    lic = _PG_LICENSE_START.search(rest)
    # GUARD: only treat the licence as a tail footer when real body precedes it, so a
    # licence-like phrase near the start can never cut the whole book to nothing
    # (the module errs toward under-stripping, never eating real text).
    if lic and not rest[: lic.start()].strip():
        lic = None
    if tn and lic:
        first, is_lic = (lic, True) if lic.start() < tn.start() else (tn, False)
    elif lic:
        first, is_lic = lic, True
    elif tn:
        first, is_lic = tn, False
    else:
        first, is_lic = None, False
    if first:
        out.append(Segment("body", True, rest[: first.start()]))
        if is_lic:
            out.append(Segment("gutenberg-licence", False, rest[first.start():],
                               "Project Gutenberg licence / legal footer"))
        else:
            out.append(Segment("transcriber-note", False, rest[first.start():], "transcriber's note"))
    else:
        out.append(Segment("body", True, rest))
    return out


def _find_toc(rest: str) -> tuple[int, int] | None:
    """Locate a table of contents: a CONTENTS heading (within the first _TOC_WINDOW
    chars) followed by >=2 chapter-list entries. Returns (start, end) char offsets,
    or None. The entry scan stops at the first non-blank line that doesn't look like
    a list entry, so lone section headings and the real text after are NOT swallowed."""
    hm = _TOC_HEAD.search(rest, 0, _TOC_WINDOW)
    if not hm:
        return None
    nl = rest.find("\n", hm.end())
    pos = hm.end() if nl == -1 else nl + 1
    entries = 0
    lead = 4               # tolerate a few non-numbered front entries (Moby's ETYMOLOGY/EXTRACTS)
    prev_blank = True       # entries are blank-line separated; track the run structure
    while pos < len(rest):
        nl = rest.find("\n", pos)
        if nl == -1:
            nl = len(rest)
        s = rest[pos:nl].strip()
        if not s:
            pos = nl + 1
            prev_blank = True
            continue
        looks_entry = _TOC_ENTRY.match(s) or (len(s) <= 70 and re.search(r"\s\d+\s*$", s))
        if looks_entry:
            entries += 1
        elif not prev_blank and entries > 0:
            pass            # wrapped continuation of the previous entry (no blank between)
        elif entries == 0 and lead > 0 and len(s) <= 70:
            lead -= 1       # short non-numbered entry before the numbered run begins
        else:
            break           # a fresh (blank-separated) non-entry line ends the TOC
        pos = nl + 1
        prev_blank = False
        if entries > 600:
            break
    return (hm.start(), pos) if entries >= 2 else None


# --------------------------------------------------------------------------- #
#  EPUB (stdlib: zipfile + ElementTree + HTMLParser)                          #
#  Ports the robust handling from the word-count-epub JS reader.              #
# --------------------------------------------------------------------------- #
_DC = "http://purl.org/dc/elements/1.1/"
_CONTENT_EXT = re.compile(r'CipherReference\s+URI="[^"]*\.(?:x?html?|opf|ncx)"', re.I)
# Spine items that are front/back matter rather than reading content. WORD-BOUNDED
# so substrings don't misfire — e.g. "cover" must NOT match "Discoveries"/"Discovering".
_EPUB_SKIP = re.compile(
    r"\b(cover|copyright|uncopyright|colophon|imprint|dedication|contents|toc|nav"
    r"|acknowledg\w*|frontmatter|backmatter|halftitle|titlepage|advert\w*)\b"
    r"|title[\s_-]?page|half[\s_-]?title|front[\s_-]?matter|back[\s_-]?matter"
    r"|table[\s_-]?of[\s_-]?contents", re.I)
_MIN_EPUB_WORDS = 20

# Ceilings on what one EPUB is allowed to expand to in memory. A zip's central
# directory declares each entry's uncompressed size, so a high-ratio entry (a zip
# bomb, or just a corrupt file) can be refused BEFORE z.read() allocates it. Real
# books are far under these: a fat illustrated epub's biggest XHTML file is a few
# MB. Matters because catalogue downloads are unauthenticated (see paths.py) and
# get imported unattended.
_MAX_ZIP_ENTRY = 32 * 1024 * 1024
_MAX_ZIP_TOTAL = 256 * 1024 * 1024

# cp1252 "smart" punctuation that arrives as C1 control codepoints when an OPF /
# metadata blob is mislabelled latin-1; map the common ones back for clean titles.
_C1_FIX = {0x91: "‘", 0x92: "’", 0x93: "“", 0x94: "”",
           0x95: "•", 0x96: "–", 0x97: "—", 0x85: "…",
           0x84: "„", 0x82: "‚"}


def _fix_mojibake(s: str) -> str:
    return s.translate(_C1_FIX) if s else s


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _zread(z: zipfile.ZipFile, name: str, budget: list[int]) -> bytes:
    """z.read(name), refusing an entry that expands past the ceilings above.

    `budget` is a one-element list holding the bytes still allowed for this file, so
    the running total survives across the spine loop."""
    size = z.getinfo(name).file_size
    if size > _MAX_ZIP_ENTRY or size > budget[0]:
        raise ExtractError("This EPUB expands to an implausible size — refusing to "
                           "read it (it is corrupt, or a zip bomb).",
                           code="EPUB_TOO_LARGE")
    budget[0] -= size
    return z.read(name)


def _extract_epub(path: Path) -> Extraction:
    with zipfile.ZipFile(path) as z:
        names = set(z.namelist())
        budget = [_MAX_ZIP_TOTAL]

        # DRM: only flag when actual CONTENT is encrypted (font obfuscation is fine).
        if "META-INF/encryption.xml" in names:
            enc = _zread(z, "META-INF/encryption.xml", budget).decode("utf-8", "replace")
            if _CONTENT_EXT.search(enc):
                flavour = "Adobe Adept DRM" if "ns.adobe.com/adept" in enc else "DRM"
                raise ExtractError(
                    f"This EPUB is protected by {flavour}: its text is encrypted, so it "
                    f"can't be imported. Remove the DRM from your purchased copy first.",
                    code="EPUB_DRM")

        opf_path = _opf_path(z, budget)
        opf_root = ET.fromstring(_zread(z, opf_path, budget))
        opf_dir = opf_path.rsplit("/", 1)[0] if "/" in opf_path else ""

        title, author, year, source, source_id = _epub_metadata(opf_root)
        manifest, spine, toc_id = _parse_opf(opf_root)
        toc_titles, toc_paths = _parse_epub_toc(z, manifest, toc_id, opf_dir, budget)

        segs: list[Segment] = []
        for idref in spine:
            item = manifest.get(idref)
            if not item:
                continue
            href, _media, props = item
            item_path = _resolve(opf_dir, href.split("#")[0])
            if item_path not in names:
                continue
            html = _zread(z, item_path, budget).decode("utf-8", "replace")
            text = _html_text(html)
            norm = _norm_path(item_path)
            title_for = toc_titles.get(norm) or _title_from_name(href)
            words = len(text.split())

            is_nav = "nav" in (props or "")
            is_front = bool(_EPUB_SKIP.search(href)) or bool(_EPUB_SKIP.search(title_for))
            kept = (words >= _MIN_EPUB_WORDS) and not is_nav and not is_front
            if kept:
                segs.append(Segment("chapter", True, text + "\n\n", title_for))
            else:
                reason = ("navigation/TOC" if is_nav else
                          "front/back matter" if is_front else
                          f"too short ({words} words)")
                segs.append(Segment("front-matter", False, text + "\n\n",
                                    f"{title_for} — {reason}"))

    ex = Extraction("epub", [s for s in segs if s.text.strip()])
    ex.title, ex.author, ex.year = title, author, year
    ex.source, ex.source_id = source or "epub", source_id
    if not ex.title:
        ex.title = Path(path).stem.replace("_", " ").strip()
    return ex


def _opf_path(z: zipfile.ZipFile, budget: list[int]) -> str:
    root = ET.fromstring(_zread(z, "META-INF/container.xml", budget))
    for el in root.iter():
        if _local(el.tag) == "rootfile" and el.get("full-path"):
            return el.get("full-path")
    raise ExtractError("EPUB container.xml has no rootfile", code="EPUB_BADCONTAINER")


def _epub_metadata(opf_root) -> tuple[str, str, int | None, str, str]:
    title = author = ""
    year: int | None = None
    source = source_id = ""
    dates: list[tuple[str, str]] = []  # (event, text)
    for el in opf_root.iter():
        ln = _local(el.tag)
        txt = (el.text or "").strip()
        if ln == "title" and not title:
            title = _clean_title(txt)
        elif ln == "creator" and not author:
            author = _fix_mojibake(txt)
        elif ln == "date" and txt:
            event = el.get(f"{{{_DC}}}event") or el.get("event") or el.get(
                "{http://www.idpf.org/2007/opf}event") or ""
            dates.append((event, txt))
        elif ln in ("identifier", "source") and txt:
            if re.search(r"gutenberg", txt, re.I):
                source = "gutenberg"
                if g := re.search(r"(\d+)", txt):
                    source_id = source_id or g.group(1)
    # prefer a non-"modification" date for the year
    for event, txt in sorted(dates, key=lambda d: d[0] == "modification"):
        if y := re.search(r"\b(1[5-9]\d\d|20\d\d)\b", txt):
            year = int(y.group(1))
            break
    return title, author, year, source, source_id


def _parse_opf(opf_root):
    manifest: dict[str, tuple[str, str, str]] = {}   # id -> (href, media-type, properties)
    spine: list[str] = []
    toc_id = None
    for el in opf_root.iter():
        ln = _local(el.tag)
        if ln == "item":
            iid = el.get("id")
            if iid:
                manifest[iid] = (el.get("href", ""), el.get("media-type", ""),
                                 el.get("properties", "") or "")
                if "nav" in (el.get("properties", "") or ""):
                    toc_id = iid
        elif ln == "spine":
            toc_id = toc_id or el.get("toc")
        elif ln == "itemref":
            if idref := el.get("idref"):
                spine.append(idref)
    return manifest, spine, toc_id


def _parse_epub_toc(z, manifest, toc_id, opf_dir, budget):
    """Return ({normalized content path -> title}, ordered set of TOC paths)."""
    titles: dict[str, str] = {}
    order: list[str] = []
    if not toc_id or toc_id not in manifest:
        return titles, order
    toc_href = manifest[toc_id][0]
    toc_path = _resolve(opf_dir, toc_href)
    toc_dir = toc_path.rsplit("/", 1)[0] if "/" in toc_path else ""
    try:
        data = _zread(z, toc_path, budget)
    except KeyError:
        return titles, order
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        return titles, order

    # EPUB 2 NCX: navPoint > navLabel > text + content@src
    for np in root.iter():
        if _local(np.tag) != "navPoint":
            continue
        label = src = ""
        for child in np.iter():
            lc = _local(child.tag)
            if lc == "text" and not label:
                label = (child.text or "").strip()
            elif lc == "content" and child.get("src"):
                src = child.get("src")
        if src:
            norm = _norm_path(_resolve(toc_dir, src.split("#")[0]))
            titles.setdefault(norm, label)
            order.append(norm)
    if order:
        return titles, order

    # EPUB 3 nav: any <a href>
    for a in root.iter():
        if _local(a.tag) != "a":
            continue
        href = a.get("href") or ""
        if not href:
            continue
        label = "".join(a.itertext()).strip()
        norm = _norm_path(_resolve(toc_dir, href.split("#")[0]))
        titles.setdefault(norm, label)
        order.append(norm)
    return titles, order


def _resolve(base_dir: str, rel: str) -> str:
    stack = [p for p in base_dir.split("/") if p] if base_dir else []
    for part in rel.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if stack:
                stack.pop()
        else:
            stack.append(part)
    return "/".join(stack)


def _norm_path(path: str) -> str:
    from urllib.parse import unquote
    return re.sub(r"/+", "/", unquote(path.replace("\\", "/")))


def _title_from_name(href: str) -> str:
    name = href.rsplit("/", 1)[-1].rsplit(".", 1)[0]
    return re.sub(r"[_-]+", " ", name).strip() or "section"


class _TextHTMLParser(HTMLParser):
    """Collect visible text, skipping head/script/style and inserting spaces at
    block boundaries so words don't run together across tags."""
    _SKIP = {"script", "style", "head", "title"}
    _BLOCK = {"p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6",
              "blockquote", "section", "article", "td", "th"}

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self._skip_depth = 0

    def handle_starttag(self, tag, attrs):
        if tag in self._SKIP:
            self._skip_depth += 1
        elif tag in self._BLOCK:
            self.parts.append(" ")

    def handle_endtag(self, tag):
        if tag in self._SKIP and self._skip_depth:
            self._skip_depth -= 1
        elif tag in self._BLOCK:
            self.parts.append(" ")

    def handle_data(self, data):
        if self._skip_depth == 0:
            self.parts.append(data)


def _html_text(html: str) -> str:
    p = _TextHTMLParser()
    try:
        p.feed(html)
    except Exception:
        # malformed markup: fall back to a crude tag strip
        return re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", html)).strip()
    return re.sub(r"[ \t]*\n[ \t]*", "\n", re.sub(r"[ \t]+", " ", "".join(p.parts))).strip()


# --------------------------------------------------------------------------- #
#  PDF (PyMuPDF; embedded text layer or a caller-supplied OCR override)       #
# --------------------------------------------------------------------------- #
# A page with fewer words than this is treated as image-only/blank (cover, plate,
# unscanned-text page). Used both for segment grouping and the has_text_layer call.
_MIN_PDF_WORDS = 15


def _dehyphenate(text: str) -> str:
    """Re-join words hyphenated across line breaks ("ele-\nphant" -> "elephant")
    and drop soft hyphens. Only fires on lowercase continuation so real hyphenated
    compounds at line end ("man-\nof-war" stays "man-of-war" wrongly? no: 'o' is
    lowercase — accept the rare false join; histograms prefer joined words)."""
    text = text.replace("­", "")
    return re.sub(r"(\w)-[ \t]*\n[ \t]*(\w)", r"\1\2", text)


def pdf_page_texts(path: Path) -> list[str]:
    """De-hyphenated EMBEDDED text of every page (0-based), for the OCR compare."""
    try:
        import pymupdf
    except ImportError:
        raise ExtractError("PDF support needs the PyMuPDF package: pip install pymupdf",
                           code="PDF_SUPPORT_MISSING")
    with pymupdf.open(path) as doc:
        return [_dehyphenate(doc[p].get_text()) for p in range(doc.page_count)]


def _extract_pdf(path: Path, ocr_text: dict[int, str] | None = None) -> Extraction:
    try:
        import pymupdf
    except ImportError:
        raise ExtractError(
            "PDF support needs the PyMuPDF package: pip install pymupdf",
            code="PDF_SUPPORT_MISSING")

    use_ocr = ocr_text is not None
    segs: list[Segment] = []
    n_text = n_image = 0
    title = author = ""
    year: int | None = None

    with pymupdf.open(path) as doc:
        if doc.is_encrypted and not doc.authenticate(""):
            raise ExtractError("This PDF is password-protected and can't be read.",
                               code="PDF_ENCRYPTED")
        meta = doc.metadata or {}
        title = _clean_title(meta.get("title") or "")
        author = (meta.get("author") or "").strip()
        # creationDate like "D:20081231..." — prefer it over modDate
        for key in ("creationDate", "modDate"):
            if y := re.search(r"D:(1[5-9]\d\d|20\d\d)", meta.get(key) or ""):
                year = int(y.group(1))
                break

        n_pages = doc.page_count
        # (page_no, text, has_text) per page, then group contiguous runs.
        per_page: list[tuple[int, str, bool]] = []
        for pno in range(n_pages):
            raw = ocr_text.get(pno, "") if use_ocr else doc[pno].get_text()
            text = _dehyphenate(raw)
            has = len(text.split()) >= _MIN_PDF_WORDS
            n_text += has
            n_image += not has
            per_page.append((pno, text, has))

        run_start, run_has, run_texts = 0, None, []
        def flush(end: int):
            if run_has is None:
                return
            label = f"pages {run_start + 1}–{end}"
            body = "\n\n".join(run_texts) + "\n\n"
            if run_has:
                segs.append(Segment("body", True, body, label + (" (OCR)" if use_ocr else "")))
            else:
                segs.append(Segment("image-pages", False, body,
                                    f"{label} — image-only / blank (no usable text)"))
        for pno, text, has in per_page:
            if has != run_has:
                flush(pno)
                run_start, run_has, run_texts = pno, has, []
            run_texts.append(text)
        flush(n_pages)

    ex = Extraction("pdf", [s for s in segs if s.text.strip()])
    ex.title, ex.author, ex.year = title, author, year
    ex.source = "pdf"
    if not ex.title:
        ex.title = Path(path).stem.replace("_", " ").strip()
    if year is not None:
        ex.year_note = "from PDF metadata (often the scan/creation date) — please check."
    ex.meta["pdf"] = {
        "n_pages": n_pages,
        "n_text_pages": n_text,
        "n_image_pages": n_image,
        # majority of pages carry text ⇒ a usable embedded layer exists
        "has_text_layer": n_text * 2 > n_pages,
        "used_ocr": use_ocr,
    }
    return ex
