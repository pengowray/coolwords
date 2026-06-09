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
from collections import defaultdict

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


LEVELS = (0, 1, 2, 3)


def score_group(cnt, freq_comb, cap, length, syl, rare, ety, formof, proper, offensive, inwikt, alpha):
    """Filter + score one group (the representative word's features, combined freq).
    Returns (score, s_rarity, s_salience, s_origin, s_aesthetic) or None if filtered."""
    if not alpha or not inwikt or formof or proper or offensive:
        return None
    if cap is not None and cap > 0.5:
        return None
    if (length or 0) < MIN_LEN:
        return None
    if freq_comb is not None and freq_comb > MAX_FREQ_PM_LC:
        return None
    s_r = rarity(freq_comb)
    s_s = math.log1p(cnt)
    s_o = 2.0 if (ety and ety not in COMMON_ORIGINS) else 0.0
    s_a = (0.5 * len(rare) if rare else 0.0)
    if length and 6 <= length <= 13:
        s_a += 0.5
    if syl and 3 <= syl <= 5:
        s_a += 0.5
    score = W_RARITY * s_r + W_SALIENCE * s_s + W_ORIGIN * s_o + W_AESTHETIC * s_a
    return score, s_r, s_s, s_o, s_a


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--slug", required=True)
    ap.add_argument("--show", type=int, default=30)
    ap.add_argument("--level", type=int, default=0, help="which level to print in the preview")
    args = ap.parse_args()

    con = connect()
    row = con.execute("SELECT id FROM books WHERE slug = ?", (args.slug,)).fetchone()
    if not row:
        raise SystemExit(f"no book with slug {args.slug!r} — run ingest.book first")
    book_id = row[0]

    # book's in-dictionary alpha words and their counts
    occ = con.execute(
        """SELECT o.word_id, o.count FROM book_occurrences o JOIN words w ON w.id = o.word_id
           WHERE o.book_id = ? AND o.word_id IS NOT NULL AND w.alpha_only = 1""",
        (book_id,),
    ).fetchall()
    count_by = {wid: c for (wid, c) in occ}
    book_wids = list(count_by)
    if not book_wids:
        raise SystemExit(f"{args.slug}: no in-dictionary words")

    con.execute("CREATE TEMP TABLE bw(id INTEGER PRIMARY KEY)")
    con.executemany("INSERT OR IGNORE INTO bw VALUES (?)", [(w,) for w in book_wids])

    # level -> {word_id: lemma_id} for the book's words (level 0 = self, implicit)
    lemma_at = {lvl: {} for lvl in LEVELS}
    for wid, lvl, lid in con.execute(
        "SELECT word_id, level, lemma_id FROM word_lemma WHERE word_id IN (SELECT id FROM bw)"
    ):
        lemma_at[lvl][wid] = lid

    def rep(wid, lvl):
        return wid if lvl == 0 else lemma_at[lvl].get(wid, wid)

    # representative features + combined family frequency, for every rep used
    reps = {rep(w, lvl) for w in book_wids for lvl in LEVELS}
    con.execute("CREATE TEMP TABLE rp(id INTEGER PRIMARY KEY)")
    con.executemany("INSERT OR IGNORE INTO rp VALUES (?)", [(r,) for r in reps])
    feats = {row[0]: row[1:] for row in con.execute(
        """SELECT id, freq_pm_lc, cap_ratio, length, syllables, rare_letters, etymology_lang,
                  is_form_of, is_proper, is_offensive, in_wiktionary, alpha_only
           FROM words WHERE id IN (SELECT id FROM rp)""")}
    fam_freq = {(lvl, lid): fl for (lvl, lid, fl) in con.execute(
        "SELECT level, lemma_id, freq_pm_lc FROM lemma_freq WHERE lemma_id IN (SELECT id FROM rp)")}

    con.execute("DELETE FROM candidates WHERE book_id = ?", (book_id,))
    per_level = {}
    for lvl in LEVELS:
        groups: dict = defaultdict(lambda: [0, 0])   # rep_id -> [combined count, n_forms]
        for wid in book_wids:
            g = groups[rep(wid, lvl)]
            g[0] += count_by[wid]
            g[1] += 1
        cands = []
        for rid, (cnt, nforms) in groups.items():
            f = feats.get(rid)
            if f is None:
                continue
            (own_lc, cap, length, syl, rare, ety, formof, proper, offensive, inwikt, alpha) = f
            comb = own_lc if lvl == 0 else fam_freq.get((lvl, rid), own_lc)
            sc = score_group(cnt, comb, cap, length, syl, rare, ety, formof, proper, offensive, inwikt, alpha)
            if sc is None:
                continue
            cands.append((rid, cnt, nforms, *sc))
        cands.sort(key=lambda c: -c[3])
        con.executemany(
            "INSERT INTO candidates(book_id, level, word_id, in_book, n_forms, score, s_rarity, "
            "s_salience, s_origin, s_aesthetic, cluster, rank) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)",
            [(book_id, lvl, rid, cnt, nf, sc, sr, ss, so, sa, i + 1)
             for i, (rid, cnt, nf, sc, sr, ss, so, sa) in enumerate(cands)],
        )
        per_level[lvl] = len(cands)
    con.commit()
    log_ingest(con, "score", f"{args.slug}: per-level " + str(per_level), sum(per_level.values()))
    print(f"{args.slug}: candidates per level {per_level}\n")

    lvl = args.level
    print(f"== level {lvl} top {args.show} ==")
    print(f"{'rank':>4} {'word':<18} {'forms':>5} {'in_bk':>5} {'score':>6}  {'rar':>4} {'sal':>4} {'org':>3} {'aes':>3}  ety")
    for r, (word, nf, cnt, sc, sr, ss, so, sa, ety) in enumerate(con.execute(
        """SELECT w.word, c.n_forms, c.in_book, c.score, c.s_rarity, c.s_salience, c.s_origin,
                  c.s_aesthetic, w.etymology_lang
           FROM candidates c JOIN words w ON w.id = c.word_id
           WHERE c.book_id = ? AND c.level = ? ORDER BY c.rank LIMIT ?""", (book_id, lvl, args.show)), 1):
        print(f"{r:>4} {word:<18} {nf:>5} {cnt:>5} {sc:>6.2f}  {sr:>4.1f} {ss:>4.1f} {so:>3.0f} {sa:>3.1f}  {ety or ''}")
    con.close()


if __name__ == "__main__":
    main()
