"""Ingest one book: extract the body text, tokenize, and store a word histogram.

    python -m ingest.book data/books/moby_dick_2701.txt \
        --slug gutenberg-2701 --title "Moby-Dick" --author "Herman Melville" --source-id 2701

Tokens are lowercased words (with internal apostrophes/hyphens) and mapped to
dictionary word_ids where possible; unmatched tokens are kept with word_id NULL.

Boilerplate stripping + format handling (txt / epub) live in ingest/extract.py;
this module tokenizes the kept body and writes it. The drag-drop web importer
(ingest/import_book.py) reuses `ingest_tokens` here so there is one ingest path.
"""
import argparse
import re
from collections import Counter

from ingest.db import connect
from ingest.extract import extract

TOKEN_RE = re.compile(r"[a-z]+(?:['’-][a-z]+)*")
_SENT_SPLIT = re.compile(r"(?<=[.!?])\s+")
_MAX_EXAMPLE = 280


def strip_boilerplate(text: str) -> str:
    """Backward-compatible helper: keep only the Gutenberg body of an already-read
    text. New code should use ingest.extract.extract(path) for full segmentation."""
    from ingest.extract import _PG_START, _PG_END
    m = _PG_START.search(text)
    if m:
        text = text[m.end():]
    m = _PG_END.search(text)
    if m:
        text = text[: m.start()]
    return text


def window_around(sentence: str, token: str, max_len: int) -> str:
    """A <= max_len snippet of `sentence` centered on the first whole-word
    (case-insensitive) occurrence of `token`, with ellipses where trimmed, so the
    word is always included even in very long sentences."""
    if len(sentence) <= max_len:
        return sentence
    m = re.search(r"(?<![a-z])" + re.escape(token) + r"(?![a-z])", sentence, re.I)
    if m is None:
        return sentence[:max_len].rstrip() + "…"
    half = max(0, (max_len - (m.end() - m.start())) // 2)
    hi = min(len(sentence), max(m.start() - half, 0) + max_len)
    lo = max(0, hi - max_len)
    snip = sentence[lo:hi].strip()
    if lo > 0:
        snip = "…" + snip
    if hi < len(sentence):
        snip = snip + "…"
    return snip


def tokenize(text: str) -> tuple[Counter, dict[str, str]]:
    """Return (token counts, token -> first in-book sentence, windowed on the token)."""
    flat = re.sub(r"\s+", " ", text).replace("’", "'")
    counts: Counter = Counter()
    examples: dict[str, str] = {}
    for sentence in _SENT_SPLIT.split(flat):
        toks = TOKEN_RE.findall(sentence.lower())
        if not toks:
            continue
        snippet = sentence.strip()
        for t in toks:
            counts[t] += 1
            if t not in examples:
                examples[t] = window_around(snippet, t, _MAX_EXAMPLE)
    return counts, examples


_BOOK_COLS = ("title", "author", "source", "source_id", "year",
              "content_hash", "format", "orig_filename", "text_source")


def ingest_tokens(con, slug: str, meta: dict, tokens: Counter,
                  examples: dict[str, str]) -> tuple[int, int, int]:
    """Upsert the book row (by slug) and replace its word histogram.

    `meta` keys are any of _BOOK_COLS (missing keys are written NULL). Returns
    (book_id, n_tokens, n_types). Shared by this CLI and ingest/import_book.py."""
    n_tokens = sum(tokens.values())
    n_types = len(tokens)

    # map this book's tokens to dictionary word_ids (chunked IN queries)
    idmap: dict[str, int] = {}
    toklist = list(tokens)
    for i in range(0, len(toklist), 900):
        chunk = toklist[i:i + 900]
        ph = ",".join("?" * len(chunk))
        for word, wid in con.execute(f"SELECT word, id FROM words WHERE word IN ({ph})", chunk):
            idmap[word] = wid

    con.execute("INSERT OR IGNORE INTO books(slug) VALUES (?)", (slug,))
    book_id = con.execute("SELECT id FROM books WHERE slug = ?", (slug,)).fetchone()[0]
    set_cols = ", ".join(f"{c}=?" for c in _BOOK_COLS)
    con.execute(
        f"UPDATE books SET {set_cols}, n_tokens=?, n_types=?, ingested_at=datetime('now') WHERE id=?",
        (*[meta.get(c) for c in _BOOK_COLS], n_tokens, n_types, book_id),
    )
    con.execute("DELETE FROM book_occurrences WHERE book_id = ?", (book_id,))
    con.executemany(
        "INSERT INTO book_occurrences(book_id, token, word_id, count, example) VALUES (?, ?, ?, ?, ?)",
        [(book_id, tok, idmap.get(tok), cnt, examples.get(tok)) for tok, cnt in tokens.items()],
    )
    con.commit()
    matched = sum(1 for t in tokens if t in idmap)
    return book_id, n_tokens, n_types, matched


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--slug", required=True)
    ap.add_argument("--title", default="")
    ap.add_argument("--author", default="")
    ap.add_argument("--source", default="gutenberg")
    ap.add_argument("--source-id", default="")
    ap.add_argument("--year", type=int, default=None)
    args = ap.parse_args()

    ex = extract(args.path)
    body = ex.kept_text
    tokens, examples = tokenize(body)
    stripped = sum(len(s.text) for s in ex.segments if not s.kept)
    print(f"{args.slug}: {sum(tokens.values()):,} tokens, {len(tokens):,} distinct types "
          f"(stripped {stripped:,} boilerplate chars across "
          f"{sum(1 for s in ex.segments if not s.kept)} regions)")

    meta = {
        "title": args.title or ex.title,
        "author": args.author or ex.author,
        "source": args.source or ex.source,
        "source_id": args.source_id or ex.source_id,
        "year": args.year if args.year is not None else ex.year,
        "format": ex.fmt,
    }
    con = connect()
    book_id, n_tokens, n_types, matched = ingest_tokens(con, args.slug, meta, tokens, examples)
    print(f"  matched {matched:,}/{n_types:,} types to the dictionary "
          f"({100*matched/max(n_types,1):.1f}%); book_id={book_id}")
    con.close()


if __name__ == "__main__":
    main()
