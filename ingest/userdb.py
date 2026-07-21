"""Create / migrate the per-user database (data/user.db).

    python -m ingest.userdb

User data (the tag collection + tag applications) lives in its own self-contained
SQLite file, keyed by stable text (book slug + lowercased headword) so it is
untouched by — and survives — dictionary/book rebuilds. This script:
  1. creates user.db and applies schema/user.sql (tables + builtin tag seed),
  2. one-time migrates any legacy coolwords.db.word_tags (id-keyed) into it by
     resolving book_id -> slug and word_id -> word, then drops the legacy table.
Idempotent and re-runnable.
"""
import sqlite3

from ingest.paths import DB_PATH, SCHEMA_PATH, USER_DB_PATH

USER_SCHEMA = SCHEMA_PATH.parent / "user.sql"


def migrate_tag_columns(user: sqlite3.Connection) -> None:
    """Additively add columns that postdate the original `tags`/`word_tags` tables
    (CREATE TABLE IF NOT EXISTS won't add them to an existing DB). Mirrors the Rust
    migrate_user()."""
    have = {r[1] for r in user.execute("PRAGMA table_info(tags)")}
    for name, decl in (("scope", "TEXT NOT NULL DEFAULT 'book'"),
                       ("interest", "TEXT NOT NULL DEFAULT 'interesting'"),
                       ("section", "TEXT NOT NULL DEFAULT ''"),
                       ("kind", "TEXT NOT NULL DEFAULT 'bool'"),
                       ("scale_max", "INTEGER NOT NULL DEFAULT 1"),
                       ("scale_labels", "TEXT"),
                       ("fav", "INTEGER NOT NULL DEFAULT 0")):
        if name not in have:
            user.execute(f"ALTER TABLE tags ADD COLUMN {name} {decl}")
    # word_tags.value: the tri-state rating (NULL==1==applied; 0==considered-declined).
    have_apps = {r[1] for r in user.execute("PRAGMA table_info(word_tags)")}
    if "value" not in have_apps:
        user.execute("ALTER TABLE word_tags ADD COLUMN value INTEGER")
    user.commit()


def main() -> None:
    USER_DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    user = sqlite3.connect(USER_DB_PATH)
    user.executescript(USER_SCHEMA.read_text(encoding="utf-8"))
    migrate_tag_columns(user)
    user.commit()

    migrated = 0
    if DB_PATH.exists():
        dic = sqlite3.connect(DB_PATH)
        legacy = dic.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='word_tags'"
        ).fetchone()
        if legacy:
            rows = dic.execute(
                """SELECT b.slug, w.word, t.tag, COALESCE(t.rater,'me'), t.ts
                   FROM word_tags t
                   JOIN books b ON b.id = t.book_id
                   JOIN words w ON w.id = t.word_id"""
            ).fetchall()
            user.executemany(
                "INSERT OR IGNORE INTO word_tags(book_slug, word, tag, rater, ts) VALUES (?, ?, ?, ?, ?)",
                rows,
            )
            # register any custom (non-builtin, non-pick) applied tags into the collection
            user.execute(
                """INSERT OR IGNORE INTO tags(name, builtin, sort, created)
                   SELECT DISTINCT tag, 0, 100, datetime('now') FROM word_tags
                   WHERE tag NOT LIKE 'pick:%' AND tag NOT IN (SELECT name FROM tags)"""
            )
            user.commit()
            migrated = len(rows)
            dic.execute("DROP TABLE word_tags")
            dic.commit()
        dic.close()

    n_tags = user.execute("SELECT count(*) FROM tags").fetchone()[0]
    n_apps = user.execute("SELECT count(*) FROM word_tags").fetchone()[0]
    user.close()
    print(f"userdb: {USER_DB_PATH} — {n_tags} tags in collection, {n_apps} applications "
          f"({migrated} migrated from coolwords.db)")


if __name__ == "__main__":
    main()
