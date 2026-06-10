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
from ingest import extract as extract_mod
from ingest import ocr
from ingest.db import connect
from ingest.extract import extract, ExtractError
from ingest.ocr import OcrError
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
    out = {
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
    if ex.fmt == "pdf":
        pdf = ex.meta.get("pdf", {})
        out["pdf"] = pdf
        out["needs_ocr"] = not pdf.get("has_text_layer", False)
        st = ocr.engines()
        out["ocr"] = {
            "engines": st,
            "default_engine": ocr.default_engine() or "",
            "cached_pages": {
                eng: len(ocr.load_cache(path, eng)["pages"]) for eng in st if st[eng]["available"]
            },
        }
    return out


# ---- OCR comparison (embedded text layer vs our re-OCR), sampled pages ---- #
_SAMPLE_N = 8
_DIFF_CTX = 8          # tokens of context kept on each side of a collapsed eq run


def _parse_pages(spec: str) -> list[int] | None:
    """'1,5,9-12' (1-based, as displayed) -> 0-based page numbers; None if empty."""
    if not spec.strip():
        return None
    out: list[int] = []
    for part in spec.split(","):
        part = part.strip()
        if "-" in part:
            a, _, b = part.partition("-")
            out.extend(range(int(a) - 1, int(b)))
        elif part:
            out.append(int(part) - 1)
    return sorted(set(p for p in out if p >= 0))


def _sample_pages(texts: list[str], n: int = _SAMPLE_N) -> list[int]:
    """Evenly-spread sample of text-bearing pages, skipping the first/last 5%
    (covers, blanks, colophons)."""
    total = len(texts)
    margin = max(1, total // 20) if total > 10 else 0
    candidates = [i for i in range(margin, total - margin)
                  if len(texts[i].split()) >= 15] or list(range(total))
    if len(candidates) <= n:
        return candidates
    step = (len(candidates) - 1) / (n - 1)
    return sorted({candidates[round(i * step)] for i in range(n)})


def _diff_ops(a_tokens: list[str], b_tokens: list[str]) -> list[dict]:
    """Word-level opcodes for the UI: eq (collapsed with gap markers), del (embedded
    only), ins (OCR only), rep (replaced)."""
    import difflib
    ops: list[dict] = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, a_tokens, b_tokens).get_opcodes():
        a, b = a_tokens[i1:i2], b_tokens[j1:j2]
        if tag == "equal":
            if len(a) > 2 * _DIFF_CTX + 4:
                ops.append({"op": "eq", "a": " ".join(a[:_DIFF_CTX]), "b": ""})
                ops.append({"op": "gap", "a": f"… {len(a) - 2 * _DIFF_CTX} words …", "b": ""})
                ops.append({"op": "eq", "a": " ".join(a[-_DIFF_CTX:]), "b": ""})
            else:
                ops.append({"op": "eq", "a": " ".join(a), "b": ""})
        elif tag == "delete":
            ops.append({"op": "del", "a": " ".join(a), "b": ""})
        elif tag == "insert":
            ops.append({"op": "ins", "a": "", "b": " ".join(b)})
        else:
            ops.append({"op": "rep", "a": " ".join(a), "b": " ".join(b)})
    return ops


def do_ocr_compare(path: Path, engine: str, pages_spec: str) -> dict:
    """OCR a sample of pages and diff them against the embedded text layer."""
    import difflib
    path = Path(path)
    embedded = extract_mod.pdf_page_texts(path)
    if not embedded:
        return {"ok": False, "code": "PDF_EMPTY", "error": "PDF has no pages."}
    pages = _parse_pages(pages_spec) or _sample_pages(embedded)
    pages = [p for p in pages if p < len(embedded)]
    ocr_texts, eng, n_new = ocr.ocr_pdf(path, engine or None, pages)

    out_pages = []
    for p in pages:
        a = embedded[p].split()
        b = ocr_texts.get(p, "").split()
        sim = difflib.SequenceMatcher(None, a, b).ratio() if (a or b) else 1.0
        out_pages.append({
            "page": p + 1,
            "sim": round(sim, 4),
            "embedded_words": len(a),
            "ocr_words": len(b),
            "ops": _diff_ops(a, b),
        })
    return {"ok": True, "engine": eng, "newly_ocred": n_new, "pages": out_pages}


def _run_step(args: list[str]) -> dict:
    """Run a pipeline module (python -m ingest.X ...) and capture its result."""
    proc = subprocess.run(
        [sys.executable, "-m", *args],
        cwd=str(PROJECT_ROOT), capture_output=True, text=True,
    )
    return {"step": args[0].split(".")[-1], "ok": proc.returncode == 0,
            "stderr": proc.stderr[-500:] if proc.returncode else ""}


def do_commit(path: Path, slug: str, title: str, author: str, year, orig_filename: str,
              run_pipeline: bool, text_source: str = "", engine: str = "") -> dict:
    path = Path(path)
    # For PDFs the text source is a choice: the embedded layer (default) or our
    # own re-OCR. OCR is cache-aware, so a prior compare/full run is reused.
    src_label = None
    pdf_ocr = None
    if extract_mod.detect_format(path) == "pdf":
        if text_source == "ocr":
            texts, eng, n_new = ocr.ocr_pdf(path, engine or None)
            print(f"commit: OCR source via {eng} ({n_new} pages newly OCRed)", file=sys.stderr)
            pdf_ocr, src_label = texts, f"ocr:{eng}"
        else:
            src_label = "embedded"
    ex = extract(path, pdf_ocr=pdf_ocr)
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
        "text_source": src_label,
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
    g.add_argument("--ocr-compare", metavar="PATH",
                   help="OCR a sample of pages and diff against the embedded text layer")
    ap.add_argument("--slug", default="")
    ap.add_argument("--title", default="")
    ap.add_argument("--author", default="")
    ap.add_argument("--year", default="")
    ap.add_argument("--orig-filename", default="")
    ap.add_argument("--no-pipeline", action="store_true")
    ap.add_argument("--engine", default="", help="OCR engine: tesseract | rapidocr (default: auto)")
    ap.add_argument("--pages", default="", help="pages for --ocr-compare, 1-based: '1,5,9-12'")
    ap.add_argument("--text-source", default="", dest="text_source",
                    help="PDF commit text source: embedded (default) | ocr")
    args = ap.parse_args()

    try:
        if args.inspect:
            result = do_inspect(Path(args.inspect))
        elif args.ocr_compare:
            result = do_ocr_compare(Path(args.ocr_compare), args.engine, args.pages)
        else:
            if not args.slug:
                raise SystemExit("--commit requires --slug")
            year = int(args.year) if re.fullmatch(r"\d{3,4}", args.year.strip()) else None
            result = do_commit(Path(args.commit), args.slug, args.title, args.author,
                               year, args.orig_filename, run_pipeline=not args.no_pipeline,
                               text_source=args.text_source, engine=args.engine)
    except (ExtractError, OcrError) as e:
        result = {"ok": False, "code": e.code, "error": str(e)}
    except Exception as e:  # surface any failure as JSON the UI can show
        result = {"ok": False, "code": "ERROR", "error": f"{type(e).__name__}: {e}"}

    # Emit UTF-8 bytes directly: book previews contain characters outside the
    # Windows console's default cp1252, and the Rust caller parses stdout as UTF-8.
    sys.stdout.buffer.write(json.dumps(result, ensure_ascii=False).encode("utf-8"))
    sys.stdout.buffer.write(b"\n")


if __name__ == "__main__":
    main()
