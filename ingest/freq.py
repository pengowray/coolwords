"""Shared helpers for the Google ngram frequency ingests (general + fiction).

The frequency *tables* (ngram_freq / fiction_freq) are keyed by surface form and
do NOT depend on the words table, so they are built once from the corpus. The
cheap "copy aggregates onto matching words" step is factored out here so it can
be re-applied after later ingests (e.g. Wiktextract) add new headwords, without
re-streaming gigabytes.

Case handling (fixes NGRAM-CASEMERGE): tokens are aggregated case-insensitively,
but we separately keep count_lc (the all-lowercase surface form) so cap_ratio =
(count - count_lc)/count exposes how proper-noun-like a token is, and freq_pm_lc
gives a cleaner common-word frequency than the all-case total.
"""
import re

# Keep alphabetic surface forms (with single internal apostrophes / hyphens);
# drop the corpus's many numeric / symbol / OCR-garbage tokens and (in the 2012
# corpus) the POS-tagged "_NOUN" forms, since '_' is not in the pattern. Matched
# on raw bytes so non-matching lines are rejected without decoding.
TOKEN_RE = re.compile(rb"^[A-Za-z]+(?:['-][A-Za-z]+)*$")


def accumulate(line_iter, counts: dict, counts_lc: dict) -> None:
    """Fold raw 'token\\tyear\\tmatch\\t...' byte lines into counts / counts_lc."""
    for bline in line_iter:
        f = bline.split(b"\t")
        if len(f) < 3 or not TOKEN_RE.match(f[0]):
            continue
        try:
            cnt = int(f[2])
        except ValueError:
            continue
        tok = f[0]
        low = tok.lower().decode("ascii")
        counts[low] += cnt
        if tok == tok.lower():            # surface form had no uppercase letters
            counts_lc[low] += cnt


def ranked_rows(counts: dict, counts_lc: dict, total: int) -> list[tuple]:
    """(token, count, count_lc, cap_ratio, rank, pm) sorted by count desc."""
    scale = 1_000_000 / total
    ranked = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
    rows = []
    for i, (tok, c) in enumerate(ranked):
        lc = counts_lc.get(tok, 0)
        cap = (c - lc) / c if c else None
        rows.append((tok, c, lc, cap, i + 1, c * scale))
    return rows


def copy_general_to_words(con) -> int:
    """Reset then copy ngram_freq aggregates onto matching words. Returns matches."""
    con.execute(
        "UPDATE words SET freq_count=NULL, freq_rank=NULL, freq_pm=NULL, "
        "freq_pm_lc=NULL, cap_ratio=NULL, in_ngram1m=0"
    )
    con.execute(
        """UPDATE words SET
               freq_count = (SELECT count FROM ngram_freq WHERE ngram_freq.token = words.word),
               freq_rank  = (SELECT rank  FROM ngram_freq WHERE ngram_freq.token = words.word),
               freq_pm    = (SELECT pm    FROM ngram_freq WHERE ngram_freq.token = words.word),
               freq_pm_lc = (SELECT pm * count_lc * 1.0 / count FROM ngram_freq WHERE ngram_freq.token = words.word),
               cap_ratio  = (SELECT cap_ratio FROM ngram_freq WHERE ngram_freq.token = words.word),
               in_ngram1m = 1
           WHERE word IN (SELECT token FROM ngram_freq)"""
    )
    con.commit()
    return con.execute("SELECT count(*) FROM words WHERE in_ngram1m = 1").fetchone()[0]


def copy_fiction_to_words(con) -> int:
    """Reset then copy fiction_freq aggregates onto matching words. Returns matches."""
    con.execute("UPDATE words SET fic_count=NULL, fic_rank=NULL, fic_pm=NULL")
    con.execute(
        """UPDATE words SET
               fic_count = (SELECT count FROM fiction_freq WHERE fiction_freq.token = words.word),
               fic_rank  = (SELECT rank  FROM fiction_freq WHERE fiction_freq.token = words.word),
               fic_pm    = (SELECT pm    FROM fiction_freq WHERE fiction_freq.token = words.word)
           WHERE word IN (SELECT token FROM fiction_freq)"""
    )
    con.commit()
    return con.execute("SELECT count(*) FROM words WHERE fic_count IS NOT NULL").fetchone()[0]
