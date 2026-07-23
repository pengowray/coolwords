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


def book_file(slug: str) -> Path:
    """The committed source file for a slug: BOOKS_DIR/<slug>.<ext>."""
    for ext in (".pdf", ".epub", ".txt"):
        p = BOOKS_DIR / f"{slug}{ext}"
        if p.exists():
            return p
    raise ExtractError(f"no stored file for '{slug}' in {BOOKS_DIR}", code="NO_FILE")


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
              run_pipeline: bool, source: str = "", source_id: str = "") -> dict:
    """Copy a file into BOOKS_DIR, ingest its histogram, optionally run the pipeline.

    `source` / `source_id` override whatever the extractor sniffed out of the file.
    That matters for catalogue downloads (ingest/catalog.py): a Standard Ebooks
    epub only announces itself as a generic 'epub', but we know it came from
    standardebooks.org with a canonical id, and recording that is what lets the
    catalogue search grey out books we already have."""
    path = Path(path)
    # Import is always fast: PDFs use their embedded text layer (a scan imports as a
    # near-empty placeholder). Re-OCR + source switching happen later on the manage
    # page (ingest.import_book --ocr-book / --reingest), so the browser never blocks.
    ex = extract(path)
    src_label = "embedded" if ex.fmt == "pdf" else None
    kept = ex.kept_text
    tokens, examples = book.tokenize(kept)
    chash = _content_hash(kept)

    con = connect()
    dup_slug, dup_title = _dup_lookup(con, chash, exclude_slug=slug)
    if dup_slug:
        con.close()
        # slug/title as fields, not just prose: ingest/catalog.py's bulk grab needs
        # to know WHICH book we collided with, so it can stamp the catalog identity
        # onto a hand-imported row and stop re-downloading it on every future grab.
        return {"ok": False, "code": "DUPLICATE", "slug": dup_slug, "title": dup_title,
                "error": f"Identical content already imported as '{dup_title}' ({dup_slug})."}

    BOOKS_DIR.mkdir(parents=True, exist_ok=True)
    dest = BOOKS_DIR / f"{slug}{path.suffix.lower()}"
    if Path(path).resolve() != dest.resolve():
        shutil.copyfile(path, dest)

    meta = {
        "title": title or ex.title,
        "author": author or ex.author,
        "source": source or ex.source or "import",
        "source_id": source_id or ex.source_id,
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


# ---- post-import OCR / source management (operate on the committed file) ---- #
def do_ocr_book(slug: str, engine: str) -> dict:
    """OCR every page of a committed PDF into its book-keyed cache (resumable).
    Prints `ocr[E] page i (i/N)` to stderr for the background-job progress bar."""
    path = book_file(slug)
    if extract_mod.detect_format(path) != "pdf":
        return {"ok": False, "code": "NOT_PDF", "error": f"'{slug}' is not a PDF."}
    texts, eng, n_new = ocr.ocr_pdf(path, engine or None)
    return {"ok": True, "slug": slug, "engine": eng,
            "pages": len(texts), "newly_ocred": n_new}


def do_ocr_status(slug: str) -> dict:
    """Per-engine OCR cache state + embedded-layer state + current text source."""
    path = book_file(slug)
    con = connect()
    row = con.execute("SELECT format, text_source FROM books WHERE slug = ?", (slug,)).fetchone()
    con.close()
    out = {"ok": True, "slug": slug, "format": row[0] if row else None,
           "text_source": row[1] if row else None,
           "is_pdf": extract_mod.detect_format(path) == "pdf"}
    if not out["is_pdf"]:
        return out
    embedded = extract_mod.pdf_page_texts(path)
    n_pages = len(embedded)
    n_text = sum(1 for t in embedded if len(t.split()) >= 15)
    st = ocr.engines()
    out.update({
        "n_pages": n_pages,
        "n_text_pages": n_text,
        "has_text_layer": n_text * 2 > n_pages,
        "default_engine": ocr.default_engine() or "",
        "engines": {
            eng: {
                "available": info["available"],
                "detail": info["detail"],
                "cached_pages": len(ocr.load_cache(path, eng)["pages"]),
                "complete": n_pages > 0 and len(ocr.load_cache(path, eng)["pages"]) >= n_pages,
            }
            for eng, info in st.items()
        },
    })
    return out


def do_reingest(slug: str, text_source: str, run_pipeline: bool = True) -> dict:
    """Re-tokenize a committed book from a chosen text source (embedded, or a cached
    OCR engine — no re-OCR), replace its histogram, and re-score. Preserves the
    book's metadata + tags (tags are keyed by slug+word in user.db)."""
    path = book_file(slug)
    con = connect()
    row = con.execute(
        "SELECT title, author, source, source_id, year, format, orig_filename "
        "FROM books WHERE slug = ?", (slug,)
    ).fetchone()
    if not row:
        con.close()
        return {"ok": False, "code": "NO_BOOK", "error": f"no book '{slug}'"}
    title, author, source, source_id, year, fmt, orig = row

    pdf_ocr = None
    src_label = "embedded"
    if text_source.startswith("ocr"):
        eng = ocr.require_engine(text_source.split(":", 1)[1] if ":" in text_source else None)
        cache = ocr.load_cache(path, eng)
        embedded = extract_mod.pdf_page_texts(path)
        if len(cache["pages"]) < len(embedded):
            con.close()
            return {"ok": False, "code": "OCR_INCOMPLETE",
                    "error": f"OCR cache for {eng} is incomplete "
                             f"({len(cache['pages'])}/{len(embedded)} pages) — run OCR first."}
        pdf_ocr = {int(k): v for k, v in cache["pages"].items()}
        src_label = f"ocr:{eng}"

    print("reingest: extract", file=sys.stderr, flush=True)
    ex = extract(path, pdf_ocr=pdf_ocr)
    tokens, examples = book.tokenize(ex.kept_text)
    chash = _content_hash(ex.kept_text)
    dup_slug, dup_title = _dup_lookup(con, chash, exclude_slug=slug)
    if dup_slug:
        con.close()
        return {"ok": False, "code": "DUPLICATE",
                "error": f"This text matches another book, '{dup_title}' ({dup_slug})."}
    meta = {"title": title, "author": author, "source": source, "source_id": source_id,
            "year": year, "content_hash": chash, "format": fmt, "orig_filename": orig,
            "text_source": src_label}
    print("reingest: ingest", file=sys.stderr, flush=True)
    book_id, n_tokens, n_types, matched = book.ingest_tokens(con, slug, meta, tokens, examples)
    con.close()

    steps = []
    if run_pipeline:
        print("reingest: score", file=sys.stderr, flush=True)
        steps.append(_run_step(["ingest.score", "--slug", slug]))
        print("reingest: cluster", file=sys.stderr, flush=True)
        steps.append(_run_step(["ingest.cluster", "--slug", slug]))
    con = connect()
    n_cand = con.execute(
        "SELECT count(*) FROM candidates WHERE book_id = ? AND level = 0", (book_id,)
    ).fetchone()[0]
    con.close()
    return {"ok": True, "slug": slug, "book_id": book_id, "text_source": src_label,
            "n_tokens": n_tokens, "n_types": n_types, "candidates": n_cand, "pipeline": steps}


def do_rescore(slug: str) -> dict:
    """Re-run score + cluster for an already-imported book (no re-extraction).

    The bulk catalogue grab (ingest/catalog.py --grab) imports with
    run_pipeline=False so the download batch stays fast; the Rust job queue then
    runs this once per new book, one at a time under the job semaphore. Also the
    right thing to run after a scoring-parameter change. Deliberately does NOT
    touch ingest.trajectory — that pass is global and slow, so it's a separate
    --refresh-trajectory job run once at the end rather than per book."""
    con = connect()
    row = con.execute("SELECT id FROM books WHERE slug = ?", (slug,)).fetchone()
    con.close()
    if not row:
        return {"ok": False, "code": "NO_BOOK", "error": f"no book '{slug}'"}
    book_id = row[0]

    steps = []
    print("rescore: score", file=sys.stderr, flush=True)
    steps.append(_run_step(["ingest.score", "--slug", slug]))
    print("rescore: cluster", file=sys.stderr, flush=True)
    steps.append(_run_step(["ingest.cluster", "--slug", slug]))

    con = connect()
    n_cand = con.execute(
        "SELECT count(*) FROM candidates WHERE book_id = ? AND level = 0", (book_id,)
    ).fetchone()[0]
    con.close()
    return {"ok": all(s["ok"] for s in steps), "slug": slug, "book_id": book_id,
            "candidates": n_cand, "pipeline": steps}


def do_refresh_trajectory() -> dict:
    """Re-run the global per-decade usage pass (covers any new words after a source
    switch). Slow — streams the ngram shards; run as a background job."""
    print("trajectory: refreshing usage charts", file=sys.stderr, flush=True)
    step = _run_step(["ingest.trajectory"])
    return {"ok": step["ok"], "step": step}


def main() -> None:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--inspect", metavar="PATH")
    g.add_argument("--commit", metavar="PATH")
    g.add_argument("--ocr-compare", metavar="PATH",
                   help="OCR a sample of pages and diff against the embedded text layer")
    g.add_argument("--ocr-book", metavar="SLUG", dest="ocr_book",
                   help="OCR a committed PDF into its book-keyed cache (background job)")
    g.add_argument("--reingest", metavar="SLUG",
                   help="re-tokenize a committed book from a chosen --text-source")
    g.add_argument("--ocr-status", metavar="SLUG", dest="ocr_status",
                   help="report OCR cache + text-source state for a committed book")
    g.add_argument("--rescore", metavar="SLUG",
                   help="re-run score + cluster for an already-imported book (background job)")
    g.add_argument("--refresh-trajectory", action="store_true", dest="refresh_trajectory",
                   help="re-run the global usage-over-time pass (background job)")
    ap.add_argument("--slug", default="")
    ap.add_argument("--title", default="")
    ap.add_argument("--author", default="")
    ap.add_argument("--year", default="")
    ap.add_argument("--orig-filename", default="")
    ap.add_argument("--source", default="",
                    help="--commit: override the sniffed source ('gutenberg', 'standardebooks')")
    ap.add_argument("--source-id", default="", dest="source_id",
                    help="--commit: override the sniffed source id")
    ap.add_argument("--no-pipeline", action="store_true")
    ap.add_argument("--engine", default="", help="OCR engine: tesseract | rapidocr (default: auto)")
    ap.add_argument("--pages", default="", help="pages for --ocr-compare, 1-based: '1,5,9-12'")
    ap.add_argument("--text-source", default="", dest="text_source",
                    help="--reingest source: embedded | ocr:<engine>")
    args = ap.parse_args()

    try:
        if args.inspect:
            result = do_inspect(Path(args.inspect))
        elif args.ocr_compare:
            result = do_ocr_compare(Path(args.ocr_compare), args.engine, args.pages)
        elif args.ocr_book:
            result = do_ocr_book(args.ocr_book, args.engine)
        elif args.ocr_status:
            result = do_ocr_status(args.ocr_status)
        elif args.reingest:
            if not args.text_source:
                raise SystemExit("--reingest requires --text-source embedded|ocr:<engine>")
            result = do_reingest(args.reingest, args.text_source, run_pipeline=not args.no_pipeline)
        elif args.rescore:
            result = do_rescore(args.rescore)
        elif args.refresh_trajectory:
            result = do_refresh_trajectory()
        else:
            if not args.slug:
                raise SystemExit("--commit requires --slug")
            year = int(args.year) if re.fullmatch(r"\d{3,4}", args.year.strip()) else None
            result = do_commit(Path(args.commit), args.slug, args.title, args.author,
                               year, args.orig_filename, run_pipeline=not args.no_pipeline,
                               source=args.source, source_id=args.source_id)
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
