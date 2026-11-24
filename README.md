# Mokuji

[![Mokuji — Search, without a server. 3.1 KB of 4.5 MB, in 2 range reads](https://reearth.github.io/mokuji/img/og-en.png)](https://reearth.github.io/mokuji/)

**<https://reearth.github.io/mokuji/>** — search real Mokuji files in the
browser, and see every range read it makes.

**A cloud-optimized concordance format.** Single immutable file, bounded range
reads, no server.

*目次* — the table of contents. The part of a book that is not the book, and
that lets you open it at the right page without reading the rest. Hold a
Mokuji file's table of contents in memory and any lookup costs one range read.

Mokuji is a single-file layout for an inverted index, designed to be published
as a static object and queried directly over HTTP range requests. A search
index becomes a file you upload, not a service you operate.

> **Status: draft, version 0.1.** The format may still change incompatibly
> before 1.0. Every file names the writer and version that produced it, so a
> file can be rebuilt when that happens; pin the commit you depend on.
> [TOWARD-1.0.md](TOWARD-1.0.md) says what 1.0 will promise and what it waits on. Questions,
> measurements on your own data, and independent implementations are welcome
> as [issues](https://github.com/reearth/mokuji/issues).

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
Postings              delta + bit-packing, one read per term,
                      or none when a term's documents fit in its entry
Payload               document ID order, block compressed
```

See [SPEC.md](SPEC.md) for the byte layout, the I/O contract, and the decision
log, and [MANIFEST.md](MANIFEST.md) for collections: an index that changes, built
from files that do not.

## This repository

The reference implementation is one Rust crate, `mokuji`: the library, and
the `mokuji` command built from it with the `cli` feature. `js/`, `python/` and `go/` are readers in other
languages, with the same API.

| | Contents |
|---|---|
| `mokuji` (`src/lib.rs`) | Search a file in what a person types: `lookup`, `complete`, `search` |
| `mokuji::format` | Byte layout: header, section table, codec identifiers. No I/O |
| `mokuji::codec` | Integer and string codecs used inside blocks |
| `mokuji::read` | `RangeReader`, a `FileReader` that reads a file on disk a range at a time, and the reader for each section |
| `mokuji::write` | Deterministic writer |
| `mokuji::meta` | The metadata section, and the canonical CBOR it is written in |
| `mokuji::tokenize` | The tokenizers registered in `TOKENIZERS.md`, which a writer builds with and a reader searches with |
| `mokuji::manifest` | Collections: a manifest over several files (`MANIFEST.md`) |
| `mokuji::cli`, `src/main.rs` | The `mokuji` command, with the `cli` feature |
| `js/` (`mokuji` on npm) | The reader in TypeScript, for browsers, workers and Node, and a writer |
| `python/` (`mokuji` on PyPI) | The reader in Python, with no dependencies, and a writer |
| `go/` (`github.com/reearth/mokuji/go`) | The reader in Go, and a writer |

The module boundaries follow the section boundaries in the specification,
because keeping the dictionary, the postings, and the payload from knowing
about each other is a requirement of the format and not only good practice.

## Getting started

Install the command line tool:

```sh
brew install reearth/tap/mokuji
cargo install mokuji --features cli    # or from source, with a Rust toolchain
```

and the library for your language:

```sh
npm install mokuji     # JavaScript: browsers, workers and Node
cargo add mokuji       # Rust
pip install mokuji     # Python
go get github.com/reearth/mokuji/go  # Go
```

Its input is one document per line, its text and its payload separated by a
tab:

```sh
printf 'tokyo chiyoda marunouchi\t35.681,139.767\n' >  places.tsv
printf 'tokyo chuo ginza\t35.671,139.772\n'          >> places.tsv
printf 'osaka kita umeda\t34.702,135.496\n'          >> places.tsv

mokuji build places.tsv places.mokuji
mokuji search places.mokuji tokyo chuo
```

`search` prints the matches, then what the query cost:

```
1	1.451	35.671,139.772

1 match(es), 1 shown, ranked by BM25
open:     1 read(s)
query:    2 read(s) - 1 dictionary and postings, 1 payload
bytes:    118 of 748
```

`mokuji prefix` completes a prefix, `mokuji info` shows the sections and the
metadata, and `mokuji verify places.mokuji --source places.tsv` reads the whole
file, checks it, and confirms it was built from that source. `mokuji` with no
arguments lists every command and option.

To query a file from a browser, a Cloudflare Worker or Node, serve it from
anywhere that honours `Range` requests and read it with
[`mokuji`](js) on npm. The demos below are each a complete, reproducible
example of one kind of index.

## Implementing it

[SPEC.md](SPEC.md) is meant to be enough to write a reader or a writer without
reading this code. [conformance](conformance) holds the fixtures and cases an
implementation answers to — results and the number of range reads — and a
Python builder that produces the fixtures from the specification alone,
independently of the Rust writer. The Rust crates, the TypeScript, Python and Go
readers, and that builder all pass the same cases.

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
| Dictionary | 863 KB (12%) | 1.10 MB (40%) |
| **Postings** | **0 bytes** — implied | **1.58 MB (58%)** |
| **Payload** | **5.99 MB (84%)** — coordinates | **43 KB (1.6%)** — paths |
| Document lengths | absent | 4.3 KB |
| Index | 7.1 MB | 2.7 MB |
| A query | prefix, **3 reads**, 1.1 KB | `建築物` ranked, **6 reads**, 16 KB |

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
in 2.0 MB, a point answered in 3–4 reads, and the address key alongside the
cells for 27 KB more — one file, both directions. The whole country is 231,668
boundaries in 126 MB, or 91.5 MB with coordinates rounded to a metre, a point
answered in 2 requests and 20 KB — against 8 requests and 150 KB from the same
polygons in FlatGeobuf — and each polygon keyed by the address registry's name
for it too.

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

The full-text side is the less complete of the two. `bigram` is coarser than
morphological analysis and produces a larger dictionary; phrase queries need
`--positions`, which quadruples that index; and ranking is the caller's, which
is deliberate but is still work the caller has to do. See [SPEC.md](SPEC.md) §1
for where those lines are drawn and why.

## Address lookup

Forward geocoding is the case the format is shaped for, because an address is a
hierarchical key: looked up whole or by prefix, never by intersecting its parts.

```sh
cd demo/addresses
./fetch-abr.sh abr
python3 abr.py town --fullset abr/mt_town_fullset_all.csv --pos abr/pos \
  --city-pos abr/mt_city_pos_all.csv --municipalities --pos-level --out abr.tsv
mokuji build abr.tsv abr.mokuji --tokenizer ja-address --payload ints:6 \
  --attribute pos_level
mokuji prefix abr.mokuji 東京都千代田区丸の内
```

Against the 727,388 town-level addresses and municipalities of Japan's Address
Base Registry, in a 4.7 MB file, completing a prefix costs **two range reads and about 3 KB** — and
costs the same two at any file size, because a contiguous range of terms is
contiguous in every section at once. See [demo/addresses](demo/addresses) for the
measurements, the provenance chain back to the register, and what the numbers do
and do not show.

A browser reads the same kind of file in [demo/web](demo/web) — Tokyo at
residence level, 1,744,962 addresses in 8.7 MB — with a reader of 13 KB
gzipped, no WebAssembly and no bundled decompressor. That is what the choice of
deflate as the required codec buys: browsers already have it. The reader is the
[`mokuji`](js) on npm package, which a page, a Cloudflare Worker and Node all
use unchanged.

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

## Developing

```sh
cargo test --all-features                             # the Rust crate, the command included
python3 conformance/fixtures/build_fixtures.py --check  # the fixtures match the spec
(cd js && npm ci && npm test)                         # the TypeScript reader
python3 -m unittest discover -s python/tests          # the Python reader
(cd go && go test ./...)                              # the Go reader
```

CI runs these, with `cargo fmt --check` and `cargo clippy`, the library
again without the `cli` feature, and a build of `tools/`.

A `v*` tag releases the command and publishes the packages; see
[RELEASING.md](RELEASING.md).

## License

Code is licensed under either of [Apache License, Version 2.0](LICENSE-APACHE)
or [MIT license](LICENSE-MIT) at your option.

The specification is [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/),
so that an independent implementation can quote it freely.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work shall be dual licensed as above, without any
additional terms or conditions.
