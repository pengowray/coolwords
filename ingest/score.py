"""Score a book's words for "interestingness" and store ranked candidates.

    python -m ingest.score --slug gutenberg-2701

Hard filters drop the "less-interesting" categories (non-words, inflections,
proper nouns, offensive, too-common, too-short). The composite score combines
transparent sub-scores so the weights can be tuned against human ratings later:
- rarity   : how rare the word is in general (lowercase-only frequency)
- salience : how much it is used in THIS book (weirdness / x-ray signal)
- origin   : bonus for unusual etymology (languages English rarely borrows from)
- aesthetic: rare letters, pleasing length / syllable count
"""
import argparse
import math

from ingest.db import connect, log_ingest

# Languages English borrows from heavily — NOT an "unusual origin".
COMMON_ORIGINS = {
    "en", "enm", "ang", "gem-pro", "ine-pro",          # native / inherited
    "fr", "fro", "frm", "xno",                          # French
    "la", "la-med", "la-ecc", "la-cla",                 # Latin
    "grc",                                              # Ancient Greek
    "non", "is",                                        # Norse
    "de", "nl", "gml", "goh", "gmh", "osx", "nds",      # Germanic
    "it", "es", "pt", "ca",                             # common Romance
}

W_RARITY, W_SALIENCE, W_ORIGIN, W_AESTHETIC = 1.0, 0.6, 1.5, 0.5
# Words absent from the ngram corpus are rarer than the top ~1M forms (whose
# rarity tops out ~3.4), but many are author coinages / productive derivations,
# so cap NULL just above the in-corpus max rather than at the ceiling.
RARE_NULL_FREQ = 4.0
RARITY_CAP = 5.0
MIN_LEN = 3
MAX_FREQ_PM_LC = 12.0    # words more common than this are too ordinary


def rarity(freq_pm_lc) -> float:
    if freq_pm_lc is None:
        return RARE_NULL_FREQ
    if freq_pm_lc <= 0:
        return RARITY_CAP
    return max(0.0, min(RARITY_CAP, -math.log10(freq_pm_lc)))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--slug", required=True)
    ap.add_argument("--show", type=int, default=40)
    args = ap.parse_args()

    con = connect()
    row = con.execute("SELECT id FROM books WHERE slug = ?", (args.slug,)).fetchone()
    if not row:
        raise SystemExit(f"no book with slug {args.slug!r} — run ingest.book first")
    book_id = row[0]

    rows = con.execute(
        """SELECT o.word_id, o.count, w.freq_pm_lc, w.cap_ratio, w.length, w.syllables,
                  w.rare_letters, w.etymology_lang, w.is_form_of, w.is_proper, w.is_offensive,
                  w.in_wiktionary
           FROM book_occurrences o JOIN words w ON w.id = o.word_id
           WHERE o.book_id = ? AND o.word_id IS NOT NULL AND w.alpha_only = 1""",
        (book_id,),
    ).fetchall()

    cands = []
    for (wid, count, freq_lc, cap, length, syl, rare, ety, formof, proper, offensive, inwikt) in rows:
        if not inwikt or formof or proper or offensive:
            continue
        if cap is not None and cap > 0.5:
            continue
        if (length or 0) < MIN_LEN:
            continue
        if freq_lc is not None and freq_lc > MAX_FREQ_PM_LC:
            continue
        s_r = rarity(freq_lc)
        s_s = math.log1p(count)
        s_o = 2.0 if (ety and ety not in COMMON_ORIGINS) else 0.0
        s_a = (0.5 * len(rare) if rare else 0.0)
        if length and 6 <= length <= 13:
            s_a += 0.5
        if syl and 3 <= syl <= 5:
            s_a += 0.5
        score = W_RARITY * s_r + W_SALIENCE * s_s + W_ORIGIN * s_o + W_AESTHETIC * s_a
        cands.append((wid, count, score, s_r, s_s, s_o, s_a))

    cands.sort(key=lambda c: -c[2])
    con.execute("DELETE FROM candidates WHERE book_id = ?", (book_id,))
    con.executemany(
        "INSERT INTO candidates(book_id, word_id, in_book, score, s_rarity, s_salience, "
        "s_origin, s_aesthetic, cluster, rank) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)",
        [(book_id, wid, cnt, sc, sr, ss, so, sa, i + 1)
         for i, (wid, cnt, sc, sr, ss, so, sa) in enumerate(cands)],
    )
    con.commit()
    log_ingest(con, "score", f"{args.slug}: {len(cands)} candidates", len(cands))
    print(f"{len(cands):,} candidates scored for {args.slug}\n")

    print(f"{'rank':>4} {'word':<18} {'in_bk':>5} {'score':>6}  {'rar':>4} {'sal':>4} {'org':>3} {'aes':>3}  ety")
    for r, (word, cnt, sc, sr, ss, so, sa, ety) in enumerate(con.execute(
        """SELECT w.word, c.in_book, c.score, c.s_rarity, c.s_salience, c.s_origin, c.s_aesthetic,
                  w.etymology_lang
           FROM candidates c JOIN words w ON w.id = c.word_id
           WHERE c.book_id = ? ORDER BY c.rank LIMIT ?""", (book_id, args.show)), 1):
        print(f"{r:>4} {word:<18} {cnt:>5} {sc:>6.2f}  {sr:>4.1f} {ss:>4.1f} {so:>3.0f} {sa:>3.1f}  {ety or ''}")
    con.close()


if __name__ == "__main__":
    main()
