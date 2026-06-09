-- Per-book analysis: word occurrences, scored "interesting" candidates, and
-- human ratings. Depends on the dictionary `words` table (schema/dictionary.sql).

CREATE TABLE IF NOT EXISTS books (
    id          INTEGER PRIMARY KEY,
    slug        TEXT UNIQUE,          -- e.g. 'gutenberg-2701'
    title       TEXT,
    author      TEXT,
    source      TEXT,                 -- 'gutenberg'
    source_id   TEXT,
    year        INTEGER,              -- publication year (for the usage-over-time marker)
    n_tokens    INTEGER,              -- total word tokens after boilerplate strip
    n_types     INTEGER,              -- distinct lowercased token types
    ingested_at TEXT
);

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
CREATE TABLE IF NOT EXISTS candidates (
    book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    word_id     INTEGER NOT NULL REFERENCES words(id),
    in_book     INTEGER NOT NULL,     -- occurrences in this book
    score       REAL    NOT NULL,
    s_rarity    REAL,
    s_salience  REAL,                 -- prominence in book vs general corpus (weirdness)
    s_origin    REAL,                 -- unusual-etymology bonus
    s_aesthetic REAL,
    cluster     INTEGER,              -- embedding cluster id (ingest/cluster.py)
    selected    INTEGER NOT NULL DEFAULT 0,  -- 1 if in the varied top-N pick
    rank        INTEGER,
    PRIMARY KEY (book_id, word_id)
);
CREATE INDEX IF NOT EXISTS idx_candidates_rank ON candidates(book_id, rank);

-- Human ratings for the selection UI.
CREATE TABLE IF NOT EXISTS ratings (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    word_id INTEGER NOT NULL REFERENCES words(id),
    rater   TEXT    NOT NULL DEFAULT 'me',
    verdict TEXT,                     -- 'keep' / 'reject' / 'shadow'
    note    TEXT,
    ts      TEXT,
    PRIMARY KEY (book_id, word_id, rater)
);
