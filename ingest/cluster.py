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

    cands = con.execute(
        "SELECT word_id, score FROM candidates WHERE book_id = ? ORDER BY rank", (book_id,)
    ).fetchall()
    words = {wid: w for (wid, w) in con.execute(
        "SELECT c.word_id, x.word FROM candidates c JOIN words x ON x.id = c.word_id WHERE c.book_id = ?",
        (book_id,))}

    emb = [(wid, score) for (wid, score) in cands if wid in rowmap]  # score-ranked
    if not emb:
        raise SystemExit("no candidates have embeddings")
    X = np.vstack([mat[rowmap[wid]] for wid, _ in emb]).astype(np.float32)

    labels = spherical_kmeans(X, args.k)
    con.execute("UPDATE candidates SET cluster = NULL, selected = 0 WHERE book_id = ?", (book_id,))
    con.executemany(
        "UPDATE candidates SET cluster = ? WHERE book_id = ? AND word_id = ?",
        [(int(labels[i]), book_id, emb[i][0]) for i in range(len(emb))],
    )

    # greedy varied pick down the score-ranked list
    picked: list[int] = []      # indices into emb
    picked_vecs: list[np.ndarray] = []
    shadowed_by: dict[int, int] = {}
    for i in range(len(emb)):
        v = X[i]
        if picked_vecs:
            sims = np.asarray(picked_vecs) @ v
            j = int(sims.argmax())
            if sims[j] >= args.shadow:
                shadowed_by[i] = picked[j]
                continue
        picked.append(i)
        picked_vecs.append(v)
        if len(picked) >= args.top:
            break

    con.executemany(
        "UPDATE candidates SET selected = 1 WHERE book_id = ? AND word_id = ?",
        [(book_id, emb[i][0]) for i in picked],
    )
    con.commit()
    log_ingest(con, "cluster", f"{args.slug}: k={args.k}, top={args.top}", len(emb))

    # --- verification output ---
    print(f"{args.slug}: {len(emb):,}/{len(cands):,} candidates have embeddings; "
          f"k={min(args.k,len(emb))} clusters; picked {len(picked)} varied words\n")
    print("== clusters (top words by score) ==")
    by_cluster: dict[int, list[str]] = {}
    for i in range(len(emb)):
        by_cluster.setdefault(int(labels[i]), []).append(words[emb[i][0]])
    for c in sorted(by_cluster):
        print(f"  c{c:>2}: {', '.join(by_cluster[c][:8])}")

    print("\n== varied top-20 (selected) ==")
    for rank, i in enumerate(picked, 1):
        print(f"  {rank:>2}. {words[emb[i][0]]:<16} (c{int(labels[i])}, score {emb[i][1]:.2f})")

    print("\n== examples of shadowed (skipped) words and what shadowed them ==")
    for shown, (i, by) in enumerate(shadowed_by.items()):
        if shown >= 12:
            break
        print(f"  {words[emb[i][0]]:<16} shadowed by {words[emb[by][0]]}")
    con.close()


if __name__ == "__main__":
    main()
