-- A local mirror of remote book catalogs (Project Gutenberg, Standard Ebooks),
-- so the "find a book to import" UI can search ~75k titles instantly without
-- touching the network. Populated by `python -m ingest.catalog --sync all`.
--
-- This is REFERENCE data — fully rebuildable from a re-sync — so it lives in
-- coolwords.db alongside the dictionary, not in user.db. ingest/db.py:init_schema
-- applies every schema/*.sql in NAME order, which puts this file between
-- books.sql and dictionary.sql; nothing here may reference `books` or `words`
-- (and nothing does — the catalog is deliberately standalone, joined to `books`
-- only at query time on (source, source_id)).

-- One row per remotely-available book we know about. The primary key is the
-- upstream identity, which is also what `books.source` / `books.source_id` record
-- once a title is imported — that pair is the join that greys out "already in".
CREATE TABLE IF NOT EXISTS catalog_books (
    source       TEXT NOT NULL,      -- 'gutenberg' | 'standardebooks'
    source_id    TEXT NOT NULL,      -- '2701' | 'charles-dickens/the-mystery-of-edwin-drood'
    title        TEXT,
    author       TEXT,               -- normalised to "Given Surname"; '; '-joined if several
    year         INTEGER,            -- publication year when known; NULL otherwise.
                                     -- NOT Gutenberg's `Issued` (that's the PG release
                                     -- date, which would date Moby-Dick to 2001).
    language     TEXT,               -- BCP-47-ish code as upstream reports it ('en')
    subjects     TEXT,               -- '; '-joined (PG subject headings / SE tags)
    n_words      INTEGER,            -- Standard Ebooks reports it; NULL for Gutenberg
    reading_ease REAL,               -- Standard Ebooks only (Flesch); NULL otherwise
    fmt          TEXT,               -- 'epub' | 'txt' — the format `url` points at
    url          TEXT,               -- resolved download URL (a MIRROR for Gutenberg;
                                     -- see ingest/catalog.py for why we never hit
                                     -- www.gutenberg.org for the file itself)
    synced_at    TEXT,               -- when this row was last refreshed
    PRIMARY KEY (source, source_id)
);

-- Search paths. A leading-wildcard LIKE can't use an index, but prefix matches
-- and the "browse by source, ordered by title/author" listings can.
CREATE INDEX IF NOT EXISTS idx_catalog_title       ON catalog_books(title);
CREATE INDEX IF NOT EXISTS idx_catalog_author      ON catalog_books(author);
CREATE INDEX IF NOT EXISTS idx_catalog_title_lc    ON catalog_books(lower(title));
CREATE INDEX IF NOT EXISTS idx_catalog_source_sort ON catalog_books(source, title);

-- Catalog freshness, one row per source, so the UI can say "Gutenberg: 75,431
-- titles, synced 3 days ago" and offer a re-sync. A sync is an explicit user
-- action — we never re-fetch a catalog page as a side effect of searching.
CREATE TABLE IF NOT EXISTS catalog_sync (
    source    TEXT PRIMARY KEY,      -- 'gutenberg' | 'standardebooks'
    synced_at TEXT,
    n_rows    INTEGER                -- rows written by that sync (not a running total)
);
