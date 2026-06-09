"""Ingest CMUdict pronunciations into the words dictionary.

CMUdict format (latin-1):  WORD  PH PH PH ...
- vowel phones carry a stress digit (0/1/2); consonants do not
- alternate pronunciations are written WORD(2), WORD(3), ...
- comment lines start with ';;;'

Derived per pronunciation:
- syllables : number of stress-bearing (vowel) phones
- stress    : the stress digits in order, e.g. '102'
- rhyme_key : phones from the last primary-stressed vowel to the end, stress
              digits removed (so 'abacus' -> 'AE B AH K AH S')
"""
import re

from ingest.db import connect, log_ingest
from ingest.paths import CMUDICT


def analyze(phones: list[str]) -> tuple[int, str, str | None]:
    syllables = sum(1 for p in phones if any(c.isdigit() for c in p))
    stress = "".join(c for p in phones for c in p if c.isdigit())

    # Rhyme starts at the last primary-stressed vowel; fall back to the last
    # vowel of any stress if there is no primary stress.
    idx = None
    for i, p in enumerate(phones):
        if p.endswith("1"):
            idx = i
    if idx is None:
        for i, p in enumerate(phones):
            if any(c.isdigit() for c in p):
                idx = i
    rhyme = None
    if idx is not None:
        rhyme = " ".join(re.sub(r"\d", "", p) for p in phones[idx:])
    return syllables, stress, rhyme


def parse_line(line: str) -> tuple[str, list[str]]:
    """Return (lowercased headword, phones).

    CMUdict alternates are written WORD, WORD(1), WORD(2)... where the *base*
    entry is the first pronunciation and (1) is the second. The parenthetical is
    therefore a 1-based alternate index, not a variant id, so we strip it and let
    the caller number variants sequentially in file order.
    """
    parts = line.split()
    raw, phones = parts[0], parts[1:]
    if raw.endswith(")") and "(" in raw:
        base, _, num = raw.rpartition("(")
        if num[:-1].isdigit():
            raw = base
    return raw.lower(), phones


def is_real_headword(word: str) -> bool:
    """Keep words that start with a letter, or an apostrophe (clipped forms like
    'tis / 'twas / 'em). Drops CMUdict symbol-name entries (!exclamation-point,
    +plus, #hash-mark, 3-d, ...) that start with punctuation or a digit."""
    return bool(word) and (word[0].isalpha() or word[0] == "'")


def main() -> None:
    con = connect()
    cur = con.cursor()
    cache: dict[str, int] = {}
    variants: dict[int, int] = {}  # word_id -> count of pronunciations seen so far
    n_words = n_pron = 0

    with open(CMUDICT, encoding="utf-8") as f:
        for line in f:
            if line.startswith(";;;") or not line.strip():
                continue
            word, phones = parse_line(line)
            if not phones or not is_real_headword(word):
                continue

            wid = cache.get(word)
            if wid is None:
                cur.execute("INSERT OR IGNORE INTO words(word, in_cmudict) VALUES (?, 1)", (word,))
                if cur.rowcount:
                    wid = cur.lastrowid
                    n_words += 1
                else:
                    wid = cur.execute("SELECT id FROM words WHERE word = ?", (word,)).fetchone()[0]
                cache[word] = wid
            variant = variants.get(wid, 0) + 1
            variants[wid] = variant

            arpabet = " ".join(phones)
            syllables, stress, rhyme = analyze(phones)
            cur.execute(
                "INSERT OR REPLACE INTO word_pronunciations"
                "(word_id, variant, arpabet, syllables, rhyme_key, stress) VALUES (?, ?, ?, ?, ?, ?)",
                (wid, variant, arpabet, syllables, rhyme, stress),
            )
            n_pron += 1
            if variant == 1:
                cur.execute(
                    "UPDATE words SET in_cmudict = 1, arpabet = ?, syllables = ?, rhyme_key = ?, stress = ? "
                    "WHERE id = ?",
                    (arpabet, syllables, rhyme, stress, wid),
                )

    con.commit()
    log_ingest(con, "cmudict", str(CMUDICT), n_pron)
    print(f"cmudict: {n_words} new words, {n_pron} pronunciations")
    con.close()


if __name__ == "__main__":
    main()
