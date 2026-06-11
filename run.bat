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
REM
REM  ---- Import a book without the CLI (drag-drop in the web UI, or by hand) ----
REM    The "+ import book" page (/import) handles .txt, .epub and .pdf: it auto-
REM    detects title/author/year, dedups by content hash, shows kept-vs-stripped
REM    regions, then copies the file into the books dir and runs the pipeline above.
REM      python -m ingest.import_book --inspect path\to\book.epub      :: preview JSON (metadata + segments), no writes
REM      python -m ingest.import_book --commit  path\to\book.epub --slug my-book --title "..." --author "..." --year 1984
REM    Books are copied to %%COOLWORDS_BOOKS_DIR%% (default D:\datasets\coolwords\books).
REM    Override it (and where the UI reads it) in a repo-root .env:  COOLWORDS_BOOKS_DIR=D:\path\to\books
REM
REM  ---- PDF support (extraction + re-OCR + embedded-vs-OCR compare) ----
REM    pip install pymupdf                       :: REQUIRED for any PDF (extract + page render)
REM    OCR engines (either; for scans or to re-OCR a bad embedded layer):
REM      winget install UB-Mannheim.TesseractOCR :: tesseract (fast, system install)
REM      pip install rapidocr-onnxruntime        :: rapidocr (pip-only; better on noisy scans)
REM    .env knobs: COOLWORDS_OCR_ENGINE=tesseract^|rapidocr ; COOLWORDS_TESSERACT=path\to\tesseract.exe
REM    Import is always fast (embedded text); manage OCR after the fact on /books
REM    (background jobs with progress + cancel). The cache lives beside the book:
REM    %%COOLWORDS_BOOKS_DIR%%\<slug>.pdf.ocr.<engine>.json
REM      python -m ingest.import_book --ocr-status SLUG                 :: cache + source state
REM      python -m ingest.import_book --ocr-book SLUG --engine tesseract:: OCR all pages (resumable)
REM      python -m ingest.import_book --ocr-compare path\to\book.pdf [--engine E] [--pages 1,5,9-12]
REM      python -m ingest.import_book --reingest SLUG --text-source embedded^|ocr:tesseract
REM ===========================================================================

cd /d "%~dp0ui"
cargo leptos watch
