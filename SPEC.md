# COLex Format Specification

**Version:** 0.1 (draft — not frozen, subject to breaking change)
**Status:** Draft
**Date:** 2026-08-24
**Editor:** Eukarya Inc.

> COLex — a cloud-optimized concordance format.
> Single immutable file, bounded range reads, no server.

This document is licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

---

## 1. Scope

COLex defines a single-file, immutable, byte-addressable layout for an inverted
index that is intended to be served as a static object over HTTP range requests.

A COLex file answers exactly one question: **for a set of query terms, which
documents contain them, and what are the statistics needed to rank them?**

The following are explicitly **outside** this specification:

- **Scoring.** A reader returns postings and the global statistics required by
  BM25 and similar models. Which model to apply, and how to tune it, is a
  property of the corpus, not of the format.
- **Tokenization.** The tokenizer is not specified. It is *recorded* (§6) so
  that indexing is reproducible, but the format does not mandate a particular
  analyzer. This matters most for languages without word boundaries, where
  morphological and n-gram analysis are both legitimate and neither can be
  standardized without excluding the other.
- **Query planning.** Boolean evaluation order, skipping strategy, and caching
  policy are implementation concerns.

This separation is deliberate. It mirrors the decision in PMTiles to place
rendering outside the tile format: the parts of the problem that are corpus- or
application-dependent are the parts that block standardization, so they are
placed outside the contract.

### 1.1 What this specification adds

The component techniques used here — finite state transducers, delta plus
bit-packing, roaring bitmaps, block compression — are all pre-existing. The
contribution is:

1. A **stated upper bound on I/O** (§4) for a text search index, in a public
   specification.
2. Treating immutability as a first-class property, which allows global
   corpus statistics to be baked into the header.
3. Structural — not incidental — support for prefix and range queries, because
   the term index is order-preserving.
4. A contract-level separation of dictionary, postings, and payload, together
   with the **monotonic ID rule** (§5.3).
5. **Provenance**: the header records the content hash of the source of truth
   from which the index was derived, making the index a verifiable derivative
   rather than an opaque artifact.

## 2. Conventions

The key words MUST, MUST NOT, REQUIRED, SHALL, SHOULD, SHOULD NOT, MAY, and
OPTIONAL are to be interpreted as described in RFC 2119.

All integers are **unsigned and little-endian** unless stated otherwise.
All offsets are absolute byte offsets from the start of the file.
Reserved fields MUST be written as zero and MUST be ignored on read.

A *reader* is an implementation that consumes a COLex file. A *writer* is an
implementation that produces one.

## 3. Design rules

These are normative constraints on the layout, stated separately because they
explain why the layout is shaped the way it is.

### 3.1 Separation of dictionary, postings, and payload

The term dictionary, the postings, and the per-document payload occupy distinct
sections and MUST NOT be interleaved. A reader MUST be able to resolve a term
without reading any payload bytes.

### 3.2 The monotonic ID rule

The term dictionary maps terms to **dense, sequentially assigned term IDs and
nothing else**. Payload MUST NOT be stored in the dictionary.

This is not stylistic. Measured on a synthetic national address set of 1.48M
keys, an FST whose values are monotonic IDs occupies 2.2 MB; replacing those
values with randomized payload-like values inflates the same structure to
19.8 MB — a factor of nine. The dictionary is small only for as long as its
values are predictable.

### 3.3 Immutability and generations

A COLex file is immutable. There is no in-place update and no delta format.
A new index is a new generation, published as a whole. The header records the
generation and the provenance of the source it was derived from (§6), so a
consumer can verify which source of truth a given generation corresponds to.

### 3.4 Deterministic build

Given identical input and identical generator parameters, a writer MUST
produce a byte-identical file. Provenance is only meaningful if the derivation
can be re-run and checked.

## 4. The I/O contract

This section is the core of the specification.

A reader is assumed to cache the fixed header, the section table, the metadata
section, and the term index. Together these are the *resident set*; they are
sized to be small enough to hold permanently (tens of kilobytes for a typical
index) and to be fetched in a single range request on a cold start.

For a query of `k` terms returning `m` documents whose payload is retrieved:

```
reads = 0                      resident set (header, section table, meta, term index)
      + k                      term blocks
      + k                      postings blocks
      + m'                     payload blocks
```

where `m' <= m`, since documents that are adjacent in document ID order share a
payload block.

**The stated upper bound is `2k + m` range reads, plus one additional read on a
cold start to populate the resident set.**

A reader MAY coalesce range requests for adjacent or nearby regions, which
reduces the count further but never increases it. A conformant reader MUST NOT
exceed the bound above.

The bound is `O(k)`, linear in the number of query terms. Precomputed postings
for frequent term pairs, which would reduce multi-term queries toward `O(1)`,
are an OPTIONAL extension; section ID `0x0050` is reserved for it. It is not
required, because the cost of building pair postings grows quadratically and a
linear bound is already a sufficient contract.

### 4.1 Why the bound is stated at all

Every existing option either declines to state a bound or cannot state one.
Formats designed for memory mapping assume that a seek is free. A general
key-value store served over range requests — SQLite over HTTP, for instance —
performs an unbounded number of page fetches, because B-tree descent depth and
page locality are not part of its contract. Stating the bound is the thing that
makes an index safe to publish as a static file.

## 5. File layout

```
+--------------------------------------------------+
| Fixed header (64 bytes)                          |
| Section table                                    |
| Metadata (provenance, tokenizer, parameters)     |  resident
| Term index (FST over block boundary keys)        |
+--------------------------------------------------+
| Term blocks (front-coded)                        |
+--------------------------------------------------+
| Postings blocks                                  |
+--------------------------------------------------+
| Payload blocks (document ID order)               |
+--------------------------------------------------+
```

Sections other than the fixed header MAY appear in any order; the section table
is authoritative. The order above is RECOMMENDED, because it places the
resident set contiguously at the head of the file, so a cold start can populate
it with one range request.

### 5.1 Fixed header

64 bytes at offset 0.

| Offset | Size | Field | Description |
|---|---|---|---|
| 0 | 8 | `magic` | ASCII `COLEXv00` |
| 8 | 1 | `spec_major` | 0 for this document |
| 9 | 1 | `spec_minor` | 1 for this document |
| 10 | 2 | `flags` | See §5.1.1 |
| 12 | 4 | `section_count` | Number of entries in the section table |
| 16 | 8 | `num_docs` | Number of documents, `N` |
| 24 | 8 | `sum_doclen` | Sum of document lengths in tokens |
| 32 | 8 | `section_table_offset` | Absolute offset of the section table |
| 40 | 4 | `section_table_len` | Length of the section table in bytes |
| 44 | 20 | reserved | MUST be zero |

A reader MUST reject a file whose `magic` does not match, and MUST reject a
file whose `spec_major` it does not implement. A reader MUST accept a file
whose `spec_minor` is greater than the one it implements, ignoring sections it
does not recognize.

`num_docs` and `sum_doclen` are the global statistics required by BM25.
The average document length is `sum_doclen / num_docs`, computed by the reader.
It is stored as two integers rather than as a precomputed float so that the
header has no floating-point representation to disagree about.

#### 5.1.1 Flags

| Bit | Name | Meaning |
|---|---|---|
| 0 | `TERMDICT_MONOLITHIC` | The term dictionary is a single monolithic FST (§5.4.2) rather than the blocked form |
| 1 | `HAS_PAYLOAD` | A payload section is present |
| 2-15 | reserved | MUST be zero |

### 5.2 Section table

`section_count` entries of 24 bytes each.

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | `section_id` |
| 2 | 1 | `codec` |
| 3 | 5 | reserved |
| 8 | 8 | `offset` |
| 16 | 8 | `length` |

Entries MUST be sorted by `section_id` and each `section_id` MUST appear at
most once.

`codec` values: `0` raw, `1` zstd, `2` zstd with the shared dictionary from
section `0x0040`. A `codec` other than `0` applies to the section as a whole
and therefore MUST NOT be used for any section that a reader is required to
address at block granularity.

Section IDs:

| ID | Name | Required |
|---|---|---|
| `0x0001` | Metadata | yes |
| `0x0010` | Term index | yes |
| `0x0011` | Term blocks | yes, unless `TERMDICT_MONOLITHIC` |
| `0x0012` | Term block offsets | yes, unless `TERMDICT_MONOLITHIC` |
| `0x0020` | Postings | yes |
| `0x0030` | Payload | if `HAS_PAYLOAD` |
| `0x0031` | Payload offsets | if `HAS_PAYLOAD` |
| `0x0040` | Shared zstd dictionary | if any `codec` is 2 |
| `0x0050` | Term pair cache | reserved, not defined in this version |
| `0x0060` | Positions | reserved, not defined in this version |

IDs below `0x8000` are reserved for this specification. IDs at or above
`0x8000` are available for application-specific use and MUST be ignored by a
reader that does not recognize them.

### 5.3 Term dictionary

The term dictionary is two layers: an FST over block boundary keys, and a
sequence of front-coded term blocks.

A single monolithic FST is not sufficient on its own to meet the bound in §4.
Traversing an FST follows arbitrary internal offsets, so over a range-read
transport the number of fetches grows with the length of the term being looked
up, and no bound can be stated. Splitting the dictionary into blocks and
indexing only the block boundary keys restores a constant bound: the FST is
small enough to be resident, and resolving a term costs exactly one block read.

The FST is order-preserving, so prefix and range queries fall out of the
structure rather than being added as a feature. This is what makes hierarchical
keys — addresses, paths, statute numbers — a natural fit.

#### 5.3.1 Term index (`0x0010`)

An FST mapping the first term of each block to the block ordinal. A lookup for
term `t` finds the greatest boundary key `<= t`; its value is the ordinal of
the block that would contain `t`.

#### 5.3.2 Term blocks (`0x0011`) and offsets (`0x0012`)

Terms are sorted by byte value. Each block holds terms sharing a prefix with
their predecessor, encoded as a shared-prefix length plus a suffix.

Each entry carries:

| Field | Description |
|---|---|
| `shared_len` | Bytes shared with the previous term in the block |
| `suffix` | Remaining bytes of the term |
| `term_id` | Dense sequential term ID |
| `doc_freq` | Number of documents containing the term |
| `postings_offset` | Absolute offset of this term's postings |
| `postings_len` | Length in bytes of this term's postings |

The entry carries the postings location **directly**. There is deliberately no
separate term-ID-to-postings-offset table, because consulting one would cost an
extra range read per term and break the bound in §4.

Section `0x0012` maps block ordinal to `(offset, length)` and is part of the
resident set.

#### 5.3.3 Monolithic mode

When `TERMDICT_MONOLITHIC` is set, section `0x0010` holds a single FST over all
terms and sections `0x0011` and `0x0012` are absent. This mode is intended for
local memory-mapped use, where a seek is free and zero-copy traversal is worth
more than a bounded fetch count. **A file in this mode does not satisfy §4** and
MUST NOT be published as a range-read target.

### 5.4 Postings (`0x0020`)

The postings for one term are a self-contained run of bytes beginning with a
one-byte codec identifier.

| Codec | Name | Status |
|---|---|---|
| `0` | delta plus bit-packing | REQUIRED |
| `1` | roaring bitmap | OPTIONAL |

Every reader MUST implement codec `0`. Placing the identifier in the run,
rather than fixing a single codec for the whole format, allows dense term
postings to use a bitmap without requiring every reader to carry that code.

Document IDs within a run are strictly increasing. A run stores document IDs
and, when term frequencies are present, one frequency per document.

### 5.5 Payload (`0x0030`, `0x0031`)

Payload is stored in document ID order and compressed per block, so that
documents that are adjacent in ID order are retrieved together. Section
`0x0031` maps a document ID to its block and its index within that block.

Payload is where general-purpose compression pays for itself: coordinates
delta-encoded against their block predecessor compress extremely well, whereas
the term dictionary is already small for structural reasons and gains little.

### 5.6 Compression

Whole-section compression MUST NOT be applied to any section that is addressed
at block granularity, since it would defeat range reads entirely. Compression
is applied per block, optionally against the shared dictionary in section
`0x0040`.

This specification does not use a seekable compression framing. Block offsets
are already carried by the offset sections, which makes a second framing layer
redundant.

## 6. Metadata (`0x0001`)

Canonical CBOR (RFC 8949 §4.2). A reader MUST tolerate unknown keys.

| Key | Type | Required | Description |
|---|---|---|---|
| `generation` | uint | yes | Monotonic generation number |
| `source` | array | yes | One entry per source of truth |
| `source[].content_hash` | text | yes | Multihash of the source content |
| `source[].uri` | text | no | Where the source can be obtained |
| `generator.name` | text | yes | Writer implementation |
| `generator.version` | text | yes | Writer version |
| `generator.params` | map | yes | Full build parameters |
| `tokenizer.id` | text | yes | Identifier from the tokenizer registry |
| `tokenizer.params` | map | yes | Tokenizer parameters |
| `attrs` | map | no | Application-defined |

`generator.params` and `tokenizer.params` together with the source content
hashes MUST be sufficient to reproduce the file byte-for-byte (§3.4).

`tokenizer.id` is drawn from a registry published alongside this specification.
The registry standardizes identifiers; it does not standardize which tokenizer
an index must use.

## 7. Conformance

An implementation is conformant if it passes the suite in `conformance/`.
The suite asserts, for each case, both the query result **and the number of
range reads performed**. The read count is part of the contract, so it is
checked the same way correctness is.

A writer is conformant if building the same input twice produces identical
bytes, and if the resulting file is accepted by a conformant reader.

## 8. Security considerations

A COLex file is untrusted input. Every offset and length in the header, the
section table, and the offset sections is attacker-controlled. A reader MUST
validate that every offset and length lies within the file before use, MUST NOT
allocate based on a length field without bounding it against the remaining file
size, and MUST fail cleanly rather than panic on malformed input.

## Appendix A. Decision log

Recorded because the reasoning is the part that is hard to reconstruct later.

**Multi-term I/O bound is `O(k)`; pair precomputation is optional.**
Making precomputed pair postings mandatory would impose a quadratic build cost
for a benefit that address lookup does not need. A linear bound is still a
bound, which is the property that was missing from the field.

**The tokenizer is outside the specification but recorded in metadata.**
Determinism, which is what provenance actually requires, is satisfied by
recording the tokenizer and its parameters. Mandating one would either exclude
morphological analysis or exclude n-grams, and the same standardization
pressure that argues for removing scoring from the format applies here.

**One required postings codec, with an identifier for others.**
A single fixed codec ages badly; unrestricted pluggability destroys
interoperability. One mandatory codec plus a registered identifier keeps every
file readable by every reader while leaving room to add a better one.

**No seekable compression framing.**
Block offsets already exist. A second framing layer would add a dependency and
a second source of truth for the same information.

**No positions in this version.**
Address lookup does not need phrase queries. Section `0x0060` is reserved so
that adding them later is not a breaking change.

**The term dictionary is blocked rather than a single FST.**
See §5.3. A monolithic FST cannot satisfy the I/O bound over a range-read
transport, and the I/O bound is the point of the format. The FST remains, one
level up, as the index over block boundaries, which preserves the ordering
properties that motivated it.
