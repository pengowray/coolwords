"""Filesystem locations for datasets and build artifacts.

Edit DATASETS if the downloaded datasets move. Everything else is derived.
"""
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

# Build artifacts (relative to the project root).
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DB_PATH = PROJECT_ROOT / "data" / "coolwords.db"
# Per-user data (tags + tag collection), separate & self-contained. The UI may
# override this with the COOLWORDS_USER_DB env var.
USER_DB_PATH = PROJECT_ROOT / "data" / "user.db"
SCHEMA_PATH = PROJECT_ROOT / "schema" / "dictionary.sql"
