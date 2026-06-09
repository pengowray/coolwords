"""Ingest WordNet from the bundled SqlUNet SQLite database.

Populates, for coolwords words that match a WordNet lemma (lowercase, space-
separated multiwords — same convention as our headwords):
- words.in_wordnet, words.wordnet_category (primary lexname), words.n_senses
  (COALESCE — only fills where Wiktextract left it NULL)
- word_pos (source='wordnet')
- word_category: every WordNet lexname for the word, e.g. noun.food / noun.animal
  / noun.person — the "kinds of X" feature; is_primary marks the most common sense
- word_relation: hypernym + part/member/substance holonyms & meronyms (synset
  level) and derivation / antonym / pertainym (lemma level)

The source DB has no index on words.lemma, so we run a few bulk joins and
aggregate in Python rather than per-word lookups (the spec measured the full
word*sense*synset join at ~0.7s).
"""
import sqlite3
from collections import defaultdict

from ingest.db import connect, log_ingest
from ingest.paths import WORDNET_SQLITE

POS_MAP = {"n": "noun", "v": "verb", "a": "adj", "s": "adj", "r": "adv"}

# Synset-level relations, mapped by linkid to a CORRECT WordNet label. SqlUNet's
# linktypes table has the holonym/meronym names swapped relative to standard
# WordNet (verified: tree's linkid-11 targets are its parts trunk/branch/crown,
# i.e. meronyms, but linktypes calls 11 "part holonym"), so we relabel here
# rather than trusting linktypes.link. (instance-hypernym 3 skipped.)
SEM_LABEL = {
    1: "hypernym",
    11: "part meronym",
    12: "part holonym",
    13: "member meronym",
    14: "member holonym",
    15: "substance meronym",
    16: "substance holonym",
}
SEM_LINKIDS = tuple(SEM_LABEL)
# lemma-level relations: derivation, antonym, pertainym (linktypes.link is fine here).
LEX_LINKIDS = (81, 30, 80)


def _placeholders(seq) -> str:
    return ",".join("?" * len(seq))


def main() -> None:
    src = sqlite3.connect(f"file:{WORDNET_SQLITE}?mode=ro", uri=True)
    con = connect()
    cur = con.cursor()
    con.execute("PRAGMA synchronous = OFF")

    # idempotent: clear this source's prior output before reloading
    cur.execute("DELETE FROM word_relation WHERE source = 'wordnet'")
    cur.execute("DELETE FROM word_category")
    cur.execute("DELETE FROM word_pos WHERE source = 'wordnet'")
    cur.execute("UPDATE words SET in_wordnet = 0, wordnet_category = NULL")
    con.commit()

    wordmap = {w: i for (i, w) in cur.execute("SELECT id, word FROM words")}

    # --- Step 1: POS, n_senses, categories (one bulk join) ---
    nsenses: dict = defaultdict(int)
    pos_by_word: dict = defaultdict(set)
    cats_by_word: dict = defaultdict(set)
    primary: dict = {}  # lemma -> (lexname, tagcount, sensenum)
    for lemma, pos, tagcount, sensenum, lexname in src.execute(
        """SELECT w.lemma, sy.pos, s.tagcount, s.sensenum, ld.lexdomainname
           FROM words w
           JOIN senses s     ON s.wordid = w.wordid
           JOIN synsets sy   ON sy.synsetid = s.synsetid
           JOIN lexdomains ld ON ld.lexdomainid = sy.lexdomainid"""
    ):
        if lemma not in wordmap:
            continue
        nsenses[lemma] += 1
        pos_by_word[lemma].add(POS_MAP.get(pos, pos))
        cats_by_word[lemma].add(lexname)
        tc = tagcount or 0
        best = primary.get(lemma)
        if best is None or (tc, -sensenum) > (best[1], -best[2]):
            primary[lemma] = (lexname, tc, sensenum)

    wn_updates = [(nsenses[l], primary[l][0], wordmap[l]) for l in nsenses]
    pos_rows = [(wordmap[l], p, "wordnet") for l, ps in pos_by_word.items() for p in ps]
    cat_rows = [(wordmap[l], c, 1 if c == primary[l][0] else 0)
                for l, cs in cats_by_word.items() for c in cs]

    cur.executemany(
        "UPDATE words SET in_wordnet=1, n_senses=COALESCE(n_senses, ?), wordnet_category=? WHERE id=?",
        wn_updates,
    )
    cur.executemany("INSERT OR IGNORE INTO word_pos(word_id, pos, source) VALUES (?, ?, ?)", pos_rows)
    cur.executemany(
        "INSERT OR IGNORE INTO word_category(word_id, category, is_primary) VALUES (?, ?, ?)", cat_rows
    )
    con.commit()
    print(f"wordnet: {len(wn_updates):,} matched lemmas, {len(pos_rows):,} pos, "
          f"{len(cat_rows):,} category rows", flush=True)

    # --- Step 2: relations ---
    seen: set = set()
    rel_rows = []

    def add(lemma, rel, target):
        if lemma not in wordmap or lemma == target:
            return
        key = (wordmap[lemma], rel, target)
        if key not in seen:
            seen.add(key)
            rel_rows.append((wordmap[lemma], rel, target, "wordnet"))

    for lemma, linkid, target in src.execute(
        f"""SELECT w.lemma, sl.linkid, w2.lemma
            FROM words w
            JOIN senses s    ON s.wordid = w.wordid
            JOIN synsets sy  ON sy.synsetid = s.synsetid
            JOIN semlinks sl ON sl.synset1id = sy.synsetid
            JOIN senses s2   ON s2.synsetid = sl.synset2id
            JOIN words w2    ON w2.wordid = s2.wordid
            WHERE sl.linkid IN ({_placeholders(SEM_LINKIDS)})""",
        SEM_LINKIDS,
    ):
        add(lemma, SEM_LABEL[linkid], target)

    for lemma, rel, target in src.execute(
        f"""SELECT w1.lemma, lt.link, w2.lemma
            FROM lexlinks ll
            JOIN words w1 ON w1.wordid = ll.word1id
            JOIN words w2 ON w2.wordid = ll.word2id
            JOIN linktypes lt ON lt.linkid = ll.linkid
            WHERE ll.linkid IN ({_placeholders(LEX_LINKIDS)})""",
        LEX_LINKIDS,
    ):
        add(lemma, rel, target)

    cur.executemany(
        "INSERT OR IGNORE INTO word_relation(word_id, rel, target, source) VALUES (?, ?, ?, ?)", rel_rows
    )
    con.commit()
    log_ingest(con, "wordnet", f"{len(wn_updates)} lemmas, {len(rel_rows)} relations", len(wn_updates))
    print(f"wordnet: {len(rel_rows):,} relation rows", flush=True)
    src.close()
    con.close()


if __name__ == "__main__":
    main()
