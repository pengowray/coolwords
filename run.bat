@echo off
REM ===========================================================================
REM  coolwords - start the rating web UI (Leptos SSR + axum + rusqlite)
REM  Serves http://127.0.0.1:3000 with hot reload. Press Ctrl+C to stop.
REM ===========================================================================
REM  One-time toolchain setup:
REM      rustup target add wasm32-unknown-unknown
REM      cargo install cargo-leptos
REM  The UI reads ..\data\coolwords.db (override with the COOLWORDS_DB env var).
REM
REM  ---- Build the word-properties dictionary (Python stdlib, run from repo root) ----
REM      python -m ingest.cmudict        :: words + pronunciations (syllables, rhyme, stress)
REM      python -m ingest.wordle         :: wordle legal-guess / answer flags
REM      python -m ingest.wiktextract    :: ~1.14M English headwords: POS, etymology, senses, gloss, flags
REM      python -m ingest.derive         :: deterministic shape features (length, scrabble, ...)
REM      python -m ingest.ngrams         :: general frequency (case-aware) + cap_ratio proper-noun signal
REM      python -m ingest.fiction        :: fiction genre baseline (the "weirdness ratio")
REM      python -m ingest.wordnet        :: noun.food/animal/person categories + hypernym/holo/meronym relations
REM      python -m ingest.langnames      :: etymology language code -> full English name
REM      python -m ingest.embeddings     :: fastText vectors -> data\coolwords_emb.npy
REM      python -m ingest.stem           :: per-level lemmas word_lemma + lemma_freq (AFTER ngrams/fiction
REM                                      ::   for frequency, AND embeddings for the level-3 prefix guard)
REM      python -m ingest.userdb         :: create/migrate the separate per-user tag DB (data\user.db);
REM                                      ::   self-contained, text-keyed, survives rebuilds. Override path
REM                                      ::   for the UI with COOLWORDS_USER_DB. Run once (idempotent).
REM      python -m ingest.stats          :: summary / verification report
REM
REM  ---- Analyze a book (book -> per-level scored candidates -> clusters + varied top-20) ----
REM      python -m ingest.book data\books\moby_dick_2701.txt --slug gutenberg-2701 --title "Moby-Dick" --author "Herman Melville" --source-id 2701
REM      python -m ingest.score   --slug gutenberg-2701      :: scores all merge levels 0-3
REM      python -m ingest.cluster --slug gutenberg-2701 --k 16 --top 20   :: clusters all levels
REM      python -m ingest.trajectory     :: per-decade usage (in-book words + their roots); run after books
REM      python -m ingest.refresh_freq   :: re-copy ngram/fiction freq onto words after adding new headwords
REM ===========================================================================

cd /d "%~dp0ui"
cargo leptos watch
