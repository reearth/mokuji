# COLex

**A cloud-optimized concordance format.** Single immutable file, bounded range
reads, no server.

COLex is a single-file layout for an inverted index, designed to be published
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

For a query of `k` terms returning `m` documents, COLex bounds a query at
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
log.

## This repository

The reference implementation, in Rust.

| Crate | Contents |
|---|---|
| `colex-format` | Byte layout: header, section table, codec identifiers. No I/O, no dependencies |
| `colex-codec` | Integer and string codecs used inside blocks |
| `colex-read` | `RangeReader`, and the reader for each section |
| `colex-write` | Deterministic writer |
| `colex-meta` | The metadata section, and the canonical CBOR it is written in |
| `colex-cli` | The `colex` command |
| `colex-wasm` | Browser bindings over `fetch` with `Range` |

The crate boundaries follow the section boundaries in the specification,
because keeping the dictionary, the postings, and the payload from knowing
about each other is a requirement of the format and not only good practice.

```sh
cargo test

# text<TAB>payload, one document per line
colex build addresses.tsv addresses.colex
colex search addresses.colex tokyo chuo
colex verify addresses.colex --source addresses.tsv
colex info addresses.colex
```

`search` reports what the query cost:

```
1	35.671,139.772

1 match(es)
open: 1 read(s)
query: 5 read(s), bound 5
```

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

The same file is read by a browser in [demo/web](demo/web), in about 300 lines
of plain JavaScript with no WebAssembly and no bundled decompressor. That is
what the choice of deflate as the required codec buys: browsers already have
it.

## Scope

Scoring is outside the specification. A reader returns postings and the global
statistics BM25 needs; which model to apply is a property of the corpus.
Tokenization is likewise outside it, but recorded in the metadata, so that an
index remains reproducible without the format having to choose between
morphological analysis and n-grams. [TOKENIZERS.md](TOKENIZERS.md) assigns the
identifiers used for that — including `ja-address`, which normalizes Japanese
addresses. Normalization belongs there rather than in a preprocessing script,
because it decides what a term is, and the index should record that.

Because the metadata names the source by content hash, `colex verify` can walk
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
