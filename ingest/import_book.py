"""Drag-drop book importer: inspect a dropped file, then commit it.

Two modes, both printing a single JSON object on stdout (consumed by the Rust web
UI, ui/src/app.rs):

  python -m ingest.import_book --inspect <path>
      Extract text + metadata, segment kept-vs-stripped regions, compute the
      content hash, and report whether an identical book already exists. No writes.

  python -m ingest.import_book --commit <path> --slug <slug> [--title ...] [...]
      Copy <path> into the configured books dir (paths.BOOKS_DIR), ingest the word
      histogram, then run the per-book analysis pipeline (score, cluster,
      trajectory). Refuses if a *different* book already has this content hash.

Keeping all extraction/segmentation/ingest here (not in Rust) means the import
preview, the commit, and any future "view source" page share one code path.
"""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

from ingest import book
from ingest.db import connect
from ingest.extract import extract, ExtractError
from ingest.paths import BOOKS_DIR, PROJECT_ROOT

_PREVIEW_KEPT = 1500       # chars of a kept body span shown in the viewer
_PREVIEW_STRIPPED = 20000  # stripped spans shown ~in full (they're the review surface)


def _content_hash(kept_text: str) -> str:
    """sha256 of the ordered, lowercased token stream — stable across whitespace /
    markup / format differences, so the same work imported twice (even txt vs epub)
    collides for dedup."""
    stream = " ".join(book.TOKEN_RE.findall(kept_text.lower()))
    return hashlib.sha256(stream.encode("utf-8")).hexdigest()


def _slugify(s: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    return s[:60].strip("-")


def _suggest_slug(con, ex, path: Path) -> str:
    if ex.source == "gutenberg" and ex.source_id:
        base = f"gutenberg-{ex.source_id}"
    else:
        base = _slugify(ex.title) or _slugify(Path(path).stem) or "book"
        base = f"import-{base}" if not base.startswith("import-") else base
    existing = {r[0] for r in con.execute("SELECT slug FROM books")}
    if base not in existing:
        return base
    for n in range(2, 1000):
        cand = f"{base}-{n}"
        if cand not in existing:
            return cand
    return base


def _segments_json(ex) -> list[dict]:
    out = []
    for s in ex.segments:
        cap = _PREVIEW_KEPT if s.kept else _PREVIEW_STRIPPED
        body = s.text.strip("\n")
        preview = body[:cap]
        out.append({
            "label": s.label,
            "kept": s.kept,
            "note": s.note,
            "char_len": len(s.text),
            "preview": preview,
            "truncated": len(body) > cap,
        })
    return out


def _dup_lookup(con, content_hash: str, exclude_slug: str = ""):
    row = con.execute(
        "SELECT slug, COALESCE(title, slug) FROM books WHERE content_hash = ? AND slug <> ? LIMIT 1",
        (content_hash, exclude_slug),
    ).fetchone()
    return (row[0], row[1]) if row else (None, None)


def do_inspect(path: Path) -> dict:
    ex = extract(path)
    kept = ex.kept_text
    tokens, _ = book.tokenize(kept)
    chash = _content_hash(kept)
    con = connect()
    dup_slug, dup_title = _dup_lookup(con, chash)
    suggested = _suggest_slug(con, ex, path)
    con.close()
    return {
        "ok": True,
        "format": ex.fmt,
        "title": ex.title,
        "author": ex.author,
        "year": ex.year,
        "year_note": ex.year_note,
        "source": ex.source,
        "source_id": ex.source_id,
        "content_hash": chash,
        "n_tokens": sum(tokens.values()),
        "n_types": len(tokens),
        "duplicate_of": dup_slug,
        "duplicate_title": dup_title,
        "suggested_slug": suggested,
        "orig_filename": Path(path).name,
        "segments": _segments_json(ex),
    }


def _run_step(args: list[str]) -> dict:
    """Run a pipeline module (python -m ingest.X ...) and capture its result."""
    proc = subprocess.run(
        [sys.executable, "-m", *args],
        cwd=str(PROJECT_ROOT), capture_output=True, text=True,
    )
    return {"step": args[0].split(".")[-1], "ok": proc.returncode == 0,
            "stderr": proc.stderr[-500:] if proc.returncode else ""}


def do_commit(path: Path, slug: str, title: str, author: str, year, orig_filename: str,
              run_pipeline: bool) -> dict:
    path = Path(path)
    ex = extract(path)
    kept = ex.kept_text
    tokens, examples = book.tokenize(kept)
    chash = _content_hash(kept)

    con = connect()
    dup_slug, dup_title = _dup_lookup(con, chash, exclude_slug=slug)
    if dup_slug:
        con.close()
        return {"ok": False, "code": "DUPLICATE",
                "error": f"Identical content already imported as '{dup_title}' ({dup_slug})."}

    BOOKS_DIR.mkdir(parents=True, exist_ok=True)
    dest = BOOKS_DIR / f"{slug}{path.suffix.lower()}"
    if Path(path).resolve() != dest.resolve():
        shutil.copyfile(path, dest)

    meta = {
        "title": title or ex.title,
        "author": author or ex.author,
        "source": ex.source or "import",
        "source_id": ex.source_id,
        "year": year,
        "content_hash": chash,
        "format": ex.fmt,
        "orig_filename": orig_filename or path.name,
    }
    book_id, n_tokens, n_types, matched = book.ingest_tokens(con, slug, meta, tokens, examples)
    con.close()

    steps = []
    if run_pipeline:
        steps.append(_run_step(["ingest.score", "--slug", slug]))
        steps.append(_run_step(["ingest.cluster", "--slug", slug]))
        steps.append(_run_step(["ingest.trajectory"]))

    con = connect()
    n_cand = con.execute(
        "SELECT count(*) FROM candidates WHERE book_id = ? AND level = 0", (book_id,)
    ).fetchone()[0]
    con.close()

    return {"ok": True, "slug": slug, "book_id": book_id, "title": meta["title"],
            "n_tokens": n_tokens, "n_types": n_types, "matched": matched,
            "candidates": n_cand, "dest": str(dest), "pipeline": steps}


def main() -> None:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--inspect", metavar="PATH")
    g.add_argument("--commit", metavar="PATH")
    ap.add_argument("--slug", default="")
    ap.add_argument("--title", default="")
    ap.add_argument("--author", default="")
    ap.add_argument("--year", default="")
    ap.add_argument("--orig-filename", default="")
    ap.add_argument("--no-pipeline", action="store_true")
    args = ap.parse_args()

    try:
        if args.inspect:
            result = do_inspect(Path(args.inspect))
        else:
            if not args.slug:
                raise SystemExit("--commit requires --slug")
            year = int(args.year) if re.fullmatch(r"\d{3,4}", args.year.strip()) else None
            result = do_commit(Path(args.commit), args.slug, args.title, args.author,
                               year, args.orig_filename, run_pipeline=not args.no_pipeline)
    except ExtractError as e:
        result = {"ok": False, "code": e.code, "error": str(e)}
    except Exception as e:  # surface any failure as JSON the UI can show
        result = {"ok": False, "code": "ERROR", "error": f"{type(e).__name__}: {e}"}

    # Emit UTF-8 bytes directly: book previews contain characters outside the
    # Windows console's default cp1252, and the Rust caller parses stdout as UTF-8.
    sys.stdout.buffer.write(json.dumps(result, ensure_ascii=False).encode("utf-8"))
    sys.stdout.buffer.write(b"\n")


if __name__ == "__main__":
    main()
