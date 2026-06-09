"""Compute words.stem: a suffix-stripped root that resolves to a REAL,
frequency-bearing dictionary word.

This surfaces lemma/base relationships (hoarsely->hoarse, importantly->important,
darkness->dark) for the rating UI WITHOUT destructively lemmatizing — "hatter"
keeps its own entry; we just expose that "hoarsely" relates to the common
"hoarse" so the user can judge it.

Design notes (verified): resolve to a real word (not a Porter stem); pick the
MAX-frequency candidate WITHIN one morphological family, trying rules in priority
order (so sleepily->sleepy, not sleep); require the base to have a frequency
(rejects junk dictionary forms). NO blind prefix stripping — that would merge
import/port/report. Prefix pairs (un-/in-/...) are surfaced separately in the UI.

Run after ngrams/fiction (needs words.freq_pm). Idempotent.
"""
from ingest.db import connect, log_ingest

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


def base_of(word: str, freq: dict) -> str | None:
    for suf, repls in RULES:
        if word.endswith(suf) and len(word) > len(suf) + 1:
            stem = word[: -len(suf)]
            hits = [(stem + r, freq[stem + r]) for r in repls
                    if stem + r != word and len(stem + r) >= 3 and freq.get(stem + r) is not None]
            if hits:
                hits.sort(key=lambda x: -x[1])
                return hits[0][0]
    return None


def root_of(word: str, freq: dict, maxdepth: int = 4) -> str | None:
    cur, seen = word, {word}
    for _ in range(maxdepth):
        b = base_of(cur, freq)
        if b is None or b in seen:
            break
        cur = b
        seen.add(cur)
    return cur if cur != word else None


def main() -> None:
    con = connect()
    rows = con.execute("SELECT word, freq_pm, alpha_only FROM words").fetchall()
    freq = {w: fp for (w, fp, _) in rows}
    updates = [(root_of(w, freq), w) for (w, _, alpha) in rows if alpha]
    updates = [(r, w) for (r, w) in updates if r is not None]

    con.execute("UPDATE words SET stem = NULL")
    con.executemany("UPDATE words SET stem = ? WHERE word = ?", updates)
    con.commit()
    log_ingest(con, "stem", "suffix-root base forms", len(updates))
    print(f"stem: {len(updates):,} words resolved to a base-form stem")
    con.close()


if __name__ == "__main__":
    main()
