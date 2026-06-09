"""Flag words that are legal Wordle guesses / answers.

Uses the NYT lists. Every answer is also a legal guess, so the guess set is the
union of both files. Words not already present (some obscure guesses) are added.
"""
from pathlib import Path

from ingest.db import connect, log_ingest
from ingest.paths import WORDLE_DIR


def load(path: Path) -> set[str]:
    words = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        w = line.strip().lower()
        if len(w) == 5 and w.isalpha():
            words.add(w)
    return words


def main() -> None:
    answers = load(WORDLE_DIR / "wordle-nyt-answers-alphabetical.txt")
    guesses = load(WORDLE_DIR / "wordle-nyt-allowed-guesses.txt") | answers

    con = connect()
    cur = con.cursor()
    n_new = 0
    for w in guesses:
        cur.execute("INSERT OR IGNORE INTO words(word, in_wordle) VALUES (?, 1)", (w,))
        n_new += cur.rowcount

    cur.executemany("UPDATE words SET in_wordle = 1 WHERE word = ?", [(w,) for w in guesses])
    cur.executemany("UPDATE words SET wordle_answer = 1 WHERE word = ?", [(w,) for w in answers])
    con.commit()
    log_ingest(con, "wordle", f"{len(guesses)} guesses / {len(answers)} answers", len(guesses))
    print(f"wordle: {len(guesses)} guesses ({len(answers)} answers), {n_new} new words")
    con.close()


if __name__ == "__main__":
    main()
