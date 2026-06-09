"""Cluster a book's candidates by embedding, and pick a varied top-N (shadowing).

    python -m ingest.cluster --slug gutenberg-11 --k 12 --top 20

- cluster: spherical k-means over the fastText vectors (already L2-normalized,
  so cosine == dot product) -> candidates.cluster, for browsing thematic groups.
- selected: greedy max-min-distance pick down the score-ranked list, skipping any
  word within `shadow` cosine of an already-picked word -> candidates.selected,
  giving a varied final shortlist (your "shadowing" requirement).

Candidates without an embedding can't be clustered/shadowed; they are left
cluster=NULL and are eligible for selection as always-distinct (never shadowed).
"""
import argparse

import numpy as np

from ingest.db import connect, log_ingest
from ingest.paths import DB_PATH

EMB_PATH = DB_PATH.parent / "coolwords_emb.npy"


def spherical_kmeans(X: np.ndarray, k: int, iters: int = 60) -> np.ndarray:
    rng = np.random.default_rng(0)
    n = len(X)
    k = min(k, n)
    # k-means++ style init using cosine distance (X is unit-normalized)
    centers = [X[rng.integers(n)]]
    for _ in range(1, k):
        sims = X @ np.array(centers).T
        d = np.clip(1.0 - sims.max(axis=1), 0, None)
        total = d.sum()
        idx = rng.choice(n, p=d / total) if total > 0 else rng.integers(n)
        centers.append(X[idx])
    C = np.array(centers, dtype=np.float32)
    labels = np.zeros(n, dtype=int)
    for _ in range(iters):
        labels = (X @ C.T).argmax(axis=1)
        newC = C.copy()
        for j in range(k):
            members = X[labels == j]
            if len(members):
                c = members.mean(axis=0)
                norm = np.linalg.norm(c)
                if norm > 0:
                    newC[j] = c / norm
        if np.allclose(newC, C):
            break
        C = newC
    return labels


def cluster_level(con, book_id, level, mat, rowmap, words, k, top, shadow) -> int:
    """Cluster + greedy-shadow one (book, level) candidate set. Returns #embedded."""
    cands = con.execute(
        "SELECT word_id, score FROM candidates WHERE book_id = ? AND level = ? ORDER BY rank",
        (book_id, level),
    ).fetchall()
    emb = [(wid, score) for (wid, score) in cands if wid in rowmap]  # score-ranked
    con.execute("UPDATE candidates SET cluster = NULL, selected = 0 WHERE book_id = ? AND level = ?",
                (book_id, level))
    if not emb:
        return 0
    X = np.vstack([mat[rowmap[wid]] for wid, _ in emb]).astype(np.float32)

    labels = spherical_kmeans(X, k)
    con.executemany(
        "UPDATE candidates SET cluster = ? WHERE book_id = ? AND level = ? AND word_id = ?",
        [(int(labels[i]), book_id, level, emb[i][0]) for i in range(len(emb))],
    )

    # greedy varied pick down the score-ranked list (shadowing)
    picked: list[int] = []
    picked_vecs: list[np.ndarray] = []
    for i in range(len(emb)):
        v = X[i]
        if picked_vecs:
            sims = np.asarray(picked_vecs) @ v
            if sims[int(sims.argmax())] >= shadow:
                continue
        picked.append(i)
        picked_vecs.append(v)
        if len(picked) >= top:
            break
    con.executemany(
        "UPDATE candidates SET selected = 1 WHERE book_id = ? AND level = ? AND word_id = ?",
        [(book_id, level, emb[i][0]) for i in picked],
    )
    if level == 0:
        print(f"  level 0 varied top: {', '.join(words[emb[i][0]] for i in picked[:12])} …")
    return len(emb)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--slug", required=True)
    ap.add_argument("--k", type=int, default=12)
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--shadow", type=float, default=0.55)
    args = ap.parse_args()

    con = connect()
    book_id = con.execute("SELECT id FROM books WHERE slug = ?", (args.slug,)).fetchone()[0]
    mat = np.load(EMB_PATH)
    rowmap = {wid: row for (wid, row) in con.execute("SELECT word_id, row FROM word_embedding_map")}
    words = {wid: w for (wid, w) in con.execute(
        "SELECT c.word_id, x.word FROM candidates c JOIN words x ON x.id = c.word_id WHERE c.book_id = ?",
        (book_id,))}

    per_level = {}
    for level in (0, 1, 2, 3):
        per_level[level] = cluster_level(
            con, book_id, level, mat, rowmap, words, args.k, args.top, args.shadow)
    con.commit()
    log_ingest(con, "cluster", f"{args.slug}: per-level embedded " + str(per_level), sum(per_level.values()))
    print(f"{args.slug}: clustered + shadowed; embedded candidates per level {per_level}")
    con.close()


if __name__ == "__main__":
    main()
