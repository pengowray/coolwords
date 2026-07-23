# coolwords

Find unique, rare, and interesting words in a text (a book, an article), plus
words specific to a genre or domain. Feed in a public-domain book, get out a
ranked, clustered list of its most interesting words, then refine it with human
ratings down to a varied final shortlist.

## Architecture (hybrid)

- **Python (stdlib + numpy)** — offline ETL of reference datasets, the book
  ingestion + scoring pipeline, and the embedding clustering/shadowing.
- **Rust / Leptos (SSR + axum)** — the rating web UI; server functions read
  candidates and write ratings directly to the database.
- **SQLite** — the spine and the contract between the two. One database
  (`data/coolwords.db`) plus an embedding sidecar (`data/coolwords_emb.npy`).
  Everything reads and writes it; there is no service/API glue.

All of `data/` is reproducible from the scripts below and is gitignored.

## 1. The word-properties dictionary

A book-independent reference DB of ~1.18M words and their properties, built from
external datasets (paths in [ingest/paths.py](ingest/paths.py)). Uses only the
Python standard library.

| Source | Gives us |
|--------|----------|
| CMUdict | pronunciation, syllables, rhyme key, stress |
| Google ngrams (1M) | frequency, rank, per-million; `cap_ratio` proper-noun signal |
| Google ngrams (fiction) | genre baseline → the "weirdness ratio" |
| Wiktextract | real-word gate, POS, **etymology language**, senses, gloss, flags |
| WordNet (SqlUNet) | `noun.food`/`noun.animal`/… categories, hypernym/holonym/meronym |
| Wordle | legal-guess / answer flags |
| fastText crawl-300d-2M | embeddings (similarity, shadowing) → `.npy` sidecar |

```pwsh
# build the dictionary (run from the project root)
python -m ingest.cmudict      # words + pronunciations
python -m ingest.wordle       # wordle flags
python -m ingest.wiktextract  # ~1.14M English headwords: POS, etymology, senses, gloss, flags
python -m ingest.derive       # deterministic shape features (run after word-adders)
python -m ingest.ngrams       # general frequency (case-aware) + cap_ratio
python -m ingest.fiction      # fiction genre baseline
python -m ingest.wordnet      # categories + relations
python -m ingest.embeddings   # fastText vectors -> data/coolwords_emb.npy
python -m ingest.stats        # summary report
```

## 2. Vertical slice: book → interesting words

```pwsh
# ingest a Project Gutenberg book, score it, cluster + pick a varied top-20
python -m ingest.book data/books/moby_dick_2701.txt --slug gutenberg-2701 `
    --title "Moby-Dick" --author "Herman Melville" --source-id 2701
python -m ingest.score   --slug gutenberg-2701      # ranked candidates
python -m ingest.cluster --slug gutenberg-2701 --k 16 --top 20   # clusters + shadowing
```

- **score** filters the "less-interesting" categories (non-words, inflections,
  proper nouns, offensive, too-common) and scores the rest by **rarity**,
  **book-salience** (weirdness/x-ray), **unusual etymology**, and **aesthetics**.
- **cluster** groups candidates by embedding (themes) and greedily picks a
  varied top-20, skipping words too similar to an already-picked one (shadowing).

### Finding books: the catalog

Rather than hunting for files by hand, mirror the public catalogs locally once
and search them offline:

```pwsh
python -m ingest.catalog --sync all              # ~75k Gutenberg + ~1.5k Standard Ebooks
python -m ingest.catalog --search "melville" --limit 10
python -m ingest.catalog --grab '[{"source":"gutenberg","source_id":"2701"}]'
```

`--grab` downloads and imports without scoring; run `python -m ingest.import_book
--rescore <slug>` per book afterwards (the UI queues this for you). Add
`--max-pages N` / `--limit-rows N` to cap a sync when smoke-testing the parsers.

Politeness is baked in, and the rules are not optional:

- **Gutenberg downloads go through a mirror** (`COOLWORDS_GUTENBERG_MIRROR`,
  default `https://gutenberg.pglaf.org`). Their robot policy forbids automated
  crawling of `www.gutenberg.org` pages but explicitly sanctions the offline
  catalog file and mirror downloads — so we fetch the one gzipped
  `pg_catalog.csv.gz` from the main site and everything else from the mirror,
  with a 2s gap between requests (their own `wget -w 2` guidance).
- **Standard Ebooks is parsed from the public HTML catalog**, not the OPDS feed:
  `/feeds/opds` and the bulk archives are gated behind Patrons Circle membership
  and return 401. The `?view=list` pages are RDFa-annotated and public; 1s
  between requests.
- Every request carries a descriptive User-Agent and goes through one per-host
  throttle. Syncing is an explicit action — searching never touches the network,
  and a book already in the DB is never re-downloaded.

## 3. Rating UI (Rust / Leptos)

```pwsh
cd ui
cargo leptos watch      # serves http://127.0.0.1:3000
```

Browse a book's candidates with gloss / origin / category / cluster, switch
between books, filter to the varied top-20, and rate each word keep / reject /
shadow. Ratings persist to the `ratings` table in the live database.

Requires the toolchain: `rustup target add wasm32-unknown-unknown`,
`cargo install cargo-leptos`. Reads the DB at `../data/coolwords.db` (override
with the `COOLWORDS_DB` env var).
