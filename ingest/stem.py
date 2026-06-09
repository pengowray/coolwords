"""Per-level lemma/stem grouping for the rating UI's "stemming level" control.

Resolves each word to a root at four aggressiveness levels and stores the mapping
(word_lemma) plus per-level family-frequency totals (lemma_freq), so the UI can
switch how aggressively surface forms are merged instantly:

  0 none          every surface form separate (implicit; not stored)
  1 inflectional  grammatical forms only: trembling/trembled -> tremble, cats -> cat
  2 derivational  = words.stem: importantly -> important, darkness -> dark
  3 aggressive    level 2 + transparent prefixes (untrembling -> tremble,
                  unfrequented -> frequent) and embedding-guarded negations
                  (illegal -> legal); NEVER agentive -er (hatter/baker stay),
                  NEVER import -> port.

Resolution always lands on a REAL frequency-bearing word (never a Porter stem),
picking the MAX-frequency candidate within one morphological family. words.stem
(the level-2 root) is preserved for backward compatibility.

Run after ngrams/fiction (needs freq_pm) and embeddings (the level-3 guard reads
data/coolwords_emb.npy). Idempotent.
"""
from collections import defaultdict

import numpy as np

from ingest.db import connect, log_ingest
from ingest.paths import DB_PATH

EMB_PATH = DB_PATH.parent / "coolwords_emb.npy"

# (suffix, [replacement bases...]) in priority order, most specific first.
RULES = [
    ("ically", ["ical", "ic"]), ("ally", ["al", "ical", "ic"]), ("fully", ["ful"]),
    ("ously", ["ous"]), ("iness", ["y"]), ("ily", ["y", "e"]), ("bly", ["ble"]),
    ("ply", ["ple"]), ("lly", ["ll", "l"]), ("ly", ["", "le", "e"]),
    ("ation", ["ate", "e"]), ("ition", ["ite"]), ("ity", ["e", "ous", ""]),
    ("ness", ["", "e"]), ("iest", ["y"]), ("ier", ["y"]), ("ied", ["y"]),
    ("ies", ["y"]), ("ing", ["", "e"]), ("er", ["", "e"]), ("ed", ["", "e"]),
    ("es", ["", "e"]), ("s", [""]),
]
# Level 1 keeps only the grammatical (number / tense / degree) suffixes.
INFLECTIONAL = {"iest", "ier", "ied", "ies", "ing", "er", "ed", "es", "s"}
INFL_RULES = [r for r in RULES if r[0] in INFLECTIONAL]
# Comparative/superlative suffixes: only valid when the base is an adjective.
# Gating on POS keeps taller->tall / happier->happy but leaves agentive -er
# separate (baker, hatter, spouter — and kills the junk hatter->hatt).
CMP = {"er", "ier", "iest"}

# Level-3 prefixes. SAFE ones are transparent and cannot create false roots, so
# they strip unconditionally. GUARDED ones (assimilated Latinate negations etc.)
# are stripped only when the word and its base are semantically close (fastText
# cosine >= COS_MIN), so un-/il- negations merge (illegal->legal) while spurious
# splits do not (import->port, react->act, report->port). Longest first.
SAFE_PREFIXES = ["counter", "pseudo", "under", "anti", "semi", "over", "multi", "non", "mis", "un"]
GUARDED_PREFIXES = ["dis", "pre", "ir", "il", "im", "in", "re", "de"]
COS_MIN = 0.35

LEVELS = (1, 2, 3)


def base_of(word: str, freq: dict, rules: list, adj: set | None = None) -> str | None:
    """One suffix-strip step to the max-frequency real word in the family.
    For comparative suffixes the base must be an adjective (see CMP) when `adj`
    is supplied, so agentive -er (baker, hatter) is not stripped."""
    for suf, repls in rules:
        if word.endswith(suf) and len(word) > len(suf) + 1:
            stem = word[: -len(suf)]
            gated = adj is not None and suf in CMP
            hits = [(stem + r, freq[stem + r]) for r in repls
                    if stem + r != word and len(stem + r) >= 3 and freq.get(stem + r) is not None
                    and (not gated or (stem + r) in adj)]
            if hits:
                hits.sort(key=lambda x: -x[1])
                return hits[0][0]
    return None


def prefix_of(word: str, freq: dict, vec_ok) -> str | None:
    """One prefix-strip step (level 3): a SAFE prefix, or a GUARDED one whose
    stripped base is semantically close to the word."""
    for pfx in SAFE_PREFIXES:
        base = word[len(pfx):]
        if word.startswith(pfx) and len(base) >= 3 and freq.get(base) is not None:
            return base
    for pfx in GUARDED_PREFIXES:
        base = word[len(pfx):]
        if word.startswith(pfx) and len(base) >= 3 and freq.get(base) is not None and vec_ok(word, base):
            return base
    return None


def resolve(word: str, freq: dict, rules: list, use_prefix: bool, vec_ok, adj: set, maxdepth: int = 6) -> str | None:
    """Iterate suffix (then, at level 3, prefix) steps to a fixpoint root.
    Returns the root word, or None if the word is already its own root."""
    cur, seen = word, {word}
    for _ in range(maxdepth):
        nxt = base_of(cur, freq, rules, adj)
        if nxt is None and use_prefix:
            nxt = prefix_of(cur, freq, vec_ok)
        if nxt is None or nxt in seen:
            break
        cur = nxt
        seen.add(cur)
    return cur if cur != word else None


def main() -> None:
    con = connect()
    rows = con.execute("SELECT id, word, freq_pm, freq_pm_lc, alpha_only FROM words").fetchall()
    wordmap = {w: i for (i, w, _, _, _) in rows}
    freq = {w: fp for (_, w, fp, _, _) in rows}                # freq_pm (may be None) for picking bases
    freq_pm = {w: (fp or 0.0) for (_, w, fp, _, _) in rows}
    freq_lc = {w: (fl or 0.0) for (_, w, _, fl, _) in rows}
    alpha = [w for (_, w, _, _, a) in rows if a]

    # Level-3 embedding guard: word -> row in the L2-normalized fastText sidecar.
    mat = np.load(EMB_PATH) if EMB_PATH.exists() else None
    rowmap = {w: r for (w, r) in con.execute(
        "SELECT x.word, m.row FROM word_embedding_map m JOIN words x ON x.id = m.word_id")}

    def vec_ok(w: str, base: str) -> bool:
        if mat is None:
            return False
        i, j = rowmap.get(w), rowmap.get(base)
        return i is not None and j is not None and float(mat[i] @ mat[j]) >= COS_MIN

    # adjective set: gates comparative -er/-ier/-iest so agentive -er stays split
    adj = {w for (w,) in con.execute(
        "SELECT DISTINCT x.word FROM word_pos p JOIN words x ON x.id = p.word_id WHERE p.pos = 'adj'")}

    cfg = {1: (INFL_RULES, False), 2: (RULES, False), 3: (RULES, True)}
    lemma_rows: list[tuple] = []                                # (word_id, level, lemma_id)
    fam_pm = {l: defaultdict(float) for l in LEVELS}
    fam_lc = {l: defaultdict(float) for l in LEVELS}
    fam_n = {l: defaultdict(int) for l in LEVELS}
    stem_updates: list[tuple] = []                              # level-2 root -> words.stem

    for w in alpha:
        wid = wordmap[w]
        for lvl in LEVELS:
            rules, use_pfx = cfg[lvl]
            root = resolve(w, freq, rules, use_pfx, vec_ok, adj)
            lid = wordmap[root] if root is not None else wid
            if root is not None:
                lemma_rows.append((wid, lvl, lid))
                if lvl == 2:
                    stem_updates.append((root, w))
            fam_pm[lvl][lid] += freq_pm[w]
            fam_lc[lvl][lid] += freq_lc[w]
            fam_n[lvl][lid] += 1

    lf_rows = [(lvl, lid, fam_pm[lvl][lid], fam_lc[lvl][lid], fam_n[lvl][lid])
               for lvl in LEVELS for lid, n in fam_n[lvl].items() if n >= 2]

    con.execute("DELETE FROM word_lemma")
    con.executemany("INSERT INTO word_lemma(word_id, level, lemma_id) VALUES (?, ?, ?)", lemma_rows)
    con.execute("DELETE FROM lemma_freq")
    con.executemany(
        "INSERT INTO lemma_freq(level, lemma_id, freq_pm, freq_pm_lc, n_members) VALUES (?, ?, ?, ?, ?)",
        lf_rows,
    )
    con.execute("UPDATE words SET stem = NULL")
    con.executemany("UPDATE words SET stem = ? WHERE word = ?", stem_updates)
    con.commit()
    log_ingest(con, "stem", f"{len(lemma_rows)} lemma maps, {len(lf_rows)} families", len(lemma_rows))
    print(f"stem: {len(lemma_rows):,} word_lemma rows (levels 1-3), "
          f"{len(lf_rows):,} multi-member families, {len(stem_updates):,} level-2 stems", flush=True)
    con.close()


if __name__ == "__main__":
    main()
