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

-- Multi-tag human judgments: a row's PRESENCE means the tag is ON for (book, word).
-- Toggling a tag off deletes its row. Replaces the single ratings.verdict.
CREATE TABLE IF NOT EXISTS word_tags (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    word_id INTEGER NOT NULL REFERENCES words(id),
    tag     TEXT    NOT NULL,         -- useful | strange | interesting | aesthetic | emblematic | category-pick
    rater   TEXT    NOT NULL DEFAULT 'me',
    ts      TEXT,
    PRIMARY KEY (book_id, word_id, tag, rater)
);
CREATE INDEX IF NOT EXISTS idx_word_tags_book_word ON word_tags(book_id, word_id);

-- Speeds the relation-target "is it in this book?" LEFT JOIN in word_detail.
CREATE INDEX IF NOT EXISTS idx_bo_wordid_book ON book_occurrences(word_id, book_id);
