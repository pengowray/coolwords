-- Per-book analysis: word occurrences, scored "interesting" candidates, and
-- human ratings. Depends on the dictionary `words` table (schema/dictionary.sql).

CREATE TABLE IF NOT EXISTS books (
    id            INTEGER PRIMARY KEY,
    slug          TEXT UNIQUE,        -- e.g. 'gutenberg-2701'
    title         TEXT,
    author        TEXT,
    source        TEXT,               -- 'gutenberg'
    source_id     TEXT,
    year          INTEGER,            -- publication year (for the usage-over-time marker)
    n_tokens      INTEGER,            -- total word tokens after boilerplate strip
    n_types       INTEGER,            -- distinct lowercased token types
    ingested_at   TEXT,
    content_hash  TEXT,               -- sha256 of the normalized kept token stream (dedup)
    format        TEXT,               -- source format: 'txt' | 'epub' | 'pdf'
    orig_filename TEXT,               -- the dropped file's original name (provenance)
    text_source   TEXT                -- PDFs: 'embedded' | 'ocr:tesseract' | 'ocr:rapidocr'
);
-- NOTE: the idx_books_content_hash index (for exact-content dedup) is created in
-- ingest/db.py:connect() AFTER ensure_columns, since content_hash is migrated in
-- on existing DBs and wouldn't exist yet when this file runs.

-- One row per distinct token in the book; word_id is NULL when the token is not
-- in the dictionary (typos / scannos / possessives / OOV).
CREATE TABLE IF NOT EXISTS book_occurrences (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    token   TEXT    NOT NULL,
    word_id INTEGER REFERENCES words(id),
    count   INTEGER NOT NULL,
    example TEXT,                       -- first sentence in the book containing the token
    PRIMARY KEY (book_id, token)
);
CREATE INDEX IF NOT EXISTS idx_book_occ_word ON book_occurrences(word_id);

-- Scored interesting-word candidates for a book (output of ingest/score.py).
-- One row per (book, stemming level, group representative): word_id is the
-- lemma/root at that level, and merged surface forms are aggregated into in_book
-- and n_forms. Level 0 = no merging (word_id is the surface form, n_forms = 1).
CREATE TABLE IF NOT EXISTS candidates (
    book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    level       INTEGER NOT NULL DEFAULT 0,   -- stemming aggressiveness 0..3 (0 = none)
    word_id     INTEGER NOT NULL REFERENCES words(id),  -- group representative (lemma at this level)
    in_book     INTEGER NOT NULL,     -- combined occurrences of all merged forms in this book
    n_forms     INTEGER NOT NULL DEFAULT 1,   -- distinct in-book surface forms merged here
    score       REAL    NOT NULL,
    s_rarity    REAL,
    s_salience  REAL,                 -- prominence in book vs general corpus (weirdness)
    s_origin    REAL,                 -- unusual-etymology bonus
    s_aesthetic REAL,
    cluster     INTEGER,              -- embedding cluster id (ingest/cluster.py)
    selected    INTEGER NOT NULL DEFAULT 0,  -- 1 if in the varied top-N pick
    rank        INTEGER,              -- per (book_id, level)
    PRIMARY KEY (book_id, level, word_id)
);
CREATE INDEX IF NOT EXISTS idx_candidates_rank ON candidates(book_id, level, rank);

-- Human ratings for the selection UI (legacy single verdict; superseded by word_tags).
CREATE TABLE IF NOT EXISTS ratings (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    word_id INTEGER NOT NULL REFERENCES words(id),
    rater   TEXT    NOT NULL DEFAULT 'me',
    verdict TEXT,                     -- 'keep' / 'reject' / 'shadow'
    note    TEXT,
    ts      TEXT,
    PRIMARY KEY (book_id, word_id, rater)
);

-- NOTE: human tags now live in their own self-contained database (data/user.db,
-- schema/user.sql), keyed by stable text (book slug + headword) so they survive
-- re-imports and dictionary rebuilds. The old word_tags table here is migrated
-- and dropped by ingest/userdb.py.

-- Speeds the relation-target "is it in this book?" LEFT JOIN in word_detail.
CREATE INDEX IF NOT EXISTS idx_bo_wordid_book ON book_occurrences(word_id, book_id);
