"""Per-decade usage trajectory for the usage-over-time chart.

Streams the Google 1M ngram shards once, folds per-year match counts into
per-(token, decade) buckets, normalizes each decade by that decade's summed
corpus total (so growth of the corpus itself doesn't masquerade as word growth),
and writes word_trajectory(word_id, decade, pm).

Gated to the words the UI can chart: every distinct in-book word, plus the
resolved stem/root of each in-book word (so the detail panel's second "root word"
chart has data even when the root itself doesn't occur in the book). Run after
books are ingested; re-runnable.
"""
import zipfile
from collections import defaultdict

from ingest.db import connect, log_ingest
from ingest.paths import NGRAM_DIR, NGRAM_1M_GLOB, NGRAM_1M_TOTALCOUNTS

MIN_DECADE = 1800


def decade_totals() -> dict:
    totals: dict = defaultdict(int)
    for line in NGRAM_1M_TOTALCOUNTS.read_text(encoding="latin-1").splitlines():
        f = line.split("\t")
        if len(f) >= 2 and f[0].isdigit():
            d = int(f[0]) // 10 * 10
            if d >= MIN_DECADE:
                totals[d] += int(f[1])
    return totals


def main() -> None:
    con = connect()
    want: dict = {}  # lowercased ascii word bytes -> word_id
    # every distinct in-book word, plus the resolved stem/root of each in-book
    # word — so anything clickable from the UI, and the "root word" second chart,
    # can get a usage trajectory.
    for wid, word in con.execute(
        """SELECT DISTINCT w.id, w.word FROM words w
           WHERE w.alpha_only = 1 AND (
               w.id IN (SELECT word_id FROM book_occurrences WHERE word_id IS NOT NULL)
               OR w.word IN (
                   SELECT DISTINCT iw.stem FROM book_occurrences bo
                   JOIN words iw ON iw.id = bo.word_id
                   WHERE iw.stem IS NOT NULL
               )
           )"""
    ):
        if word.isascii():
            want[word.encode("ascii")] = wid
    print(f"trajectory: tracking {len(want):,} in-book + root words", flush=True)

    totals = decade_totals()
    counts: dict = defaultdict(lambda: defaultdict(int))  # word_id -> decade -> match
    shards = sorted(NGRAM_DIR.glob(NGRAM_1M_GLOB))
    if not shards:
        raise SystemExit(f"no ngram shards in {NGRAM_DIR}")
    for shard in shards:
        with zipfile.ZipFile(shard) as zf, zf.open(zf.namelist()[0]) as raw:
            for bline in raw:
                f = bline.split(b"\t")
                if len(f) < 3:
                    continue
                wid = want.get(f[0].lower())
                if wid is None:
                    continue
                try:
                    d = int(f[1]) // 10 * 10
                    if d >= MIN_DECADE:
                        counts[wid][d] += int(f[2])
                except ValueError:
                    continue
        print(f"  {shard.name}: {len(counts):,} words seen", flush=True)

    rows = []
    for wid, decs in counts.items():
        for d, c in decs.items():
            t = totals.get(d)
            if t:
                rows.append((wid, d, c / t * 1_000_000))

    con.execute("DELETE FROM word_trajectory")
    con.executemany("INSERT INTO word_trajectory(word_id, decade, pm) VALUES (?, ?, ?)", rows)
    con.commit()
    log_ingest(con, "trajectory", f"{len(counts)} words, {len(rows)} rows", len(rows))
    print(f"trajectory: {len(counts):,} words, {len(rows):,} decade rows", flush=True)
    con.close()


if __name__ == "__main__":
    main()
