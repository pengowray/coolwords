"""Ingest the Google Books fiction sub-corpus (2012) as a genre baseline.

Builds fiction_freq and copies fic_count/fic_rank/fic_pm onto matching curated
words. Compared against the general ngram_freq this is the "weirdness ratio": a
word common in fiction but rare in general is genre-typical; rare in both is
genuinely unusual.

The 2012 corpus is POS-tagged (e.g. ABANDON_VERB) with a bare untagged form that
is the verified cross-POS total. TOKEN_RE rejects anything containing '_', so we
keep only the bare totals and avoid double counting.

Note: the general baseline is the 2009 1M corpus and this is the 2012 fiction
corpus — different vintages, so treat the ratio as a heuristic, not exact.
"""
import gzip
from collections import defaultdict

from ingest.db import connect, log_ingest
from ingest.freq import accumulate, ranked_rows, copy_fiction_to_words
from ingest.paths import NGRAM_DIR

FICTION_GLOB = "googlebooks-eng-fiction-all-1gram-20120701-*.gz"
FICTION_TOTALCOUNTS = NGRAM_DIR / "googlebooks-eng-fiction-all-totalcounts-20120701.txt"


def corpus_total() -> int:
    """Sum match counts across the tab-separated 'year,match,pages,volumes' groups."""
    total = 0
    for part in FICTION_TOTALCOUNTS.read_text(encoding="latin-1").split("\t"):
        fields = part.split(",")
        if len(fields) >= 2 and fields[1].isdigit():
            total += int(fields[1])
    return total


def aggregate() -> tuple[dict, dict]:
    counts: dict = defaultdict(int)
    counts_lc: dict = defaultdict(int)
    shards = sorted(NGRAM_DIR.glob(FICTION_GLOB))
    if not shards:
        raise SystemExit(f"no fiction shards matched {FICTION_GLOB} in {NGRAM_DIR}")
    for shard in shards:
        with gzip.open(shard, "rb") as raw:
            accumulate(raw, counts, counts_lc)
        print(f"  {shard.name}: {len(counts):,} distinct word forms so far", flush=True)
    return counts, counts_lc


def main() -> None:
    con = connect()
    total = corpus_total()
    print(f"fiction corpus total 1grams: {total:,}", flush=True)

    counts, counts_lc = aggregate()
    print(f"aggregated {len(counts):,} word forms; writing fiction_freq...", flush=True)

    rows = ranked_rows(counts, counts_lc, total)
    con.execute("DELETE FROM fiction_freq")
    con.executemany(
        "INSERT INTO fiction_freq(token, count, count_lc, cap_ratio, rank, pm) VALUES (?, ?, ?, ?, ?, ?)",
        rows,
    )
    con.commit()
    matched = copy_fiction_to_words(con)
    log_ingest(con, "ngrams-fiction", f"{len(rows)} forms, {matched} matched words", len(rows))
    print(f"fiction_freq: {len(rows):,} forms | matched curated words: {matched:,}", flush=True)
    con.close()


if __name__ == "__main__":
    main()
