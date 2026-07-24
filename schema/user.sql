-- Per-user data: the tag collection, word-tag applications, and book tags.
-- Lives in its OWN self-contained database (data/user.db, overridable via
-- COOLWORDS_USER_DB) so it is never touched by dictionary/book rebuilds. Keyed
-- by STABLE TEXT (book slug, plus the lowercased headword for word tags), so
-- tags survive re-imports, dictionary rebuilds, and even deleting
-- coolwords.db. NOT applied to coolwords.db
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
    scale_labels TEXT,                        -- nullable JSON array of level names (optional)
    fav         INTEGER NOT NULL DEFAULT 0    -- 1 = pinned for quick access in the verbarium
);

-- Tag applications. No foreign keys into the dictionary — fully self-contained.
-- `value` encodes the state: NULL == the value-less "on" (applied, no numeric level
-- — the plain true state, and what a bool tag always is); 0 == "considered and
-- deliberately declined" (remembered, but NOT applied); >=1 == applied at that scale
-- level. Absent row = never considered. The old tap-on default was value=1; a
-- one-time migration (see migrate_user) rewrote those to NULL, so a `1` here is now a
-- deliberate level-1 rating, not "on". `COALESCE(value,1) >= 1` still means "applied"
-- (NULL/on and every level count; only the 0 row is excluded).
CREATE TABLE IF NOT EXISTS word_tags (
    book_slug TEXT NOT NULL,             -- e.g. 'gutenberg-2701'
    word      TEXT NOT NULL,             -- lowercased headword
    tag       TEXT NOT NULL,             -- references tags.name (by value; or 'pick:<bucket>')
    rater     TEXT NOT NULL DEFAULT 'me',
    ts        TEXT,
    value     INTEGER,                   -- NULL==on(value-less); 0==considered-declined; >=2==level
    PRIMARY KEY (book_slug, word, tag, rater)
);
CREATE INDEX IF NOT EXISTS idx_user_word_tags ON word_tags(book_slug, word);

-- Tags on BOOKS (not words): 'fiction', 'poetry', 'to-read', plus derived ones
-- like 'src.gutenberg' / 'fmt.epub'. Same contract as word_tags — keyed by the
-- book's slug, no foreign key into coolwords.db — so a dictionary rebuild, a
-- re-import, or deleting coolwords.db entirely leaves the tags intact. Deleting a
-- book does NOT delete its tags; they lie dormant and reattach if the same slug
-- comes back (deliberate: re-importing a book you'd already curated should not
-- silently lose that work).
--
-- `auto` marks rows this app maintains rather than the user: they are reconciled
-- from books.source / books.format (so a Standard Ebooks epub gets
-- 'src.standardebooks' + 'fmt.epub' for free), and the reconciler is free to
-- delete an auto row that no longer matches. It must never touch auto=0 rows —
-- that's the user's own tagging, even if the name collides with a derived one.
--
-- `tag` is normalised on write: lowercased, restricted to [a-z0-9.:-], with '.'
-- carrying the same hierarchy convention as the word tags ('src.gutenberg' nests
-- under 'src') and ':' available for key:value tags.
CREATE TABLE IF NOT EXISTS book_tags (
    book_slug TEXT NOT NULL,
    tag       TEXT NOT NULL,              -- normalised: lowercase, [a-z0-9.:-]
    auto      INTEGER NOT NULL DEFAULT 0, -- 1 = derived from the book's source/format, not user-set
    ts        TEXT,
    PRIMARY KEY (book_slug, tag)
);
-- The tag-first lookup ("which books are tagged 'poetry'?") has no other index.
CREATE INDEX IF NOT EXISTS idx_book_tags_tag ON book_tags(tag);

-- Builtin defaults (the old hardcoded set). INSERT OR IGNORE so re-running and
-- user edits to comments are preserved. 'star' is the quick ★ toggle.
INSERT OR IGNORE INTO tags(name, comment, builtin, sort, created) VALUES
    ('star',        'quick favourite / uncategorized keep', 1, 0, datetime('now')),
    ('useful',      'handy — would actually use this word',  1, 1, datetime('now')),
    ('strange',     'odd, surprising, or unexpected',        1, 2, datetime('now')),
    ('interesting', 'noteworthy for some reason',            1, 3, datetime('now')),
    ('aesthetic',   'pleasing to say or look at',            1, 4, datetime('now')),
    ('emblematic',  'captures the flavour of this book',     1, 5, datetime('now'));
