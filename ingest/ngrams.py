"""Ingest Google Books 1M ngram frequencies (2009 'top 1 million words' corpus).

Builds the ngram_freq table (surface-form totals, case-insensitive, with a
count_lc/cap_ratio breakdown) then copies aggregates onto matching curated words.
The expensive streaming is independent of the words table; the cheap copy step
lives in ingest/freq.py so it can be re-applied after new headwords are added.
"""
import zipfile
from collections import defaultdict

from ingest.db import connect, log_ingest
from ingest.freq import accumulate, ranked_rows, copy_general_to_words
from ingest.paths import NGRAM_DIR, NGRAM_1M_GLOB, NGRAM_1M_TOTALCOUNTS


def corpus_total() -> int:
    """Sum of all 1gram occurrences across years (denominator for per-million)."""
    total = 0
    for line in NGRAM_1M_TOTALCOUNTS.read_text(encoding="latin-1").splitlines():
        f = line.split("\t")
        if len(f) >= 2 and f[0].isdigit():
            total += int(f[1])
    return total


def aggregate() -> tuple[dict, dict]:
    counts: dict = defaultdict(int)
    counts_lc: dict = defaultdict(int)
    shards = sorted(NGRAM_DIR.glob(NGRAM_1M_GLOB))
    if not shards:
        raise SystemExit(f"no ngram shards matched {NGRAM_1M_GLOB} in {NGRAM_DIR}")
    for shard in shards:
        with zipfile.ZipFile(shard) as zf, zf.open(zf.namelist()[0]) as raw:
            accumulate(raw, counts, counts_lc)
        print(f"  {shard.name}: {len(counts):,} distinct word forms so far", flush=True)
    return counts, counts_lc


def main() -> None:
    con = connect()
    total = corpus_total()
    print(f"corpus total 1grams: {total:,}", flush=True)

    counts, counts_lc = aggregate()
    print(f"aggregated {len(counts):,} word forms; writing ngram_freq...", flush=True)

    rows = ranked_rows(counts, counts_lc, total)
    con.execute("DELETE FROM ngram_freq")
    con.executemany(
        "INSERT INTO ngram_freq(token, count, count_lc, cap_ratio, rank, pm) VALUES (?, ?, ?, ?, ?, ?)",
        rows,
    )
    con.commit()
    matched = copy_general_to_words(con)
    log_ingest(con, "ngrams-1M", f"{len(rows)} forms, {matched} matched words", len(rows))
    print(f"ngram_freq: {len(rows):,} forms | matched curated words: {matched:,}", flush=True)
    con.close()


if __name__ == "__main__":
    main()
