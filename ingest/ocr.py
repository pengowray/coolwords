"""OCR for PDF pages: switchable engines + a per-engine sidecar cache.

Engines (both optional; each reports how to install itself if missing):
  tesseract  — the system binary, called directly via subprocess. Located via the
               COOLWORDS_TESSERACT .env/env knob, then PATH, then common dirs.
  rapidocr   — pip-only (rapidocr-onnxruntime): PaddleOCR models on onnxruntime.

The engine is chosen per call, else COOLWORDS_OCR_ENGINE (.env), else whichever
is available (tesseract preferred). OCR results are cached in a JSON sidecar next
to the PDF — `<file>.ocr.<engine>.json`, page→text — so a sample compare, a full
re-OCR, and a commit never re-OCR the same page, and engines never clobber each
other's results.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from ingest.paths import env

DPI = 300
_TESS_HINT = "winget install UB-Mannheim.TesseractOCR  (or set COOLWORDS_TESSERACT in .env)"
_RAPID_HINT = "pip install rapidocr-onnxruntime"


class OcrError(Exception):
    def __init__(self, message: str, code: str = "OCR_ERROR"):
        super().__init__(message)
        self.code = code


# --------------------------------------------------------------------------- #
#  engine discovery                                                           #
# --------------------------------------------------------------------------- #
def find_tesseract() -> str | None:
    cand = env("COOLWORDS_TESSERACT")
    if cand and Path(cand).exists():
        return cand
    if hit := shutil.which("tesseract"):
        return hit
    for p in (r"C:\Program Files\Tesseract-OCR\tesseract.exe",
              r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe",
              # winget UB-Mannheim user-scope install
              str(Path.home() / r"AppData\Local\Programs\Tesseract-OCR\tesseract.exe")):
        if Path(p).exists():
            return p
    return None


_rapid_instance = None


def _rapidocr_class():
    try:
        from rapidocr_onnxruntime import RapidOCR  # classic package name
        return RapidOCR
    except ImportError:
        pass
    try:
        from rapidocr import RapidOCR  # newer unified package
        return RapidOCR
    except ImportError:
        return None


def engines() -> dict:
    """Status of every known engine, for the UI / CLI to display."""
    tess = find_tesseract()
    rapid = _rapidocr_class() is not None
    return {
        "tesseract": {"available": tess is not None,
                      "detail": tess or f"not found — install: {_TESS_HINT}"},
        "rapidocr": {"available": rapid,
                     "detail": "installed" if rapid else f"not found — install: {_RAPID_HINT}"},
    }


def default_engine() -> str | None:
    pref = env("COOLWORDS_OCR_ENGINE").strip().lower()
    st = engines()
    if pref in st:
        return pref if st[pref]["available"] else None
    if st["tesseract"]["available"]:
        return "tesseract"
    if st["rapidocr"]["available"]:
        return "rapidocr"
    return None


def require_engine(name: str | None) -> str:
    eng = (name or "").strip().lower() or default_engine()
    st = engines()
    if not eng:
        raise OcrError(
            f"No OCR engine available. Install one: tesseract: {_TESS_HINT}; rapidocr: {_RAPID_HINT}",
            code="OCR_MISSING")
    if eng not in st:
        raise OcrError(f"Unknown OCR engine '{eng}' (know: {', '.join(st)})", code="OCR_MISSING")
    if not st[eng]["available"]:
        raise OcrError(f"OCR engine '{eng}' is not installed — {st[eng]['detail']}", code="OCR_MISSING")
    return eng


# --------------------------------------------------------------------------- #
#  per-page OCR                                                               #
# --------------------------------------------------------------------------- #
def _ocr_tesseract(png_path: Path) -> str:
    tess = find_tesseract()
    proc = subprocess.run(
        [tess, str(png_path), "stdout", "-l", "eng", "--dpi", str(DPI)],
        capture_output=True,
    )
    if proc.returncode != 0:
        raise OcrError(f"tesseract failed: {proc.stderr.decode('utf-8', 'replace')[:300]}")
    return proc.stdout.decode("utf-8", "replace")


def _ocr_rapidocr(pix) -> str:
    global _rapid_instance
    import numpy as np
    if _rapid_instance is None:
        _rapid_instance = _rapidocr_class()()
    arr = np.frombuffer(pix.samples, dtype=np.uint8).reshape(pix.height, pix.width, pix.n)
    result = _rapid_instance(arr)
    if isinstance(result, tuple):           # classic API: (lines, elapse)
        result = result[0]
    if result is None:
        return ""
    if hasattr(result, "txts"):             # newer API: RapidOCROutput
        return "\n".join(result.txts or [])
    # classic lines: [box(4x2), text, score]; sort into reading order (y, then x)
    def key(line):
        box = line[0]
        return (min(p[1] for p in box), min(p[0] for p in box))
    return "\n".join(line[1] for line in sorted(result, key=key))


def cache_path(pdf_path: Path, engine: str) -> Path:
    return Path(str(pdf_path) + f".ocr.{engine}.json")


def load_cache(pdf_path: Path, engine: str) -> dict:
    p = cache_path(pdf_path, engine)
    if p.exists():
        try:
            return json.loads(p.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            pass
    return {"engine": engine, "dpi": DPI, "pages": {}}


def ocr_pdf(pdf_path: Path, engine: str | None = None, pages: list[int] | None = None,
            dpi: int = DPI) -> tuple[dict[int, str], str, int]:
    """OCR the given 0-based pages (default: all), reading/extending the sidecar
    cache. Returns ({page: text}, engine_used, n_newly_ocred)."""
    import pymupdf

    pdf_path = Path(pdf_path)
    eng = require_engine(engine)
    cache = load_cache(pdf_path, eng)
    cpath = cache_path(pdf_path, eng)

    with pymupdf.open(pdf_path) as doc:
        want = list(pages) if pages is not None else list(range(doc.page_count))
        want = [p for p in want if 0 <= p < doc.page_count]
        todo = [p for p in want if str(p) not in cache["pages"]]
        for i, pno in enumerate(todo):
            page = doc[pno]
            scale = dpi / 72.0
            if eng == "tesseract":
                pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), colorspace=pymupdf.csGRAY)
                with tempfile.TemporaryDirectory() as td:
                    png = Path(td) / "page.png"
                    pix.save(png)
                    text = _ocr_tesseract(png)
            else:
                pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), colorspace=pymupdf.csRGB)
                text = _ocr_rapidocr(pix)
            cache["pages"][str(pno)] = text
            print(f"ocr[{eng}] page {pno + 1} ({i + 1}/{len(todo)})", file=sys.stderr, flush=True)
            if (i + 1) % 10 == 0:  # checkpoint so an interrupt loses little work
                cpath.write_text(json.dumps(cache, ensure_ascii=False), encoding="utf-8")
        if todo:
            cpath.write_text(json.dumps(cache, ensure_ascii=False), encoding="utf-8")

    return {p: cache["pages"].get(str(p), "") for p in want}, eng, len(todo)
