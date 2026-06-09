# coolwords

Find unique, rare, and interesting words in a text (a book, an article), plus
words specific to a genre or domain. Feed in a public-domain book, get out a
ranked list of its most interesting words, refined with human ratings.

## Architecture (hybrid)

- **Rust / Leptos** — the app, web UI, book ingestion, tokenization, and the
  deterministic feature computation. *(not built yet)*
- **Python (stdlib + ML)** — offline ETL of reference datasets, and the
  NLP-heavy steps (lemmatization/NER, embeddings, clustering). *(in progress)*
- **SQLite** — the spine and the contract between the two. Everything reads and
  writes one database; there is no service/API glue.

## The word-properties dictionary (current focus)

A *book-independent* reference database of words and their properties, built
from external datasets. It is the foundation everything else queries: rarity
signals, cross-book connection tags ("4-syllable words", "words containing q"),
filters (proper nouns, lemma variants), and the public word lookup.

The dictionary build uses **only the Python standard library** (`sqlite3`,
`gzip`, `zipfile`, `json`, `csv`) — no third-party packages required.

### Sources

| Source | Gives us | Status |
|--------|----------|--------|
| CMUdict | pronunciation, syllables, rhyme key, stress | ✅ |
| Wordle lists | legal-guess / answer flags | ✅ |
| Google ngrams (1M) | frequency, rank, decade trajectory | ⬜ |
| Google ngrams (fiction) | genre-relative frequency | ⬜ |
| WordNet (SqlUNet) | POS, hypernyms, sense counts | ⬜ |
| Wiktextract | POS, etymology language, senses, phrases | ⬜ |
| fastText / word2vec | embeddings (similarity, shadowing) | ⬜ |

Dataset locations are configured in [ingest/paths.py](ingest/paths.py).

### Build

Run from the project root (rebuilds `data/coolwords.db`):

```pwsh
python -m ingest.cmudict   # words + pronunciations
python -m ingest.wordle    # wordle flags
python -m ingest.derive    # deterministic shape features (run last)
python -m ingest.stats     # summary + sample queries
```
