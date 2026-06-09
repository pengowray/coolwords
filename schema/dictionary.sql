-- coolwords word-properties dictionary (book-independent reference data).
-- Built from external datasets (CMUdict, Google ngrams, WordNet, Wiktionary,
-- Wordle, word vectors). One row per lowercased headword in `words`; data that
-- is 1:many per word lives in satellite tables.
--
-- Idempotent: safe to run repeatedly (IF NOT EXISTS everywhere). Columns added
-- after a table's first release are also listed in ingest/db.py _MIGRATIONS so
-- existing databases converge without a full rebuild.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS words (
    id              INTEGER PRIMARY KEY,
    word            TEXT    NOT NULL UNIQUE,    -- lowercased headword (may contain spaces/hyphens)

    -- shape / deterministic (ingest/derive.py)
    char_len        INTEGER,                    -- total characters
    length          INTEGER,                    -- count of alphabetic characters
    n_tokens        INTEGER,                    -- whitespace-separated tokens
    is_phrase       INTEGER,                    -- 1 if multi-word (contains a space)
    alpha_only      INTEGER,                    -- 1 if matches /^[a-z]+$/
    scrabble        INTEGER,                    -- scrabble score (alpha_only words only; NULL otherwise)
    rare_letters    TEXT,                       -- subset of {j,k,q,x,z} present, sorted

    -- pronunciation (ingest/cmudict.py; primary variant — see word_pronunciations)
    arpabet         TEXT,
    syllables       INTEGER,
    rhyme_key       TEXT,                       -- phones from the last PRIMARY-stressed (1) vowel to the
                                                -- end (fallback: last stress-bearing vowel), stress stripped
    stress          TEXT,                       -- stress-digit string, e.g. '102'

    -- general frequency (ingest/ngrams.py — Google 1M corpus, all-case surface total)
    freq_count      INTEGER,                    -- summed match count across all case variants
    freq_rank       INTEGER,                    -- positional id by count desc, alphabetical tie-break
                                                -- (NOT a dense rank; equal-count words differ — use freq_pm for ties)
    freq_pm         REAL,                       -- occurrences per million (all-case)
    freq_pm_lc      REAL,                       -- per million counting only the all-lowercase surface form
    cap_ratio       REAL,                       -- fraction of occurrences that were capitalized (proper-noun signal)

    -- genre-baseline frequency (ingest/fiction.py — Google fiction sub-corpus)
    fic_count       INTEGER,
    fic_rank        INTEGER,
    fic_pm          REAL,                       -- per-million within fiction

    -- lexical (ingest/wordnet.py, ingest/wiktextract.py)
    n_senses        INTEGER,
    etymology_lang  TEXT,                       -- primary source language code (e.g. 'fa', 'it')
    etymology_text  TEXT,                       -- human-readable etymology string (display/fallback)
    wordnet_category TEXT,                      -- denormalized primary WordNet lexname (e.g. 'noun.food')

    -- classification flags (ingest/wiktextract.py)
    is_proper       INTEGER NOT NULL DEFAULT 0, -- any sense is a proper noun (pos='name')
    is_form_of      INTEGER NOT NULL DEFAULT 0, -- inflected / alternative form of another lemma
    is_offensive    INTEGER NOT NULL DEFAULT 0, -- any sense tagged vulgar/offensive/derogatory/slur

    -- provenance flags
    in_cmudict      INTEGER NOT NULL DEFAULT 0,
    in_ngram1m      INTEGER NOT NULL DEFAULT 0,
    in_wordnet      INTEGER NOT NULL DEFAULT 0,
    in_wiktionary   INTEGER NOT NULL DEFAULT 0,
    in_wordle       INTEGER NOT NULL DEFAULT 0, -- legal Wordle guess
    wordle_answer   INTEGER NOT NULL DEFAULT 0, -- in the Wordle answer list
    in_fasttext     INTEGER NOT NULL DEFAULT 0  -- has a fastText embedding
);

CREATE INDEX IF NOT EXISTS idx_words_freq_rank ON words(freq_rank);
CREATE INDEX IF NOT EXISTS idx_words_syllables ON words(syllables);
CREATE INDEX IF NOT EXISTS idx_words_rhyme     ON words(rhyme_key);
CREATE INDEX IF NOT EXISTS idx_words_length    ON words(length);
CREATE INDEX IF NOT EXISTS idx_words_etymology ON words(etymology_lang);

-- All pronunciations including variants (CMUdict WORD(1), WORD(2)... entries).
CREATE TABLE IF NOT EXISTS word_pronunciations (
    word_id   INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    variant   INTEGER NOT NULL DEFAULT 1,
    arpabet   TEXT    NOT NULL,
    syllables INTEGER,
    rhyme_key TEXT,
    stress    TEXT,
    PRIMARY KEY (word_id, variant)
);

-- Part of speech, possibly several per word, attributed to a source.
CREATE TABLE IF NOT EXISTS word_pos (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    pos     TEXT    NOT NULL,
    source  TEXT    NOT NULL,
    PRIMARY KEY (word_id, pos, source)
);

-- Etymology source languages (bor = borrowed, inh = inherited, der = derived,
-- lbor = learned borrowing, slbor = semi-learned borrowing, calque).
CREATE TABLE IF NOT EXISTS word_etymology (
    word_id  INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    lang     TEXT    NOT NULL,    -- source language code (e.g. 'fa', 'it')
    relation TEXT    NOT NULL DEFAULT '',
    source   TEXT    NOT NULL,
    PRIMARY KEY (word_id, lang, relation, source)
);

-- WordNet lexname categories per word (ingest/wordnet.py): the "kinds of X"
-- feature, e.g. category='noun.food' / 'noun.animal' / 'noun.person'.
CREATE TABLE IF NOT EXISTS word_category (
    word_id    INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    category   TEXT    NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0,   -- the category of the most common sense
    PRIMARY KEY (word_id, category)
);
CREATE INDEX IF NOT EXISTS idx_word_category_category ON word_category(category);

-- WordNet lemma/synset relations (ingest/wordnet.py): hypernym, holonyms,
-- meronyms, derivation, antonym, ... Target stored as a lemma string (portable;
-- the target may not itself be a coolwords headword).
CREATE TABLE IF NOT EXISTS word_relation (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    rel     TEXT    NOT NULL,
    target  TEXT    NOT NULL,
    source  TEXT    NOT NULL DEFAULT 'wordnet',
    PRIMARY KEY (word_id, rel, target, source)
);
CREATE INDEX IF NOT EXISTS idx_word_relation_word ON word_relation(word_id, rel);

-- Maps a word to its row in the embedding sidecar (data/coolwords_emb.npy,
-- float32 (N, 300), L2-normalized). Kept out of the .db so the large, stable
-- vectors don't bloat / re-sync the frequently-changing database.
CREATE TABLE IF NOT EXISTS word_embedding_map (
    word_id INTEGER PRIMARY KEY REFERENCES words(id) ON DELETE CASCADE,
    row     INTEGER NOT NULL
);

-- Per-year frequency for trajectory over decades (Google ngrams). Reserved.
CREATE TABLE IF NOT EXISTS word_freq_year (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    year    INTEGER NOT NULL,
    count   INTEGER NOT NULL,
    PRIMARY KEY (word_id, year)
);

-- Raw surface-form frequency from Google ngrams, for ANY token (not just
-- curated headwords). Aggregated case-insensitively (sum over case variants),
-- but count_lc / cap_ratio preserve how often the form was capitalized.
CREATE TABLE IF NOT EXISTS ngram_freq (
    token     TEXT    PRIMARY KEY,             -- lowercased surface form
    count     INTEGER NOT NULL,                -- total across all case variants
    count_lc  INTEGER NOT NULL DEFAULT 0,      -- occurrences of the all-lowercase form only
    cap_ratio REAL,                            -- (count - count_lc) / count
    rank      INTEGER,                         -- positional id, count desc, alphabetical tie-break
    pm        REAL                             -- occurrences per million (all-case)
);
CREATE INDEX IF NOT EXISTS idx_ngram_freq_rank ON ngram_freq(rank);

-- Surface-form frequency within the Google fiction sub-corpus (genre baseline).
-- Compared against ngram_freq to spot genre-typical vs genuinely-rare words.
CREATE TABLE IF NOT EXISTS fiction_freq (
    token     TEXT    PRIMARY KEY,
    count     INTEGER NOT NULL,
    count_lc  INTEGER NOT NULL DEFAULT 0,
    cap_ratio REAL,
    rank      INTEGER,
    pm        REAL
);
CREATE INDEX IF NOT EXISTS idx_fiction_freq_rank ON fiction_freq(rank);

-- Append-only record of each ingest run.
CREATE TABLE IF NOT EXISTS ingest_log (
    source TEXT,
    detail TEXT,
    rows   INTEGER,
    ts     TEXT
);
