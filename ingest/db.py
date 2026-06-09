"""SQLite connection + schema helpers shared by the ingest scripts."""
from pathlib import Path
import sqlite3

from ingest.paths import DB_PATH, SCHEMA_PATH


# Columns added to `words` after its original definition. CREATE TABLE IF NOT
# EXISTS will not add columns to an existing table, so we ALTER them in on open.
# Listed here as well as in the schema so fresh and existing DBs converge.
_MIGRATIONS: dict[str, list[tuple[str, str]]] = {
    "words": [
        ("fic_count", "INTEGER"),
        ("fic_rank", "INTEGER"),
        ("fic_pm", "REAL"),
        ("freq_pm_lc", "REAL"),
        ("cap_ratio", "REAL"),
        ("etymology_text", "TEXT"),
        ("gloss", "TEXT"),
        ("wordnet_category", "TEXT"),
        ("is_proper", "INTEGER NOT NULL DEFAULT 0"),
        ("is_form_of", "INTEGER NOT NULL DEFAULT 0"),
        ("is_offensive", "INTEGER NOT NULL DEFAULT 0"),
        ("in_fasttext", "INTEGER NOT NULL DEFAULT 0"),
    ],
    "ngram_freq": [
        ("count_lc", "INTEGER NOT NULL DEFAULT 0"),
        ("cap_ratio", "REAL"),
    ],
    "fiction_freq": [
        ("count_lc", "INTEGER NOT NULL DEFAULT 0"),
        ("cap_ratio", "REAL"),
    ],
    "candidates": [
        ("selected", "INTEGER NOT NULL DEFAULT 0"),
    ],
}


def connect(db_path: Path = DB_PATH) -> sqlite3.Connection:
    """Open the dictionary database, creating its directory and schema if needed."""
    db_path = Path(db_path)
    db_path.parent.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(db_path)
    con.execute("PRAGMA journal_mode = WAL")
    con.execute("PRAGMA synchronous = NORMAL")
    con.execute("PRAGMA foreign_keys = ON")
    init_schema(con)
    ensure_columns(con)
    return con


def init_schema(con: sqlite3.Connection, schema_dir: Path = SCHEMA_PATH.parent) -> None:
    """Apply every schema/*.sql file (dictionary, books, ...) in name order."""
    for sql in sorted(Path(schema_dir).glob("*.sql")):
        con.executescript(sql.read_text(encoding="utf-8"))
    con.commit()


def ensure_columns(con: sqlite3.Connection) -> None:
    """Additively migrate columns that postdate a table's original creation."""
    for table, cols in _MIGRATIONS.items():
        existing = {row[1] for row in con.execute(f"PRAGMA table_info({table})")}
        for name, decl in cols:
            if name not in existing:
                con.execute(f"ALTER TABLE {table} ADD COLUMN {name} {decl}")
    con.commit()


def log_ingest(con: sqlite3.Connection, source: str, detail: str, rows: int) -> None:
    con.execute(
        "INSERT INTO ingest_log(source, detail, rows, ts) VALUES (?, ?, ?, datetime('now'))",
        (source, detail, rows),
    )
    con.commit()
