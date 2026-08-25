#!/usr/bin/env python3
"""Turn a set of vectors into a corpus that answers "what is nearest to this".

Nearest-neighbour search looks like the one thing an inverted index cannot do,
and it is the same trick as everything else here. Cluster the vectors; the
cluster a vector belongs to is a *term*, and the vectors in it are that term's
postings. A query finds the few nearest cluster centres, looks them up, and
reranks what comes back. That is an IVF index, which is what a vector database
builds — written down as terms and postings instead of as a private structure.

The payload is a product-quantized code: the vector, compressed to a fixed
number of bytes, which is what makes reranking affordable at all. 128 float32
dimensions become 16 bytes, and the distance to a query is read off a table
rather than computed on the original vector.

    python3 ivf.py --sift sift/ --clusters 1024 --bytes 16 --out vec.tsv

Output is one document per vector, in the corpus format `mokuji build` reads:

    <cluster>\t<id>|<code>

Documents come out in cluster order, so a cluster's postings are a run of
consecutive document IDs and its codes are a run of consecutive payload blocks.
That is the difference between one read per cluster and one per candidate.
"""

import argparse
import base64
import pathlib
import sys

import numpy as np


def read_fvecs(path, limit=None):
    """The `.fvecs` layout: each vector is its dimension, then that many floats."""
    raw = np.fromfile(path, dtype=np.int32)
    dim = raw[0]
    rows = raw.reshape(-1, dim + 1)
    if limit:
        rows = rows[:limit]
    return rows[:, 1:].view(np.float32).astype(np.float32)


def read_ivecs(path):
    raw = np.fromfile(path, dtype=np.int32)
    dim = raw[0]
    return raw.reshape(-1, dim + 1)[:, 1:]


def assign(x, centroids, batch=20000):
    """Nearest centroid for each row of `x`, in batches so the distance matrix
    never has to exist all at once."""
    out = np.empty(len(x), dtype=np.int32)
    norms = (centroids ** 2).sum(1)
    for i in range(0, len(x), batch):
        chunk = x[i : i + batch]
        # |a-b|^2 = |a|^2 - 2ab + |b|^2, and |a|^2 is the same across centroids.
        d = norms - 2.0 * (chunk @ centroids.T)
        out[i : i + batch] = d.argmin(1)
    return out


def kmeans(x, k, iterations, seed=0):
    """Lloyd's algorithm. Nothing clever: the point is the index, not this."""
    rng = np.random.default_rng(seed)
    centroids = x[rng.choice(len(x), k, replace=False)].copy()
    for step in range(iterations):
        labels = assign(x, centroids)
        counts = np.bincount(labels, minlength=k)
        fresh = np.zeros_like(centroids)
        np.add.at(fresh, labels, x)
        # A cluster nobody joined is restarted on a random point rather than
        # left at the origin, where it would attract everything.
        empty = counts == 0
        fresh[~empty] /= counts[~empty, None]
        if empty.any():
            fresh[empty] = x[rng.choice(len(x), int(empty.sum()), replace=False)]
        shift = float(np.linalg.norm(fresh - centroids))
        centroids = fresh
        print(f"    iteration {step + 1}/{iterations}, shift {shift:.1f}", file=sys.stderr)
    return centroids


def train_pq(residuals, m, seed=0):
    """One 256-entry codebook per subspace, which is what makes a code a byte."""
    dim = residuals.shape[1]
    assert dim % m == 0, "the number of bytes must divide the dimension"
    width = dim // m
    books = np.empty((m, 256, width), dtype=np.float32)
    for i in range(m):
        print(f"  subspace {i + 1}/{m}", file=sys.stderr)
        books[i] = kmeans(residuals[:, i * width : (i + 1) * width], 256, 8, seed + i)
    return books


def encode_pq(residuals, books, batch=50000):
    m, _, width = books.shape
    codes = np.empty((len(residuals), m), dtype=np.uint8)
    for i in range(m):
        part = residuals[:, i * width : (i + 1) * width]
        for j in range(0, len(part), batch):
            codes[j : j + batch, i] = assign(part[j : j + batch], books[i])
    return codes


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sift", default="sift")
    parser.add_argument("--clusters", type=int, default=1024)
    parser.add_argument("--bytes", type=int, default=16, help="bytes per code")
    parser.add_argument("--iterations", type=int, default=12)
    parser.add_argument("--out", default="vec.tsv")
    parser.add_argument("--centroids", default="centroids.f32")
    parser.add_argument("--codebooks", default="codebooks.f32")
    parser.add_argument("--sizes", default="sizes.i32")
    parser.add_argument(
        "--align",
        type=int,
        default=0,
        help="pad each cluster out to a multiple of this many documents, so a "
        "cluster's codes never straddle a payload block",
    )
    args = parser.parse_args()

    sift = pathlib.Path(args.sift)
    learn = read_fvecs(sift / "sift_learn.fvecs")
    base = read_fvecs(sift / "sift_base.fvecs")
    print(f"{len(base):,} vectors of {base.shape[1]} dimensions", file=sys.stderr)

    print(f"clustering into {args.clusters}", file=sys.stderr)
    centroids = kmeans(learn, args.clusters, args.iterations)

    print("assigning", file=sys.stderr)
    labels = assign(base, centroids)

    # The residual, not the vector: what is left after the cluster centre has
    # accounted for most of it is smaller and easier to quantize, which is the
    # whole reason a coarse quantizer helps twice.
    print(f"training {args.bytes}-byte codes", file=sys.stderr)
    learn_labels = assign(learn, centroids)
    books = train_pq(learn - centroids[learn_labels], args.bytes)

    print("encoding", file=sys.stderr)
    codes = encode_pq(base - centroids[labels], books)

    centroids.tofile(args.centroids)
    books.tofile(args.codebooks)

    # Cluster order, so that a cluster's documents are consecutive.
    order = np.argsort(labels, kind="stable")
    width = len(str(args.clusters - 1))
    counts = np.bincount(labels, minlength=args.clusters)
    out = pathlib.Path(args.out)
    padding = 0
    with out.open("w", encoding="ascii") as handle:
        written = 0
        cluster = -1
        for doc in order:
            if args.align and labels[doc] != cluster:
                cluster = labels[doc]
                # A document with no text has no terms, so it is never a
                # candidate; it exists to push the next cluster onto a block
                # boundary. Its payload is empty, which deflates to nothing.
                while written % args.align:
                    handle.write("\t\n")
                    written += 1
                    padding += 1
            code = base64.urlsafe_b64encode(codes[doc].tobytes()).decode().rstrip("=")
            handle.write(f"c{labels[doc]:0{width}d}\t{doc}|{code}\n")
            written += 1
    if args.align:
        print(
            f"padded with {padding:,} empty documents "
            f"({100 * padding / len(base):.0f}%)",
            file=sys.stderr,
        )

    # How many documents each cluster got, in cluster order. A reader gets this
    # from the postings; the demo's query script gets it from here because the
    # CLI merges the clusters it was asked for into one result.
    sizes = np.bincount(labels, minlength=args.clusters).astype(np.int32)
    sizes.tofile(args.sizes)

    print(f"wrote {out} ({out.stat().st_size:,} bytes)", file=sys.stderr)
    print(
        f"cluster sizes: min {sizes.min()}, median {int(np.median(sizes))}, "
        f"max {sizes.max()}, mean {sizes.mean():.0f}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
