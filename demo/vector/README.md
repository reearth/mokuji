# Nearest-neighbour search

A vector database is the most server-shaped thing in a modern stack, and the
structure underneath the common one is an **inverted index**.

Cluster the vectors. The cluster a vector belongs to is a *term*; the vectors in
it are that term's postings. A query finds the few nearest cluster centres, looks
those clusters up, and reranks what comes back. That is IVF, which is what FAISS
builds and what most vector databases run — written down as terms and postings
instead of as a private structure.

The payload is a product-quantized code: 128 float32 dimensions compressed to 16
bytes, so that reranking a few thousand candidates is a table lookup per
candidate rather than a distance over a vector.

```sh
# SIFT1M: 1,000,000 descriptors, 10,000 queries, and the true 100 nearest
# neighbours of each. ftp://ftp.irisa.fr/local/texmex/corpus/sift.tar.gz
python3 ivf.py --sift sift/ --clusters 1024 --bytes 16 --align 1024 --out vec.tsv
cat centroids.f32 codebooks.f32 sizes.i32 > quantizer.bin
mokuji build vec.tsv vec.mokuji --tokenizer space-split \
    --no-freqs --no-doc-lengths \
    --docs-per-payload-block 1024 --terms-per-block 1024 \
    --section 0x8000=quantizer.bin \
    --meta index=ivf-pq --meta clusters=1024 --meta code-bytes=16 --meta align=1024

python3 search.py --index vec.mokuji --nprobe 8
```

| | |
|---|---|
| Vectors | 1,000,000 × 128 dimensions |
| Raw | 512 MB |
| Index | **21.8 MB**, quantizer included |
| Clusters | 1,024 — terms |
| Code | 16 bytes per vector |
| A query | **21 range reads, 190 KB** |

## Recall

Measured against the ground truth SIFT ships, so the numbers mean the same thing
they mean anywhere else. `1-recall@10` is whether the true nearest neighbour came
back at all; `10-recall@10` is how much of the true top ten did.

| nprobe | 1-recall@10 | 10-recall@10 | Candidates | Reads | KB |
|---|---|---|---|---|---|
| 1 | 0.490 | 0.330 | 1,144 | 3.6 | 28 |
| 2 | 0.605 | 0.410 | 2,315 | 6.2 | 52 |
| 4 | 0.760 | 0.481 | 4,596 | 11.2 | 99 |
| **8** | **0.885** | **0.539** | **9,051** | **21.4** | **190** |
| 16 | 0.920 | 0.555 | 17,673 | 41.4 | 368 |
| 32 | 0.925 | 0.564 | 34,376 | 80.6 | 713 |

Recall is bounded twice over, and it is worth knowing which bound is binding.
The clusters are the first: at nprobe 8, 85.1% of the true ten are in a probed
cluster at all, and nothing downstream can recover the rest. The code is the
second, and here it is the one that hurts — of the 85.1% the clusters could
reach, 16 bytes keeps 63%.

Which is a knob, and it costs bytes rather than reads:

| Code | Index | 1-recall@10 | 10-recall@10 | Reads | KB |
|---|---|---|---|---|---|
| 8 bytes | 13.6 MB | 0.620 | 0.353 | 21.4 | 118 |
| 16 bytes | 21.8 MB | 0.885 | 0.539 | 21.4 | 190 |
| 32 bytes | 37.8 MB | 0.915 | 0.656 | 21.4 | 331 |

The read count does not move. A longer code changes what a block holds, not how
many blocks a query touches.

## What this demonstrates

**Padding is free, and it halves the bytes.** A cluster's documents are
consecutive, but a cluster does not start on a payload block boundary, so its
codes straddle blocks and each edge costs a whole extra block. `--align 1024`
pads each cluster out with **444,592 empty documents** — 44% more documents —
so that every cluster starts on a boundary.

| | Reads | KB fetched | File |
|---|---|---|---|
| Unaligned | 25.7 | 356 | 21,198,443 |
| Aligned | 21.4 | 190 | 21,174,665 |

Bytes fetched nearly halve, and **the file gets smaller**. A document with no
text has no terms and is never a candidate; its payload is empty, which is one
varint that deflates to nothing. 44% more documents cost 24 KB less than the
block misalignment did.

**Block size is the reads-versus-bytes dial, and it is set by the cluster size.**

| Documents per payload block | Reads | KB |
|---|---|---|
| 64 | 162.8 | 213 |
| 256 | 53.4 | 240 |
| 1,024 | 26.1 | 363 |
| 2,048 | 21.6 | 532 |

The default of 64 is right for a document index, where a query returns ten
things. Here a query returns nine thousand, and the block wants to be the size of
a cluster.

**The quantizer rides in the file.** Centroids, codebooks and cluster sizes are
659 KB that a reader needs before it can ask anything, and the format has no
opinion about any of it. `--section 0x8000=quantizer.bin` attaches it as an
application section (`SPEC.md` §5.2), located from the section table like
everything else. One object, no sidecar. This demo is why the flag exists — the
specification reserved those identifiers and nothing produced one.

**Payload is 95% of the file, and postings are 1.5%.** 330 KB of postings for
1,000,000 entries is 2.6 bits each, because documents are in cluster order and a
cluster's postings are therefore a run of consecutive IDs that delta-codes to
all ones. The same ordering that makes the payload blocks work makes the
postings nearly disappear.

## Where it stops

**Something still has to embed the query.** SIFT ships its query vectors, which
lets this demo skip the question. A text application cannot: it needs the model
that produced the index to be available at query time, in the browser or at the
edge. The index is static; the encoder is not. So the honest claim here is **no
vector database**, not *no server* — which is what the other demos claim.

**HNSW does not fit, and should not be made to.** A graph walk is pointer
chasing with no bound on its depth, and each hop is a round trip. It is the same
argument that kept a monolithic FST out of the term dictionary (`SPEC.md`
Appendix A). IVF fits because probing is a fixed number of independent lookups
and a graph walk is not.

**The postings are doing very little work.** Every cluster's postings list is a
run of consecutive document IDs, which two numbers would state. A reader that
already holds the cluster sizes — this one does, they are in the quantizer
section — could skip the postings entirely and go from cluster to document range,
saving 8 of the 21 reads. The generality costs something here; in the demos where
a term names scattered documents it is the whole point.

**Nothing here is a benchmark of the algorithm.** k-means with twelve iterations
and a textbook PQ are what these numbers were measured on. FAISS would train
better codebooks and get more recall out of the same 16 bytes. What is being
measured is the *file*: what a query costs in range reads and how much of the
object it touches.
