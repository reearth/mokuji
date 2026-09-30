# Mokuji

**A cloud-optimized concordance format.** Single immutable file, bounded range
reads, no server.

*目次* — the table of contents. The part of a book that is not the book, and
that lets you open it at the right page without reading the rest. Hold a
Mokuji file's table of contents in memory and any lookup costs one range read.

Mokuji is a single-file layout for an inverted index, designed to be published
as a static object and queried directly over HTTP range requests. A search
index becomes a file you upload, not a service you operate.

> Status: **early draft.** The format is not frozen and will change. Do not
> build on it yet.

## Why

Search indexes have historically required a server because they were mutable.
Once a snapshot is accepted as the unit of publication, and generations are
managed explicitly, the server stops being necessary — the same argument that
moved map tiles onto object storage.

Parts of this already exist. What has been missing is a public specification
that states an **upper bound on I/O**. A format served over range requests is
only safe to publish if the number of requests per query is bounded and
written down; without that, cost and latency are unpredictable in a way that
no amount of implementation quality fixes.

For a query of `k` terms returning `m` documents, Mokuji bounds a query at
`2k + m` range reads, plus one read on a cold start. That bound is in the
specification, and the conformance suite asserts it.

Two consequences fall out of the design rather than being added to it: the
term index is order-preserving, so prefix and range queries work structurally,
which suits hierarchical keys such as addresses; and the header records the
content hash of the source it was derived from, so an index is a verifiable
derivative of a source of truth rather than an opaque blob.

## Layout

```
Header + section table + metadata + term index   resident, tens of KB
Term blocks           front-coded, one read per term
Postings              delta + bit-packing, one read per term
Payload               document ID order, block compressed
```

See [SPEC.md](SPEC.md) for the byte layout, the I/O contract, and the decision
log, and [MANIFEST.md](MANIFEST.md) for collections: an index that changes, built
from files that do not.

## This repository

The reference implementation, in Rust.

| Crate | Contents |
|---|---|
| `mokuji-format` | Byte layout: header, section table, codec identifiers. No I/O, no dependencies |
| `mokuji-codec` | Integer and string codecs used inside blocks |
| `mokuji-read` | `RangeReader`, and the reader for each section |
| `mokuji-write` | Deterministic writer |
| `mokuji-meta` | The metadata section, and the canonical CBOR it is written in |
| `mokuji-cli` | The `mokuji` command |

The crate boundaries follow the section boundaries in the specification,
because keeping the dictionary, the postings, and the payload from knowing
about each other is a requirement of the format and not only good practice.

```sh
cargo test

# text<TAB>payload, one document per line
mokuji build addresses.tsv addresses.mokuji
mokuji search addresses.mokuji tokyo chuo
mokuji verify addresses.mokuji --source addresses.tsv
mokuji info addresses.mokuji
```

`search` reports what the query cost:

```
1	35.671,139.772

1 match(es)
open: 1 read(s)
query: 5 read(s), bound 5
```

## Three shapes on one layout

A geocoder, a place-name search and a full-text search differ in nearly
everything a user would notice. What counts as a term. Whether a term names one record or thousands.
What the payload means. Whether results come back ranked. What a query even
looks like — a prefix in one case, an intersection in the other.

They are the same file layout, and the two indexes here are built by the same
writer and read by the same reader:

| | Japanese addresses | PLATEAU specifications |
|---|---|---|
| Records | 1,744,962 | 2,523 |
| Terms | 1,744,962 | 77,240 |
| Tokenizer | `ja-address` | `bigram` |
| Dictionary | 863 KB (12%) | 1.08 MB (35%) |
| **Postings** | **0 bytes** — implied | **1.96 MB (63%)** |
| **Payload** | **5.99 MB (84%)** — coordinates | **43 KB (1.4%)** — paths |
| Document lengths | absent | 4.3 KB |
| Index | 7.1 MB | 3.1 MB |
| A query | prefix, **3 reads**, 1.1 KB | two terms ranked, **21 reads**, 57 KB |

A gazetteer is the third: [demo/poi](demo/poi) indexes 98,187 Japanese places
under all their names at once, with coordinates in attribute columns rather than
payload, because a place needs a name to show as well as a position to return.

All three are reproducible: [demo/addresses](demo/addresses),
[demo/poi](demo/poi), [demo/documents](demo/documents).

[demo/postal](demo/postal) is a fourth arrangement rather than a fourth shape:
postal codes and addresses in **one** dictionary, answering in both directions,
because the two key spaces sort into disjoint ranges and neither disturbs the
other's prefixes.

[demo/fuzzy](demo/fuzzy) is the same index answered loosely: character bigrams,
the **union** of the query's terms instead of their intersection, and postings
fetched for only the rarest few — which the dictionary reports before any of
them are read. `丸ノ内` finds `丸の内`, and a dropped prefecture finds the
address 94% of the time.

[demo/vector](demo/vector) is the one that looks impossible. Cluster a million
vectors and the cluster is a term, the vectors in it are its postings, and the
quantized code is the payload — which is IVF, the structure a vector database
runs. SIFT1M in **21.8 MB** against 512 MB raw, and a nearest-neighbour query in
**21 range reads**.

[demo/reverse](demo/reverse) takes that further and goes the other way round the
map. A geohash cell is a string whose prefixes are the cells containing it, so
*which polygon holds this point* is a term lookup: 6,021 Tokyo 町丁目 boundaries
in 2.6 MB, a point answered in 4–5 reads, and the address key alongside the
cells for 44 KB more — one file, both directions.

The section profiles are inverted. In the address index the postings section is
empty and payload is five sixths of the file; in the document index postings are
two thirds and payload is a rounding error. Nothing in the format had to choose
between them.

That works because of what the format declines to decide, not because of what it
specifies:

| What differs | Where it was left |
|---|---|
| What a term is | the tokenizer: recorded, not specified |
| Whether a term names one record or many | postings, which vanish when a term names its own record |
| What the payload means | outside the specification |
| How a number in the payload is written down | declared in the file, not interpreted |
| Whether results are ranked | scoring outside; the statistics a scorer needs inside |

What both get is the same: a sorted dictionary, postings and payload that can be
located without reading anything else, provenance back to a source of truth, and
a stated bound on the number of range reads.

The full-text side is the less complete of the two. There are no positions, so
no phrase queries; `bigram` is coarser than morphological analysis and produces
a larger dictionary; and ranking is the caller's, which is deliberate but is
still work the caller has to do. See [SPEC.md](SPEC.md) §1 for where those lines
are drawn and why.

## Address lookup

Forward geocoding is the case the format is shaped for, because an address is a
hierarchical key: looked up whole or by prefix, never by intersecting its parts.

```sh
cd demo/addresses && ./run.sh
```

Against the 725,814 addresses of Japan's Address Base Registry, in a 5.7 MB
file, completing a prefix costs **two range reads and about 1 KB** — and costs
the same two at any file size, because a contiguous range of terms is contiguous
in every section at once. See [demo/addresses](demo/addresses) for the
measurements, the provenance chain back to the register, and what the numbers do
and do not show.

A browser reads the same kind of file in [demo/web](demo/web) — Tokyo at
residence level, 1,744,962 addresses in 8.7 MB — in about 400 lines of plain
JavaScript with no WebAssembly and no bundled decompressor. That is what the
choice of deflate as the required codec buys: browsers already have it.

## Scope

Scoring is outside the specification. A reader returns postings and the global
statistics BM25 needs; which model to apply is a property of the corpus.
Tokenization is likewise outside it, but recorded in the metadata, so that an
index remains reproducible without the format having to choose between
morphological analysis and n-grams. [TOKENIZERS.md](TOKENIZERS.md) assigns the
identifiers used for that — including `ja-address`, which normalizes Japanese
addresses. Normalization belongs there rather than in a preprocessing script,
because it decides what a term is, and the index should record that.

Because the metadata names the source by content hash, `mokuji verify` can walk
a whole file, check that everything it claims about itself holds, and confirm
that it derives from the source it says it does.

## License

Code is licensed under either of [Apache License, Version 2.0](LICENSE-APACHE)
or [MIT license](LICENSE-MIT) at your option.

The specification is [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/),
so that an independent implementation can quote it freely.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work shall be dual licensed as above, without any
additional terms or conditions.
