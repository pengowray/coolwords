-- Per-user data: the tag collection and tag applications. Lives in its OWN
-- self-contained database (data/user.db, overridable via COOLWORDS_USER_DB) so
-- it is never touched by dictionary/book rebuilds. Keyed by STABLE TEXT
-- (book slug + lowercased headword), so tags survive re-imports, dictionary
-- rebuilds, and even deleting coolwords.db. NOT applied to coolwords.db
-- (ingest/db.py skips this file; the Leptos UI applies it to the user DB).

-- The user's tag collection (definitions): builtin defaults + custom tags.
-- `scope`    : 'book' (this book only) | 'word' (the word across ALL books).
-- `interest` : 'interesting' (favourites the word) | 'neutral' (a note/descriptor,
--              no fav) | 'uninteresting' (negative: junk / proper-noun / not-a-word).
-- Validated in the UI server fns; new columns are ALTERed onto existing DBs by
-- ingest/userdb.py and the Leptos open_user() (CREATE IF NOT EXISTS won't add them).
-- Hierarchy is a naming convention: a '.' in the name nests it under its prefix
-- (`thing.material` is a child of `thing`); a child implies its parent for
-- favourites/filters (derived at read time, never stored). `kind`/`scale_max`/
-- `scale_labels` make a tag a 1..N scale instead of a plain on/off boolean.
CREATE TABLE IF NOT EXISTS tags (
    name     TEXT PRIMARY KEY,           -- 'star', 'useful', 'ship.jargon', ...
    comment  TEXT,                       -- what this tag is for (free text)
    builtin  INTEGER NOT NULL DEFAULT 0, -- 1 for the seeded defaults below
    sort     INTEGER NOT NULL DEFAULT 100,
    created  TEXT,
    scope    TEXT NOT NULL DEFAULT 'book',
    interest TEXT NOT NULL DEFAULT 'interesting',
    section  TEXT NOT NULL DEFAULT '',        -- user subheading within a scope ('' = ungrouped)
    kind        TEXT NOT NULL DEFAULT 'bool', -- 'bool' (on/off) | 'scale' (1..scale_max)
    scale_max   INTEGER NOT NULL DEFAULT 1,   -- bool == 1; a scale is 2..10
    scale_labels TEXT                         -- nullable JSON array of level names (optional)
);

-- Tag applications. No foreign keys into the dictionary — fully self-contained.
-- `value` is the tri-state rating: NULL (legacy rows) == 1 == applied; an explicit
-- 0 means "considered and deliberately declined" (remembered, but NOT applied);
-- >=1 means applied (the magnitude, for scale tags). Absent row = never considered.
CREATE TABLE IF NOT EXISTS word_tags (
    book_slug TEXT NOT NULL,             -- e.g. 'gutenberg-2701'
    word      TEXT NOT NULL,             -- lowercased headword
    tag       TEXT NOT NULL,             -- references tags.name (by value; or 'pick:<bucket>')
    rater     TEXT NOT NULL DEFAULT 'me',
    ts        TEXT,
    value     INTEGER,                   -- NULL==1==applied; 0==considered-declined; >=1 applied
    PRIMARY KEY (book_slug, word, tag, rater)
);
CREATE INDEX IF NOT EXISTS idx_user_word_tags ON word_tags(book_slug, word);

-- Builtin defaults (the old hardcoded set). INSERT OR IGNORE so re-running and
-- user edits to comments are preserved. 'star' is the quick ★ toggle.
INSERT OR IGNORE INTO tags(name, comment, builtin, sort, created) VALUES
    ('star',        'quick favourite / uncategorized keep', 1, 0, datetime('now')),
    ('useful',      'handy — would actually use this word',  1, 1, datetime('now')),
    ('strange',     'odd, surprising, or unexpected',        1, 2, datetime('now')),
    ('interesting', 'noteworthy for some reason',            1, 3, datetime('now')),
    ('aesthetic',   'pleasing to say or look at',            1, 4, datetime('now')),
    ('emblematic',  'captures the flavour of this book',     1, 5, datetime('now'));
