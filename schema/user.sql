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
CREATE TABLE IF NOT EXISTS tags (
    name     TEXT PRIMARY KEY,           -- 'star', 'useful', 'ship-jargon', ...
    comment  TEXT,                       -- what this tag is for (free text)
    builtin  INTEGER NOT NULL DEFAULT 0, -- 1 for the seeded defaults below
    sort     INTEGER NOT NULL DEFAULT 100,
    created  TEXT,
    scope    TEXT NOT NULL DEFAULT 'book',
    interest TEXT NOT NULL DEFAULT 'interesting',
    section  TEXT NOT NULL DEFAULT ''         -- user subheading within a scope ('' = ungrouped)
);

-- Tag applications. No foreign keys into the dictionary — fully self-contained.
CREATE TABLE IF NOT EXISTS word_tags (
    book_slug TEXT NOT NULL,             -- e.g. 'gutenberg-2701'
    word      TEXT NOT NULL,             -- lowercased headword
    tag       TEXT NOT NULL,             -- references tags.name (by value; or 'pick:<bucket>')
    rater     TEXT NOT NULL DEFAULT 'me',
    ts        TEXT,
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
