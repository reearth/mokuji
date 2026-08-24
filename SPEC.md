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

### 4.2 Where the dictionary lives

The bound above assumes a reader fetches a term block when it needs one. A
reader MAY instead hold the whole term blocks section, as stored, and decompress
one block per lookup. Then a term costs **no range read at all**, and a query
costs only its payload reads.

Both are conformant and neither is a different file. Blocks are independently
decodable and the offsets that locate them are resident either way, so this is a
deployment choice rather than a format one:

| | resident set | term lookup | typical query |
|---|---|---|---|
| Fetched dictionary | tens to hundreds of KB | 1 read per block | 2 reads |
| Resident dictionary | plus the dictionary's stored size | 0 reads | 1 read |

On 1,744,962 Tokyo addresses at 1024 terms a block, the dictionary is 864 KB
stored, so holding it costs one read at open and under a megabyte of memory,
and a prefix query drops from 2 reads and 725 bytes to 1 read and 220. Holding
it *as stored* matters: decompressed it would be 16 MB, and decompressing one
9 KB block per lookup is not a cost worth twenty times the memory to avoid.

A writer influences the choice through `terms_per_block`, since that sets how
large the dictionary is and how much a single lookup fetches. It does not make
the choice.

### 4.3 Why the bound is stated at all

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
| Term index (block boundary keys)                 |
| Term block offsets                               |
| Payload offsets                                  |
+--------------------------------------------------+
| Postings                                         |
+--------------------------------------------------+
| Term blocks (front-coded)                        |
+--------------------------------------------------+
| Payload blocks (document ID order)               |
+--------------------------------------------------+
```

Sections other than the fixed header MAY appear in any order; the section table
is authoritative. The order above is RECOMMENDED for two reasons.

It places the resident set contiguously at the head, so a cold start can
populate it with one range request.

And it places postings **before** the term blocks that point into them, so that
a postings offset is known before any term block is built and a writer can emit
the file in one pass. With the opposite order each section's size depends on
the other's, and once blocks are compressed that dependency has no fixed point
to converge on: a changed offset changes a varint, which changes what the
compressor emits, which moves the offset again.

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

##### Implied postings

A `postings_len` of zero means the term occurs in **exactly one document, whose
ID is the term's own ID, with frequency one**. No postings run is stored and
none is read.

Such an entry MUST have `doc_freq` of 1, and its term ID MUST be less than
`num_docs`. A reader MUST reject an entry that does not.

Its `postings_offset` is not used by a reader. A writer MUST still encode the
field as the running position — an implied entry simply consumes no bytes — so
that the offset chain stays a plain running total and the encoding rule has no
special case.

This is what an index of a dictionary rather than a document collection looks
like: an address, a path, or a statute number occurs in exactly the one record
that is about it. Written out, such postings say only "term *i* names document
*i*" — measured at 300,000 addresses, 2.0 MB saying nothing, in a section that
cannot be compressed because its addressed unit is a single run (§5.6). Stating
the equivalence in the dictionary removes both the bytes and one range read per
query.

A writer MAY use implied postings for any term whose postings are exactly the
document with that term's ID at frequency one, and MUST write a normal run
otherwise. It is decided per term, so an index whose terms mostly name one
record each still benefits from the terms that do.

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

A term whose postings are implied (§5.3.4) has no run at all. If every term in
an index has implied postings, the postings section is present with a length of
zero.

### 5.5 Payload (`0x0030`, `0x0031`)

Payload is stored in document ID order and grouped into fixed-size blocks, so
that documents adjacent in ID order are retrieved together. This is what makes
the `m'` term of §4 smaller than the number of results.

**What payload means is outside this specification. How it is written down is
not.** An address index stores coordinates and a document index stores a title;
the format does not need to know which, and saying so would tie a general
format to one use. But an encoding is not a meaning, and declining to specify
one leaves every index storing numbers as text.

The line is the one PMTiles draws around a tile: it declares the type and the
compression without saying what the picture is of.

#### 5.5.1 Payload offsets (`0x0031`)

| Field | Encoding |
|---|---|
| `payload_codec` | `u8`, see below |
| `values_per_doc` | `u8`, codec 1 only; zero otherwise |
| `scale_exponent` | `i8`, codec 1 only; zero otherwise |
| reserved | `u8`, MUST be zero |
| `docs_per_block` | `u32`, non-zero |
| `block_count` | `u32` |
| `offsets` | `u64` × (`block_count` + 1), absolute, strictly increasing |

Document `d` lives in block `d / docs_per_block` at index `d mod docs_per_block`.
A fixed block size means the mapping is arithmetic rather than a lookup, so this
section stays resident even for a large corpus.

| Codec | Name | Status |
|---|---|---|
| `0` | opaque | REQUIRED |
| `1` | delta-coded integers | REQUIRED |

#### 5.5.2 Codec 0, opaque

A block is:

| Field | Encoding |
|---|---|
| `entry_count` | `varint` |
| entries × `entry_count` | `varint len`, then `len` bytes |

Entries are in document ID order starting at `block_ordinal * docs_per_block`.
The final block MAY hold fewer than `docs_per_block` entries. The bytes are
returned as they were stored and mean nothing to the format.

#### 5.5.3 Codec 1, delta-coded integers

Each document carries exactly `values_per_doc` signed integers. A block is:

| Field | Encoding |
|---|---|
| `entry_count` | `varint` |
| entries × `entry_count` | `values_per_doc` zigzag `varint`s |

Zigzag maps a signed value to an unsigned one as `(v << 1) ^ (v >> 63)`, so that
a small negative number stays small.

The first entry of a block stores its values as they are. Every later entry
stores each value as the difference from the value in the same position of the
entry before it. Deltas do not cross a block boundary, because a block is
addressed on its own and must decode on its own.

`scale_exponent` records that a stored integer `v` stands for `v × 10^exponent`.
A reader MUST NOT interpret it further. It is the difference between an encoding
and a meaning: the format states how a number was written down, and says nothing
about what it measures or how accurate it is.

##### Why delta

Documents are in term order, and for a hierarchical key that ordering is not
arbitrary. Addresses that sort together are near each other on the ground, so
consecutive documents differ only in their low digits. Measured on 1,744,962
Tokyo addresses (`tools/payload-encodings.py`), per address after per-block
compression:

| | per address |
|---|---|
| Coordinates as published text | 8.69 B |
| The same rounded to six decimals | 5.61 B |
| Fixed point, one `int32` pair | 5.05 B |
| **Fixed point, delta-coded** | **3.39 B** |

The monotonic ID rule (§3.2) was adopted to keep the dictionary small. It pays a
second time here, in a section that does not know the dictionary exists.

### 5.6 Compression

Compression is applied per **addressed unit**: one block of a block-addressed
section, or the whole of a section that is read in one go. Never across the
file, and never across a run of blocks — compressing more than a reader can
address would force it to fetch bytes it does not want, which is the thing this
format exists to avoid.

The codec is recorded per section (§5.2). A compressed unit is:

| Field | Encoding |
|---|---|
| `decompressed_len` | `varint` |
| `payload` | codec-specific |

The length comes first so that a reader can bound the allocation before making
it, rather than discovering the size mid-stream from attacker-controlled input
(§8). A reader MUST reject a unit whose output does not match its declared
length, and MAY refuse a declared length larger than it is willing to allocate.

Offsets in the offset sections address **compressed** bytes, which is what makes
a block reachable without decompressing anything before it.

The **postings section MUST be stored raw**. Its addressed unit is a single
term's run — a handful of bytes — so compressing each would enlarge the section,
and it would break the property that a contiguous range of terms can be fetched
in one request (§4.1).

This specification does not use a seekable compression framing. Block offsets
are already carried by the offset sections, which makes a second framing layer
redundant.

#### 5.6.1 What the codec choice costs

Measured on 300,000 Japanese addresses (`demo/addresses`), per block:

| | term blocks | payload |
|---|---|---|
| deflate | 7.06x | 4.38x |
| zstd | 7.20x | 6.12x |
| zstd with a trained shared dictionary | 27.2x | 7.51x |
| deflate with a preset dictionary | 24.3x | — |

Two things follow. Between deflate and zstd without a dictionary the difference
is small — noise on the dictionary, 1.4x on payload. Against that, a shared
dictionary is worth 3.4x, and it is worth roughly as much to deflate as to zstd:
**the dictionary matters far more than the algorithm**.

Deflate is therefore the required codec. It is not the best compressor; it is
the one every language runtime already has and every browser decompresses
natively, so a reader in a page needs no bundled decompressor. zstd is
registered as optional for publishers who would rather have the payload ratio
than the reach.

A dictionary-using codec is the obvious next addition, and section `0x0040` is
reserved for the dictionary it would need. It is deliberately not in this
version: browsers cannot supply a preset dictionary through their native
decompression, so a dictionary codec requires a decompressor in the page no
matter which algorithm it uses, and that trade is worth making deliberately
rather than by default.

## 6. Metadata (`0x0001`)

Canonical CBOR (RFC 8949 §4.2). A reader MUST tolerate unknown keys.

| Key | Type | Required | Description |
|---|---|---|---|
| `generation` | uint | yes | Monotonic generation number |
| `source` | array | yes | One entry per artifact the index was derived from |
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

A source entry names an artifact the index was derived from. That is usually a
source of truth — a register, a corpus — but an index is often produced from
one by a program, and a chain that names the register and the result while
omitting the step between them is checkable only in parts. Recording the
transform's own hash as a source closes it, and needs no other mechanism.

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

*Corrected by measurement.* The argument above is about the **index**, where it
holds. It was then allowed to settle the question for the **dictionary**, where
it does not. Built over the 1,744,962 addresses of Tokyo at residence level
(`tools/fst-comparison`):

| | bytes | per term |
|---|---|---|
| Blocked front-coding, deflated per block | 2,765,050 | 1.58 |
| `fst::Set` | 571,926 | 0.33 |
| `fst::Map` carrying the rank | 733,494 | 0.42 |

A transducer looks **4.8x smaller**, and the mechanism is real: minimization
shares the *suffixes* as well as the prefixes, so the `1番1号 … 30番20号` tail
that hangs off thousands of different neighbourhoods is stored once as a shared
subgraph. Front-coding shares only the prefix, and per-block deflate recovers
the rest only within a block's window.

*Corrected again.* That comparison is against 128 terms a block, a default
chosen before there was any real data to choose it on. The window is the
variable, and widening it is free:

| terms per block | dictionary | per term | bytes per query |
|---|---|---|---|
| 128 | 2,755,118 | 1.58 | 435 |
| 1024 | 864,419 | 0.50 | 725 |
| 2048 | 669,538 | 0.38 | 965 |
| 4096 | 552,694 | 0.32 | 1,471 |

At 2048 terms a block the blocked dictionary is smaller than `fst::Map`, and at
4096 smaller than `fst::Set`. Larger blocks also *reduce* the read count for a
query with many results, since more of them land in one block. What they cost is
bytes per lookup, and the file is smaller besides.

So the size argument for a transducer does not survive tuning. What survives is
narrower: a transducer is queried in place, while a blocked dictionary held in
memory is held compressed and decompresses a block per lookup. At 864 KB stored
against 16 MB decompressed, holding it compressed is obviously right, and the
per-lookup decompression is microseconds — so even that advantage is small.

The measurement stands, the mechanism stands, and the conclusion is that the
specification cost is not worth paying. §4.2 gets the property that a transducer
was wanted for.

**Implied postings are a dictionary entry, not a postings codec.**
A codec that encoded "this term names its own document" compactly would still
cost a byte per term and, worse, a range read per query to fetch that byte. The
saving worth having is not writing it down at all, and the place to say so is
the entry that is already being read.

It is stated per term rather than as a header flag so that an index whose terms
mostly name one record each still benefits from the terms that do, without
splitting the format into a dictionary mode and a document mode.

That the term and document ID spaces coincide for such a term is a recorded,
checkable equivalence rather than a coupling. §3.1 requires that a reader
resolve a term without touching payload; nothing there asks two identifier
spaces never to agree, and a reader that does not implement the case still
reads every other index correctly.

**Deflate is the required codec, not zstd.**
Measured, the two are within noise on the term dictionary and 1.4x apart on
payload, while a shared dictionary is worth 3.4x to either. Since the algorithm
choice buys so little, it is decided on reach rather than ratio: every language
runtime has zlib and every browser decompresses raw deflate natively, so a
reader in a page needs nothing bundled. zstd is registered as optional, and a
dictionary-using codec is left for a later version because it costs a
decompressor in the page whichever algorithm it uses.

**Postings are placed before the term blocks that point into them.**
Term block entries carry absolute postings offsets, so with the opposite order
each section's size depends on the other's. Uncompressed that converges after a
round or two; compressed it need not converge at all, since a changed offset
changes a varint, which changes what the compressor emits, which moves the
offset again. Ordering the file so it can be written in one pass is better than
iterating toward a layout that may not settle.

**Minimal bit widths are required, not merely permitted.**
Determinism (§3.4) needs exactly one valid encoding of a given input. A codec
that allows a wider-than-necessary field would allow two files to be equally
valid and differently shaped, which would make a byte-identical rebuild
unverifiable.

**Payload contents are outside the specification; payload encodings are not.**
The format frames payload and locates it; what is in it is an application's
business, and specifying a coordinate encoding would tie a general format to one
use. That argument covers meaning. It does not cover encoding, and letting it
stand for both left every index storing numbers as decimal text — measured, 2.6x
larger than the same numbers delta-coded, which is more than rounding them away
to a precision nobody claimed would save.

So the line moves to where PMTiles draws it: declare the encoding, say nothing
about the subject. A codec identifier and a scale, and no opinion about what is
being scaled.

The precision question turns out not to be beyond the format either, once it is
read as provenance rather than compression. An index that rounds and records
that it rounded is a shortened copy that says so; one that rounds silently is a
damaged one. `generator.params` is where that belongs.

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

Its full annotated hex dump is in `conformance/fixtures/tiny.hex`. The fixture
is stored raw, so that it can be read against §5 by hand. What follows walks one
query through it, `chuo`.

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

**Term block offsets** (`0x0012`), at 536: block 0 spans `0x26f..0x285`.

**Term block 0**, at 623, 22 bytes:

```
0000026f  02 00 00 07 63 68 69 79 6f 64 61 01 dc 04 00 02  |....chiyoda.....|
0000027f  02 75 6f 02 00 05                                |.uo...|
```

Two entries, first term ID 0.

Entry 0: `shared_len` 0, `suffix_len` 7, `chiyoda`; `doc_freq` 1;
`postings_offset` `dc 04`, the first entry so absolute — 0x5c plus 4 shifted by
seven is 604; and `postings_len` **0**. The length is what matters: this term's
postings are implied (§5.3.4). It occurs in exactly one document, document 0,
because its own term ID is 0. Nothing is stored and nothing is read.

Entry 1: `shared_len` 2, `suffix_len` 2, `uo` — the term is `ch` + `uo`, which
is the front-coding paying for itself on a two-term block; `doc_freq` 2;
`postings_offset` delta 0, because the entry before it consumed no bytes, so
604 again; `postings_len` 5. Its term ID is 0 + 1 = 1, carried by position
rather than stored.

**Postings** for `chuo`, at 608, 5 bytes:

```
00000260  00 02 01 03 00                                   |.....|
```

Codec 0; two documents. The document group has `bit_width` 1 and one byte,
`0x03`, which is the bits `1, 1`. The first value is a document ID and the rest
are gaps less one, so the documents are 1 and 1 + 1 + 1 = 3. The frequency
group has `bit_width` 0 and therefore no bytes at all: every frequency is 1,
stored as 0.

Five bytes for a two-document posting list with frequencies, and the
frequencies were free.

**Payload offsets** (`0x0031`), at 568:

```
00000238  00 00 00 00 02 00 00 00 02 00 00 00 ab 02 00 00  |................|
```

Payload codec 0, opaque, so no value count and no scale; two documents to a
block; two blocks; then the offsets. Document 1 is at index 1 of block 0.

**Cost.** One term block read, one postings read, and one payload read per
distinct payload block. For `tokyo AND chuo`, which matches document 1: four
reads plus one, against a bound of `2k + m` = 5.

For `chiyoda` alone: one term block read and one payload read. The postings
read does not happen, because there were no postings to read.
