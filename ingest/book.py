"""Ingest one book: strip boilerplate, tokenize, and store a word histogram.

    python -m ingest.book data/books/moby_dick_2701.txt \
        --slug gutenberg-2701 --title "Moby-Dick" --author "Herman Melville" --source-id 2701

Tokens are lowercased words (with internal apostrophes/hyphens) and mapped to
dictionary word_ids where possible; unmatched tokens are kept with word_id NULL.
"""
import argparse
import re
from collections import Counter

from ingest.db import connect

TOKEN_RE = re.compile(r"[a-z]+(?:['’-][a-z]+)*")
_SENT_SPLIT = re.compile(r"(?<=[.!?])\s+")
_MAX_EXAMPLE = 280
# Project Gutenberg start/end markers wrap the actual text.
_START = re.compile(r"\*\*\*\s*START OF (THE|THIS) PROJECT GUTENBERG EBOOK.*?\*\*\*", re.I | re.S)
_END = re.compile(r"\*\*\*\s*END OF (THE|THIS) PROJECT GUTENBERG EBOOK", re.I)


def strip_boilerplate(text: str) -> str:
    m = _START.search(text)
    if m:
        text = text[m.end():]
    m = _END.search(text)
    if m:
        text = text[:m.start()]
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

    with open(args.path, encoding="utf-8") as f:
        raw = f.read()
    body = strip_boilerplate(raw)
    tokens, examples = tokenize(body)
    n_tokens = sum(tokens.values())
    print(f"{args.slug}: {n_tokens:,} tokens, {len(tokens):,} distinct types "
          f"(stripped {len(raw)-len(body):,} boilerplate chars)")

    con = connect()
    # map this book's tokens to dictionary word_ids (chunked IN queries)
    idmap: dict[str, int] = {}
    toklist = list(tokens)
    for i in range(0, len(toklist), 900):
        chunk = toklist[i:i + 900]
        ph = ",".join("?" * len(chunk))
        for word, wid in con.execute(f"SELECT word, id FROM words WHERE word IN ({ph})", chunk):
            idmap[word] = wid

    con.execute("INSERT OR IGNORE INTO books(slug) VALUES (?)", (args.slug,))
    book_id = con.execute("SELECT id FROM books WHERE slug = ?", (args.slug,)).fetchone()[0]
    con.execute(
        "UPDATE books SET title=?, author=?, source=?, source_id=?, year=?, n_tokens=?, n_types=?, "
        "ingested_at=datetime('now') WHERE id=?",
        (args.title, args.author, args.source, args.source_id, args.year, n_tokens, len(tokens), book_id),
    )
    con.execute("DELETE FROM book_occurrences WHERE book_id = ?", (book_id,))
    con.executemany(
        "INSERT INTO book_occurrences(book_id, token, word_id, count, example) VALUES (?, ?, ?, ?, ?)",
        [(book_id, tok, idmap.get(tok), cnt, examples.get(tok)) for tok, cnt in tokens.items()],
    )
    con.commit()
    matched = sum(1 for t in tokens if t in idmap)
    print(f"  matched {matched:,}/{len(tokens):,} types to the dictionary "
          f"({100*matched/len(tokens):.1f}%); book_id={book_id}")
    con.close()


if __name__ == "__main__":
    main()
