"""Print a summary of the dictionary database plus sample queries.

A quick, comprehensive sanity check after a build.
"""
from ingest.db import connect


def main() -> None:
    con = connect()
    q = con.execute

    print("== row counts ==")
    for table in ("words", "word_pronunciations", "word_pos", "word_etymology",
                  "word_category", "word_relation", "ngram_freq", "fiction_freq"):
        n = q(f"SELECT count(*) FROM {table}").fetchone()[0]
        print(f"  {table:24} {n:>10,}")

    print("\n== words provenance / flags ==")
    for col in ("in_cmudict", "in_ngram1m", "in_fiction_placeholder", "in_wordnet",
                "in_wiktionary", "in_wordle", "wordle_answer", "in_fasttext",
                "is_proper", "is_form_of", "is_offensive"):
        if col == "in_fiction_placeholder":
            n = q("SELECT count(*) FROM words WHERE fic_count IS NOT NULL").fetchone()[0]
            print(f"  {'has_fiction_freq':24} {n:>10,}")
            continue
        n = q(f"SELECT count(*) FROM words WHERE {col} = 1").fetchone()[0]
        print(f"  {col:24} {n:>10,}")
    n_phrase = q("SELECT count(*) FROM words WHERE is_phrase = 1").fetchone()[0]
    print(f"  {'is_phrase (multiword)':24} {n_phrase:>10,}")

    print("\n== top etymology source languages ==")
    for lang, n in q("SELECT etymology_lang, count(*) FROM words WHERE etymology_lang IS NOT NULL "
                     "GROUP BY etymology_lang ORDER BY count(*) DESC LIMIT 12").fetchall():
        print(f"  {lang:8} {n:>8,}")

    print("\n== top WordNet categories ==")
    for cat, n in q("SELECT category, count(*) FROM word_category GROUP BY category "
                    "ORDER BY count(*) DESC LIMIT 12").fetchall():
        print(f"  {cat:18} {n:>8,}")

    print("\n== sample: noun.food words ==")
    foods = q("SELECT word FROM word_category JOIN words ON words.id=word_category.word_id "
              "WHERE category='noun.food' AND alpha_only=1 ORDER BY freq_rank LIMIT 12").fetchall()
    print("  " + ", ".join(w for (w,) in foods))

    print("\n== sample: words of Persian (fa) origin ==")
    fa = q("SELECT word FROM words WHERE etymology_lang='fa' AND alpha_only=1 "
           "ORDER BY freq_rank LIMIT 12").fetchall()
    print("  " + ", ".join(w for (w,) in fa))

    print("\n== highest cap_ratio (proper-noun signal), freq_pm>1 ==")
    for w, cr in q("SELECT word, cap_ratio FROM words WHERE cap_ratio IS NOT NULL AND freq_pm>1 "
                   "AND alpha_only=1 AND length>=4 ORDER BY cap_ratio DESC, freq_pm DESC LIMIT 8").fetchall():
        print(f"  {cr:.3f}  {w}")

    print("\n== last ingest runs ==")
    for source, detail, rows, ts in q(
            "SELECT source, detail, rows, ts FROM ingest_log ORDER BY ts DESC LIMIT 10").fetchall():
        print(f"  {ts}  {source:16} {rows:>10,}  {detail}")

    con.close()


if __name__ == "__main__":
    main()
