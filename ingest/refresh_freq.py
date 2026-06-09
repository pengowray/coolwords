"""Re-copy ngram_freq / fiction_freq aggregates onto the words table.

Cheap (no corpus streaming): run after an ingest that adds new headwords (e.g.
wiktextract) so the new words pick up frequency from the already-built tables.
"""
from ingest.db import connect
from ingest.freq import copy_general_to_words, copy_fiction_to_words


def main() -> None:
    con = connect()
    g = copy_general_to_words(con)
    f = copy_fiction_to_words(con)
    print(f"refresh_freq: {g:,} words with general freq, {f:,} with fiction freq")
    con.close()


if __name__ == "__main__":
    main()
