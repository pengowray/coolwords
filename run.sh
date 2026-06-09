#!/usr/bin/env bash
# ============================================================================
#  coolwords - start the rating web UI (Leptos SSR + axum + rusqlite)
#  Serves http://127.0.0.1:3000 with hot reload. Press Ctrl+C to stop.
# ============================================================================
#  One-time toolchain setup:
#      rustup target add wasm32-unknown-unknown
#      cargo install cargo-leptos
#  The UI reads ../data/coolwords.db (override with the COOLWORDS_DB env var).
#
#  ---- Build the word-properties dictionary (Python stdlib, run from repo root) ----
#      python -m ingest.cmudict        # words + pronunciations (syllables, rhyme, stress)
#      python -m ingest.wordle         # wordle legal-guess / answer flags
#      python -m ingest.wiktextract    # ~1.14M English headwords: POS, etymology, senses, gloss, flags
#      python -m ingest.derive         # deterministic shape features (length, scrabble, ...)
#      python -m ingest.ngrams         # general frequency (case-aware) + cap_ratio proper-noun signal
#      python -m ingest.fiction        # fiction genre baseline (the "weirdness ratio")
#      python -m ingest.wordnet        # noun.food/animal/person categories + hypernym/holo/meronym relations
#      python -m ingest.langnames      # etymology language code -> full English name
#      python -m ingest.embeddings     # fastText vectors -> data/coolwords_emb.npy
#      python -m ingest.stem           # suffix-root base forms (run AFTER ngrams/fiction; needs frequency)
#      python -m ingest.stats          # summary / verification report
#
#  ---- Analyze a book (book -> scored candidates -> clusters + varied top-20) ----
#      python -m ingest.book data/books/moby_dick_2701.txt --slug gutenberg-2701 --title "Moby-Dick" --author "Herman Melville" --source-id 2701
#      python -m ingest.score   --slug gutenberg-2701
#      python -m ingest.cluster --slug gutenberg-2701 --k 16 --top 20
#      python -m ingest.refresh_freq   # re-copy ngram/fiction freq onto words after adding new headwords
# ============================================================================
set -euo pipefail
cd "$(dirname "$0")/ui"
exec cargo leptos watch
