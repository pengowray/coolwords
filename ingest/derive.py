"""Compute deterministic shape features for every word.

Run last, after the word-adding ingests, so every row (whatever its source)
gets these. Idempotent.
"""
import re

from ingest.db import connect, log_ingest

_SCRABBLE = {
    **dict.fromkeys("eaionrtlsu", 1),
    **dict.fromkeys("dg", 2),
    **dict.fromkeys("bcmp", 3),
    **dict.fromkeys("fhvwy", 4),
    "k": 5,
    **dict.fromkeys("jx", 8),
    **dict.fromkeys("qz", 10),
}
_RARE = set("jkqxz")
_ALPHA = re.compile(r"^[a-z]+$")


def scrabble(word: str) -> int:
    return sum(_SCRABBLE.get(c, 0) for c in word)


def main() -> None:
    con = connect()
    rows = con.execute("SELECT id, word FROM words").fetchall()
    updates = []
    for wid, word in rows:
        alpha_only = 1 if _ALPHA.match(word) else 0
        updates.append((
            len(word),                                              # char_len
            sum(c.isalpha() for c in word),                         # length (letters)
            len(word.split()),                                      # n_tokens
            1 if " " in word else 0,                                # is_phrase
            alpha_only,
            scrabble(word) if alpha_only else None,                 # scrabble
            "".join(sorted(c for c in set(word) if c in _RARE)) or None,  # rare_letters
            wid,
        ))

    con.executemany(
        "UPDATE words SET char_len=?, length=?, n_tokens=?, is_phrase=?, "
        "alpha_only=?, scrabble=?, rare_letters=? WHERE id=?",
        updates,
    )
    con.commit()
    log_ingest(con, "derive", "shape features", len(updates))
    print(f"derive: {len(updates)} words updated")
    con.close()


if __name__ == "__main__":
    main()
