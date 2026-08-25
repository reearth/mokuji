#!/usr/bin/env python3
"""Nearest-neighbour search over an index built by ivf.py, and what it recalls.

The query is three steps, and only the middle one touches the file:

1. Find the `nprobe` nearest cluster centres. This is arithmetic over a resident
   table, not a read.
2. Look those clusters up as terms. That is `2k + m` of `SPEC.md` §4 with k set
   by nprobe.
3. Rerank what came back by asymmetric distance against the quantized codes,
   which is a table lookup per code rather than a distance over the vector.

Recall is measured against the ground truth SIFT ships, so the numbers are
comparable to what any other index reports on the same data.

    python3 search.py --index vec.mokuji --nprobe 8 --queries 200
"""

import argparse
import base64
import pathlib
import subprocess
import sys

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from ivf import read_fvecs, read_ivecs


def quantizer(mokuji, index, section, clusters, dim, m):
    """The coarse quantizer, out of the file it belongs to.

    Centroids, codebooks and cluster sizes are what a reader needs before it can
    ask anything, and they are not the format's business — so they ride in an
    application section (`SPEC.md` §5.2), located from the section table like
    everything else. One object, no sidecar.
    """
    table = subprocess.run(
        [mokuji, "info", str(index)], capture_output=True, text=True, check=True
    ).stdout
    row = [l for l in table.splitlines() if l.strip().startswith(f"{section:#06x}")]
    if not row:
        raise SystemExit(f"no section {section:#06x} in {index}")
    offset, length = (int(x) for x in row[0].split()[2::2])
    with open(index, "rb") as handle:
        handle.seek(offset)
        blob = handle.read(length)
    at = clusters * dim * 4
    centroids = np.frombuffer(blob[:at], dtype=np.float32).reshape(clusters, dim)
    books = np.frombuffer(blob[at : at + m * 256 * (dim // m) * 4], dtype=np.float32)
    sizes = np.frombuffer(blob[at + books.nbytes :], dtype=np.int32)
    return centroids, books.reshape(m, 256, -1), sizes


def probe(mokuji, index, clusters, width, limit):
    """Every document in the named clusters, with what the lookup cost."""
    terms = [f"c{c:0{width}d}" for c in clusters]
    out = subprocess.run(
        [mokuji, "search", str(index), *terms, "--any", "-n", str(limit)],
        capture_output=True,
        text=True,
        check=True,
    )
    docs = []
    ids = []
    codes = []
    for line in out.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 2:
            continue
        # The document ID is the index's; the ID in the payload is the one the
        # vectors were published with, and the one the ground truth names.
        docs.append(int(parts[0]))
        original, _, code = parts[1].partition("|")
        ids.append(int(original))
        codes.append(base64.urlsafe_b64decode(code + "=="))
    reads = 0
    read_bytes = 0
    for line in out.stderr.splitlines():
        if line.startswith("query:"):
            reads = int(line.split()[1])
        elif line.startswith("bytes:"):
            read_bytes = int(line.split()[1])
    return (
        np.array(docs),
        np.array(ids),
        np.frombuffer(b"".join(codes), dtype=np.uint8).reshape(len(docs), -1),
        reads,
        read_bytes,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--index", default="vec.mokuji")
    parser.add_argument("--sift", default="sift")
    parser.add_argument("--section", type=lambda v: int(v, 16), default=0x8000)
    parser.add_argument("--clusters", type=int, default=1024)
    parser.add_argument("--dim", type=int, default=128)
    parser.add_argument("--code-bytes", type=int, default=16)
    parser.add_argument("--align", type=int, default=0)
    parser.add_argument("--nprobe", default="1,2,4,8,16,32")
    parser.add_argument("--queries", type=int, default=200)
    parser.add_argument("--k", type=int, default=10)
    parser.add_argument("--mokuji", default="mokuji")
    args = parser.parse_args()

    m = args.code_bytes
    centroids, books, sizes = quantizer(
        args.mokuji, args.index, args.section, args.clusters, args.dim, m
    )
    clusters = len(sizes)
    dim = centroids.shape[1]
    width = len(str(clusters - 1))

    sift = pathlib.Path(args.sift)
    queries = read_fvecs(sift / "sift_query.fvecs")[: args.queries]
    truth = read_ivecs(sift / "sift_groundtruth.ivecs")[: args.queries]

    # Which cluster a document is in. A reader knows because it asked for that
    # term; this script asked for several at once and got them merged.
    if args.align:
        # Each cluster was pushed onto a block boundary, so its documents start
        # at the next multiple of the alignment rather than where the last one
        # ended. The padding in between belongs to no cluster and is never a
        # candidate, so it only has to be skipped over.
        starts = [0]
        for size in sizes:
            end = starts[-1] + int(size)
            starts.append((end + args.align - 1) // args.align * args.align)
        bounds = np.array(starts)
    else:
        bounds = np.concatenate([[0], np.cumsum(sizes)])

    print(
        f"{'nprobe':>7}{'1-recall@' + str(args.k):>12}"
        f"{str(args.k) + '-recall@' + str(args.k):>13}{'candidates':>12}{'reads':>8}{'KB':>9}"
    )
    for nprobe in [int(n) for n in args.nprobe.split(",")]:
        hits = first = 0
        total_candidates = total_reads = total_bytes = 0
        for q, want in zip(queries, truth[:, : args.k]):
            order = np.argsort(((centroids - q) ** 2).sum(1))[:nprobe]
            docs, ids, codes, reads, read_bytes = probe(
                args.mokuji, args.index, sorted(order), width, 4_000_000
            )
            total_candidates += len(docs)
            total_reads += reads
            total_bytes += read_bytes

            # Asymmetric distance: the query's residual against each cluster is
            # turned into one 256-entry table per subspace, and a code's distance
            # is then sixteen table lookups and an add.
            which = np.searchsorted(bounds, docs, side="right") - 1
            best = np.full(len(docs), np.inf, dtype=np.float32)
            for c in sorted(order):
                mask = which == c
                if not mask.any():
                    continue
                residual = q - centroids[c]
                table = np.empty((m, 256), dtype=np.float32)
                sub = dim // m
                for i in range(m):
                    part = residual[i * sub : (i + 1) * sub]
                    table[i] = ((books[i] - part) ** 2).sum(1)
                picked = codes[mask]
                best[mask] = table[np.arange(m), picked].sum(1)

            top = ids[np.argsort(best)[: args.k]]
            hits += len(set(top.tolist()) & set(want.tolist()))
            # The literature's headline: was the true nearest neighbour found
            # at all, anywhere in the k returned.
            first += int(want[0] in top)

        n = len(queries)
        print(
            f"{nprobe:>7}{first / n:>12.3f}{hits / (n * args.k):>13.3f}"
            f"{total_candidates // n:>12,}{total_reads / n:>8.1f}"
            f"{total_bytes / n / 1024:>9.1f}"
        )


if __name__ == "__main__":
    main()
