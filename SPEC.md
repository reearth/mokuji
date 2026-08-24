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

### 4.1 Prefix and range queries

A prefix query returning `n` terms costs:

```
reads = b                      term blocks spanned by the matching range
      + 1                      postings, if the terms' postings are wanted
      + p                      payload blocks
```

`b` is at most `ceil(n / terms_per_block) + 1`. Whether the range continues past
a block is answerable from the resident term index alone — a block whose first
term does not share the prefix contains nothing that does — so only the first
block is read on faith.

The single postings read is a consequence of the layout rather than an
optimization: runs are written in term order, so terms adjacent in the
dictionary have adjacent runs, and a prefix match is exactly such a range.

Prefix and range queries are not a feature layered on the dictionary. They
follow from its being sorted, which is what makes hierarchical keys — addresses,
paths, statute numbers — the case the format handles best rather than a case it
tolerates.

### 4.2 Why the bound is stated at all

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
| 2 | `HAS_FREQS` | Postings runs carry a term frequency per document |
| 3-15 | reserved | MUST be zero |

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

#### 5.3.1 Common encodings

`varint` is LEB128: seven payload bits per byte, least significant group
first, with the high bit set on every byte but the last. An encoding longer
than ten bytes, or one whose final byte would overflow a `u64`, MUST be
rejected.

`front-coded key` is `varint shared_len`, `varint suffix_len`, then `suffix_len`
bytes. The key is the first `shared_len` bytes of the preceding key followed by
the suffix. The first key in a run has `shared_len` zero. A `shared_len`
exceeding the length of the preceding key MUST be rejected.

Front-coding is why entries within a block are decoded sequentially rather than
binary-searched: a key is only meaningful relative to its predecessor. Blocks
are sized so that this scan stays cheap; the search across blocks, which is the
part that would grow with the corpus, happens in the resident term index
instead.

#### 5.3.2 Term index (`0x0010`)

The first term of every block, in order. Resident, and decoded sequentially.

| Field | Encoding |
|---|---|
| `block_count` | `u32` |
| `boundary_key` × `block_count` | front-coded key |

A block ordinal is a position in this list, not a stored value. To locate term
`t`, find the greatest boundary key less than or equal to `t`; its position is
the ordinal of the only block that can contain `t`. If `t` precedes the first
boundary key, `t` is absent.

Keys MUST be strictly increasing. A reader MUST reject a term index that is not,
since the search above would otherwise silently return the wrong block.

#### 5.3.3 Term block offsets (`0x0012`)

`block_count + 1` values of `u64`, absolute offsets, strictly increasing. Block
`i` occupies `offsets[i]..offsets[i + 1]`. Resident.

Storing one boundary per block rather than an offset and a length halves the
section and makes the blocks provably contiguous and non-overlapping.

#### 5.3.4 Term blocks (`0x0011`)

One block is one range read. Its layout:

| Field | Encoding |
|---|---|
| `entry_count` | `varint` |
| `first_term_id` | `varint` |
| entries × `entry_count` | below |

Each entry:

| Field | Encoding |
|---|---|
| `term` | front-coded key, relative to the previous entry in this block |
| `doc_freq` | `varint`, number of documents containing the term |
| `postings_offset` | `varint`; first entry absolute, later entries the delta from the previous entry's offset |
| `postings_len` | `varint` |

The term ID of entry `j` is `first_term_id + j`. It is not stored per entry:
IDs are dense and assigned in term order, so position carries them. This is the
monotonic ID rule (§3.2) in its concrete form — the dictionary holds terms,
lengths, and locations, and nothing that varies independently of term order.

The entry carries the postings location **directly**. There is deliberately no
term-ID-to-postings-offset table, because consulting one would cost an extra
range read per term and break the bound in §4. Postings runs are written in
term order, so the offset deltas are small and positive.

Terms MUST be strictly increasing across the whole dictionary, blocks MUST
partition the terms in order, and the first term of block `i` MUST equal
boundary key `i`.

#### 5.3.5 Monolithic mode

When `TERMDICT_MONOLITHIC` is set, section `0x0010` holds a single finite state
transducer over all terms, and sections `0x0011` and `0x0012` are absent. The
transducer encoding is not defined in this version.

This mode is for local memory-mapped use, where a seek is free and zero-copy
traversal is worth more than a bounded fetch count. **A file in this mode does
not satisfy §4** and MUST NOT be published as a range-read target.

### 5.4 Postings (`0x0020`)

The postings for one term are a self-contained run, located by its dictionary
entry and beginning with a one-byte codec identifier.

| Codec | Name | Status |
|---|---|---|
| `0` | delta with bit-packing | REQUIRED |
| `1` | roaring bitmap | OPTIONAL |

Every reader MUST implement codec `0`. Carrying the identifier in the run,
rather than fixing one codec for the file, lets a dense term use a bitmap
without requiring every reader to implement one.

#### 5.4.1 Bit-packed groups

A sequence of `n` values is encoded as `ceil(n / 128)` groups of up to 128
values. Each group is:

| Field | Encoding |
|---|---|
| `bit_width` | `u8`, 0 to 64 |
| `packed` | `ceil(group_len * bit_width / 8)` bytes |

Values are packed least significant bit first: value `j` of the group occupies
bits `j * bit_width` to `(j + 1) * bit_width` of the group's bit stream, where
bit `i` is bit `i mod 8` of byte `i / 8`. Padding bits in the final byte MUST be
zero.

`bit_width` MUST be the smallest width that represents every value in the
group. A width of zero encodes a group whose values are all zero and occupies
no bytes at all — the common case for a run of consecutive document IDs, and
the reason the widths are chosen per group rather than per run.

Requiring the minimal width, rather than merely permitting it, is what makes
the encoding canonical, which is what lets §3.4 require byte-identical rebuilds.

#### 5.4.2 Codec 0

| Field | Encoding |
|---|---|
| `codec` | `u8`, value 0 |
| `doc_count` | `varint` |
| document values | bit-packed groups, `doc_count` values |
| frequency values | bit-packed groups, `doc_count` values; present only if `HAS_FREQS` |

Document IDs are strictly increasing. The encoded values are the first document
ID, then `doc_id[i] - doc_id[i - 1] - 1` for each subsequent document. Gaps are
reduced by one because the difference is at least one, which turns a run of
consecutive documents into a run of zeros and hence into a group of width zero.

Frequency values are `frequency - 1`, for the same reason: a term that appears
once in every document it occurs in — the ordinary case for an address index —
packs to nothing.

A reader MUST reject a run whose decoded document IDs are not strictly
increasing or exceed `num_docs`.

### 5.5 Payload (`0x0030`, `0x0031`)

Payload is an opaque byte string per document. **Its contents are outside this
specification**; only the framing is defined. An address index stores
coordinates, a document index stores a title and a URL, and the format does not
need to know which.

Payload is stored in document ID order and grouped into fixed-size blocks, so
that documents adjacent in ID order are retrieved together. This is what makes
the `m'` term of §4 smaller than the number of results.

#### 5.5.1 Payload offsets (`0x0031`)

| Field | Encoding |
|---|---|
| `docs_per_block` | `u32`, non-zero |
| `block_count` | `u32` |
| `offsets` | `u64` × (`block_count` + 1), absolute, strictly increasing |

Document `d` lives in block `d / docs_per_block` at index `d mod docs_per_block`.
A fixed block size means the mapping is arithmetic rather than a lookup, so this
section stays resident even for a large corpus.

#### 5.5.2 Payload blocks (`0x0030`)

| Field | Encoding |
|---|---|
| `entry_count` | `varint` |
| entries × `entry_count` | `varint len`, then `len` bytes |

Entries are in document ID order starting at `block_ordinal * docs_per_block`.
The final block MAY hold fewer than `docs_per_block` entries.

Payload is where general-purpose compression earns its place: coordinates
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
| `source[].content_hash` | text | yes | `<algorithm>:<lowercase hex>`; `algorithm` is `sha2-256` in this version |
| `source[].uri` | text | no | Where the source can be obtained |
| `generator.name` | text | yes | Writer implementation |
| `generator.version` | text | yes | Writer version |
| `generator.params` | map | yes | Full build parameters |
| `tokenizer.id` | text | yes | Identifier from the tokenizer registry |
| `tokenizer.params` | map | yes | Tokenizer parameters |
| `attrs` | map | no | Application-defined |

`generator.params` and `tokenizer.params` together with the source content
hashes MUST be sufficient to reproduce the file byte-for-byte (§3.4).

`tokenizer.id` is drawn from `TOKENIZERS.md`, published alongside this
specification. That registry standardizes identifiers; it does not standardize
which tokenizer an index must use. Identifiers beginning `x-` are private and
will never be registered.

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

**The required term index encoding is a front-coded key list, not an FST.**
Once the dictionary is blocked, the index holds only block boundary keys —
a small fraction of the terms — so the size advantage of a transducer over a
front-coded list is worth little, while specifying a transducer byte-for-byte
is worth a great deal of specification. A format that criticises others for
letting an implementation stand in for a specification cannot then defer its
own central structure to one. The transducer remains available under
`TERMDICT_MONOLITHIC` and may be added later as an alternative index encoding.
The property that motivated it, order preservation, comes from the dictionary
being sorted and is unaffected.

**Minimal bit widths are required, not merely permitted.**
Determinism (§3.4) needs exactly one valid encoding of a given input. A codec
that allows a wider-than-necessary field would allow two files to be equally
valid and differently shaped, which would make a byte-identical rebuild
unverifiable.

**Payload contents are outside the specification.**
The format frames payload and locates it; what is in it is an application's
business. Specifying a coordinate encoding would tie a general format to one
use, and the compression argument for grouping payload by document ID holds
regardless of what the bytes mean.

**The term dictionary is blocked rather than a single FST.**
See §5.3. A monolithic FST cannot satisfy the I/O bound over a range-read
transport, and the I/O bound is the point of the format. The FST remains, one
level up, as the index over block boundaries, which preserves the ordering
properties that motivated it.

## Appendix B. A worked example

`conformance/fixtures/tiny.colex` is 745 bytes and indexes four documents:

```
tokyo chiyoda    35.694,139.754
tokyo chuo       35.671,139.772
tokyo shibuya    35.664,139.698
osaka chuo       34.681,135.513
```

Its full annotated hex dump is in `conformance/fixtures/tiny.hex`. What
follows walks one query through it, `chuo`, using nothing but §5.

**Header**, at offset 0:

```
00000000  43 4f 4c 45 58 76 30 30 00 01 06 00 07 00 00 00  |COLEXv00........|
00000010  04 00 00 00 00 00 00 00 08 00 00 00 00 00 00 00  |................|
00000020  40 00 00 00 00 00 00 00 a8 00 00 00 00 00 00 00  |@...............|
```

Magic `COLEXv00`; version 0.1; flags `0x0006`, so `HAS_PAYLOAD` and
`HAS_FREQS`; seven sections; four documents totalling eight tokens; the section
table is 0xa8 bytes at offset 0x40.

**Term index** (`0x0010`), at offset 509:

```
000001fd  03 00 00 00 00 07 63 68 69 79 6f 64 61 00 05 6f  |......chiyoda..o|
0000020d  73 61 6b 61 00 05 74 6f 6b 79 6f                 |saka..tokyo|
```

Three blocks, then three front-coded keys: `chiyoda`, `osaka`, `tokyo`, each
with `shared_len` zero because they share no prefix. The greatest key not
exceeding `chuo` is `chiyoda`, so block 0 is the only block that can hold it.

**Term block offsets** (`0x0012`), at 536: block 0 spans `0x258..0x26e`.

**Term block 0**, at 600, 22 bytes:

```
00000258  02 00 00 07 63 68 69 79 6f 64 61 01 94 05 04 02  |....chiyoda.....|
00000268  02 75 6f 02 04 05                                |.uo...|
```

Two entries, first term ID 0.

Entry 0: `shared_len` 0, `suffix_len` 7, `chiyoda`; `doc_freq` 1; postings
offset `94 05`, the first entry so absolute — 0x14 plus 5 shifted by seven is
660; `postings_len` 4.

Entry 1: `shared_len` 2, `suffix_len` 2, `uo` — the term is `ch` + `uo`, which
is the front-coding paying for itself on a two-term block; `doc_freq` 2;
postings offset delta 4, so 664; `postings_len` 5. Its term ID is 0 + 1 = 1,
carried by position rather than stored.

**Postings** for `chuo`, at 664, 5 bytes:

```
00000298  00 02 01 03 00                                   |.....|
```

Codec 0; two documents. The document group has `bit_width` 1 and one byte,
`0x03`, which is the bits `1, 1`. The first value is a document ID and the rest
are gaps less one, so the documents are 1 and 1 + 1 + 1 = 3. The frequency
group has `bit_width` 0 and therefore no bytes at all: every frequency is 1,
stored as 0.

Five bytes for a two-document posting list with frequencies, and the
frequencies were free.

**Cost.** One term block read, one postings read, and one payload read per
distinct payload block. For `tokyo AND chuo`, which matches document 1: four
reads plus one, against a bound of `2k + m` = 5.
