"""Ingest fastText crawl-300d-2M word embeddings into a numpy sidecar.

Streams the 4.5GB .vec (never fully loaded), keeps only vectors whose token
matches a curated word, L2-normalizes them, and writes:
- data/coolwords_emb.npy : float32 (K, 300), L2-normalized (cosine == dot product)
- word_embedding_map(word_id, row) : word -> row in the matrix
- words.in_fasttext : provenance flag

Brute-force cosine over the matrix (M @ M[i]) is sub-millisecond at this scale,
so no ANN index is needed (and faiss/hnswlib have no Python 3.14 wheels anyway).
"""
import io
import zipfile

import numpy as np

from ingest.db import connect, log_ingest
from ingest.paths import FASTTEXT_VEC_ZIP, DB_PATH

EMB_PATH = DB_PATH.parent / "coolwords_emb.npy"
DIM = 300


def main() -> None:
    con = connect()
    cur = con.cursor()
    want = {w: i for (i, w) in cur.execute("SELECT id, word FROM words")}
    print(f"matching {len(want):,} curated words against fastText ...", flush=True)

    ids: list[int] = []
    rows: list[np.ndarray] = []
    with zipfile.ZipFile(FASTTEXT_VEC_ZIP) as zf:
        name = zf.namelist()[0]
        with zf.open(name) as raw:
            tw = io.TextIOWrapper(raw, encoding="utf-8", newline="\n")
            count, dim = map(int, tw.readline().split())
            assert dim == DIM, f"unexpected dim {dim}"
            for line in tw:
                sp = line.find(" ")
                wid = want.get(line[:sp])
                if wid is None:
                    continue
                v = np.array(line[sp + 1:].split(), dtype=np.float32)
                if v.shape[0] != DIM:
                    continue
                norm = float(np.linalg.norm(v))
                if norm > 0:
                    v /= norm
                ids.append(wid)
                rows.append(v)
                if len(ids) % 50000 == 0:
                    print(f"  matched {len(ids):,} ...", flush=True)

    mat = np.vstack(rows).astype(np.float32) if rows else np.zeros((0, DIM), np.float32)
    np.save(EMB_PATH, mat)
    print(f"wrote {EMB_PATH.name} shape {mat.shape}", flush=True)

    con.execute("DELETE FROM word_embedding_map")
    con.execute("UPDATE words SET in_fasttext = 0")
    cur.executemany(
        "INSERT INTO word_embedding_map(word_id, row) VALUES (?, ?)",
        [(wid, row) for row, wid in enumerate(ids)],
    )
    cur.executemany("UPDATE words SET in_fasttext = 1 WHERE id = ?", [(wid,) for wid in ids])
    con.commit()
    log_ingest(con, "embeddings-fasttext", f"{len(ids)} vectors -> {EMB_PATH.name}", len(ids))
    print(f"embeddings: {len(ids):,} vectors stored ({100*len(ids)/max(1,len(want)):.1f}% of words)", flush=True)
    con.close()


if __name__ == "__main__":
    main()
