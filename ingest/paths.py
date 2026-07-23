"""Filesystem locations for datasets and build artifacts.

Edit DATASETS if the downloaded datasets move. Everything else is derived.

A repo-root `.env` (gitignored) can override a few user-specific paths without
touching code; see `load_dotenv` / `env` below. Today's knobs are
`COOLWORDS_BOOKS_DIR` (where the import UI copies dropped books) and
`COOLWORDS_GUTENBERG_MIRROR` (which mirror ingest/catalog.py downloads from).
"""
import os
from pathlib import Path

# Root of the downloaded reference datasets.
DATASETS = Path(r"P:\datasets-downloaded")

CMUDICT = DATASETS / "datasets-CMU-dict-sphinx" / "cmusphinx" / "cmudict-0.7b"

NGRAM_DIR = DATASETS / "datasets-ngrams-1grams"
# The "top 1 million words" English corpus (2009): glob of zipped TSV shards.
NGRAM_1M_GLOB = "googlebooks-eng-1M-1gram-20090715-*.csv.zip"
NGRAM_1M_TOTALCOUNTS = NGRAM_DIR / "googlebooks-eng-1M-totalcounts-20090715.txt"

WORDLE_DIR = DATASETS / "datasets-wordle"

WORDNET_SQLITE = DATASETS / "datasets-wordnet" / "sqlite-5.2.0-31-all" / "Sqlunet.db"

_WIKT = DATASETS / "datasets-wikt-wiktextract" / "2023-11-01"
WIKTEXTRACT = _WIKT / "raw-wiktextract-data.json.gz"
WIKTEXTRACT_SAMPLE = _WIKT / "raw-wiktextract-data-1000lines-sample.json"

WORDVEC_DIR = DATASETS / "datasets-wordvec"
FASTTEXT_VEC_ZIP = WORDVEC_DIR / "crawl-300d-2M.vec.zip"
GOOGLENEWS_BIN_GZ = WORDVEC_DIR / "GoogleNews-vectors-negative300.bin.gz"

# Build artifacts (relative to the project root). The two DB paths honour the same
# env overrides the Rust UI reads (COOLWORDS_DB / COOLWORDS_USER_DB), so a packaged
# deploy (e.g. the Home Assistant add-on) can point both sides at /share without a
# repo-rooted data dir. The embedding sidecar is resolved as DB_PATH.parent/…npy by
# its consumers, so it rides along with whatever COOLWORDS_DB points at.
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DB_PATH = Path(os.environ.get("COOLWORDS_DB") or PROJECT_ROOT / "data" / "coolwords.db")
# Per-user data (tags + tag collection), separate & self-contained.
USER_DB_PATH = Path(os.environ.get("COOLWORDS_USER_DB") or PROJECT_ROOT / "data" / "user.db")
SCHEMA_PATH = PROJECT_ROOT / "schema" / "dictionary.sql"

ENV_PATH = PROJECT_ROOT / ".env"


def load_dotenv(path: Path = ENV_PATH) -> dict[str, str]:
    """Parse a minimal `KEY=VALUE` .env file (no third-party dep).

    Blank lines and `#` comments are skipped; surrounding quotes on the value are
    stripped; a leading `export ` is tolerated. Returns the parsed mapping (also
    handy for callers); does NOT mutate os.environ."""
    out: dict[str, str] = {}
    if not path.exists():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        if line.startswith("export "):
            line = line[len("export "):]
        key, _, val = line.partition("=")
        key, val = key.strip(), val.strip()
        if len(val) >= 2 and val[0] == val[-1] and val[0] in "\"'":
            val = val[1:-1]
        if key:
            out[key] = val
    return out


def env(name: str, default: str = "") -> str:
    """A config value, with precedence: real environment > repo `.env` > default."""
    if name in os.environ:
        return os.environ[name]
    return load_dotenv().get(name, default)


# Where the import UI copies dropped books. Override via COOLWORDS_BOOKS_DIR
# (real env var or repo `.env`); defaults to a location outside the repo so the
# (potentially large) imported corpus isn't mixed into version control.
BOOKS_DIR = Path(env("COOLWORDS_BOOKS_DIR", r"D:\datasets\coolwords\books"))

# Scratch space for files on their way in: browser uploads land here before
# --inspect / --commit, and ingest/catalog.py downloads here before importing.
# Must stay in lockstep with the Rust staging_dir() in ui/src/app.rs, which is
# likewise books_dir().join(".staging").
STAGING_DIR = BOOKS_DIR / ".staging"

# Project Gutenberg's robot policy forbids automated crawling of www.gutenberg.org
# pages but explicitly sanctions downloading from a mirror. aleph is PG's own
# master mirror — the server every other mirror pulls from — and serves the
# generated cache/epub files over plain HTTP (its certificate is issued for a
# different hostname, and these are public-domain books, so there is nothing to
# protect in transit). The obvious alternative, https://gutenberg.pglaf.org, had
# an EXPIRED TLS certificate when this was last checked; ingest/catalog.py keeps
# it and https://gutenberg.nabasny.com as automatic fallbacks either way.
# Override per-machine to pick a closer mirror — see
# https://www.gutenberg.org/MIRRORS.ALL for the full list.
GUTENBERG_MIRROR = env("COOLWORDS_GUTENBERG_MIRROR", "http://aleph.gutenberg.org").rstrip("/")
