# Mokuji Format Specification

**Version:** 0.1 (draft — not frozen, subject to breaking change)
**Status:** Draft
**Date:** 2026-08-24
**Editor:** Eukarya Inc.

> Mokuji — a cloud-optimized concordance format.
> Single immutable file, bounded range reads, no server.
>
> 目次: the table of contents. The part of a book that is not the book, and
> that lets you open it at the right page without reading the rest.

This document is licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

---

## 1. Scope

Mokuji defines a single-file, immutable, byte-addressable layout for an inverted
index that is intended to be served as a static object over HTTP range requests.

A Mokuji file answers exactly one question: **for a set of query terms, which
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

### 1.2 One layout, two shapes

The exclusions above are not modesty. They are what lets a single layout hold
two indexes that have almost nothing else in common.

A dictionary of addresses has one term per record, no postings worth storing,
payload that is a pair of numbers, and a prefix for a query. A collection of
documents has thousands of terms per record, postings that dominate the file,
payload that is a path, and a ranked intersection for a query. Measured, their
section profiles are inverted: postings are 0% of one and 63% of the other,
payload 84% and 1%.

Every one of those differences falls in a place this specification declined to
decide — what a term is, what payload means, whether a term's postings are worth
writing down, whether anything is ranked. What the two share is the part that is
specified: a sorted dictionary, postings and payload that can be located without
reading anything else, and the bound in §4.

## 2. Conventions

The key words MUST, MUST NOT, REQUIRED, SHALL, SHOULD, SHOULD NOT, MAY, and
OPTIONAL are to be interpreted as described in RFC 2119.

All integers are **unsigned and little-endian** unless stated otherwise.
All offsets are absolute byte offsets from the start of the file.
Reserved fields MUST be written as zero and MUST be ignored on read.

A *reader* is an implementation that consumes a Mokuji file. A *writer* is an
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

A Mokuji file is immutable. There is no in-place update and no delta format.
A new index is a new generation, published as a whole. The header records the
generation and the provenance of the source it was derived from (§6), so a
consumer can verify which source of truth a given generation corresponds to.

### 3.4 Deterministic build

Given identical input and identical generator parameters, a writer MUST produce
a byte-identical file. Provenance is only meaningful if the derivation can be
re-run and checked.

This is a property of **a** writer, not of the format. Two conformant writers
using different compression libraries produce different, equally valid files
from the same input, because no specification of deflate fixes which of the
valid encodings a compressor emits. That is why the metadata records
`generator.name` and `generator.version` (§6): a rebuild is checkable against
the writer that made it, and the file says which one that was.

## 4. The I/O contract

This section is the core of the specification.

A reader is assumed to cache the fixed header, the section table, the metadata
section, the term index, and the completions section if there is one (§5.3.5).
Together these are the *resident set*; they are sized to be small enough to hold
permanently (tens of kilobytes for a typical index) and to be fetched in a
single range request on a cold start.

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

The bound is `O(k)`, linear in the number of query terms. It is not better than
linear, and deliberately so; see Appendix A.

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

#### 4.1.1 Short prefixes

The bound above holds at any size, but `b` grows with `n`, and a short prefix
matches a great many terms. Measured on 12,854,245 places (`tools/README.md`),
a one-letter prefix spans a median of 114 of 5,698 dictionary blocks, and a
two-letter one 12. That is a bound, and not a small one.

A completions section (§5.3.5) holds the answer to exactly those prefixes,
decided when the file was written. When it declares a coverage of `T` terms and
`S` bytes, a prefix query for at most `docs_per_entry` documents costs:

```
reads = p                      if the prefix has an entry

reads = b                      otherwise, where the prefix matches fewer than T
      + 1                      terms, so b <= ceil(T / terms_per_block) + 1,
      + p                      and the postings read is shorter than S bytes
```

Whether the prefix has an entry is answered by the resident set, so a reader
knows which case it is in before it reads anything. On the same places indexed
one term per name (`name-list`), a section covering `T` = 1,024 and `S` = 16 KB
holds 11,243 prefixes in 165 KB, and no partly typed name among 2,776 tried
cost more than 3 reads before its payload.

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
| Completions, if any                              |
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
| 0 | 8 | `magic` | ASCII `MOKUJIv0` |
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
| 3 | `HAS_POSITIONS` | A positions section is present, and dictionary entries locate it; requires `HAS_FREQS` |
| 4-15 | reserved | MUST be zero |

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

`codec` values are those of §5.6: `0` raw, `1` deflate, `2` zstd. A `codec`
other than `0` applies to the section as a whole and therefore MUST NOT be used
for any section that a reader is required to address at block granularity.

Section IDs:

| ID | Name | Required |
|---|---|---|
| `0x0001` | Metadata | yes |
| `0x0010` | Term index | yes |
| `0x0011` | Term blocks | yes |
| `0x0012` | Term block offsets | yes |
| `0x0013` | Completions | no |
| `0x0020` | Postings | yes |
| `0x0030` | Payload | if `HAS_PAYLOAD` |
| `0x0031` | Payload offsets | if `HAS_PAYLOAD` |
| `0x0032` | Document lengths | no |
| `0x0033` | Document attributes | no |
| `0x0060` | Positions | if `HAS_POSITIONS` |

IDs below `0x8000` are reserved for this specification, and one this version
does not define carries no meaning: a reader MUST ignore a section it does not
recognize rather than assume a use for it. IDs at or above `0x8000` are
available for application-specific use.

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
| `positions_offset` | `varint`, present only if `HAS_POSITIONS`; first entry absolute, later entries the delta from the previous entry's offset |
| `positions_len` | `varint`, present only if `HAS_POSITIONS` |

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

#### 5.3.5 Completions (`0x0013`)

Optional. For a prefix that many terms share, the documents a completion would
show first, chosen when the file was written — so that the question §4.1.1 says
is expensive is answered before it is asked.

The section is read whole and belongs to the resident set. Its codec (§5.2) MAY
compress it, since it is never addressed in parts.

| Field | Encoding |
|---|---|
| `min_terms` | `varint`; see coverage below |
| `max_span` | `varint`; see coverage below |
| `docs_per_entry` | `varint`, non-zero |
| `entry_count` | `varint` |
| entries × `entry_count` | below |

Each entry:

| Field | Encoding |
|---|---|
| `prefix` | front-coded key (§5.3.1), relative to the previous entry |
| `doc_count` | `varint`, 1 to `docs_per_entry` |
| `docs` | `doc_count` zigzag `varint`s (§5.5.3): the first a document ID, each later one the difference from the document before it |

A **prefix** here is any non-empty byte string, and a term *begins with* it when
its first bytes are the prefix. Byte strings rather than characters, because the
dictionary compares bytes and knows nothing of an encoding.

Rules a reader MUST enforce: prefixes are non-empty and strictly increasing;
every listed document is less than `num_docs`; no document is listed twice in
one entry.

Rules a writer MUST follow, which a verifier can check against the rest of the
file and a reader need not:

- **What an entry lists.** Every listed document contains at least one term that
  begins with the prefix, and `doc_count` is `docs_per_entry` or the number of
  such documents, whichever is smaller.
- **Coverage.** When `min_terms` is not zero, every prefix that at least
  `min_terms` terms begin with has an entry. When `max_span` is not zero, every
  prefix whose terms' `postings_len` sum to at least `max_span` bytes has an
  entry. A writer MAY give an entry to any other prefix. Both zero is a promise
  of nothing: the section is then a cache, and §4.1.1 states no bound for it.

**Which** documents an entry lists, and in what order, is the writer's choice
and outside this specification, exactly as scoring is (§1). The order is the one
a reader SHOULD present them in. A writer records how it chose in
`generator.params`, as it records everything else it branched on (§6).

A reader wanting more than `docs_per_entry` documents for a prefix, or a prefix
combined with anything else, is past what the entry answers, and reads the
dictionary and postings as §4.1 describes.

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

A dictionary-using codec is the obvious next addition, and it would need a
section to hold the dictionary. It is deliberately not in this version, and no
identifier is set aside for it: browsers cannot supply a preset dictionary
through their native decompression, so a dictionary codec requires a
decompressor in the page no matter which algorithm it uses. That trade is worth
making deliberately, and reserving an identifier for it in advance would be
deciding it by default.

### 5.6a Positions (`0x0060`)

Optional. Where in a document each of its terms occurs, which is what a phrase
query needs and nothing else does.

One run per term, located by the `positions_offset` and `positions_len` its
dictionary entry carries. A run holds, for each of the term's documents **in the
order the postings run gives them**, bit-packed groups (§5.4.1) of that
document's positions:

| Field | Encoding |
|---|---|
| per document | `frequency` values, the first a position and the rest gaps less one |

The frequency comes from the postings run, so positions are read after postings
and never instead of them — and `HAS_POSITIONS` therefore requires `HAS_FREQS`,
since without frequencies nothing says where one document's positions end and
the next begin. A reader MUST reject a file that sets one without the other. Positions within a document are strictly increasing,
and a reader MUST reject a run whose decoded values are not.

A run is decoded from the front: to reach the fifth document's positions a
reader decodes the four before it. Blocks are not involved, because a phrase
query wants the positions of the term it is testing, not of one document.

#### What positions cost, and what they do not

Measured on 2,523 documents and 5.1 million token occurrences (`demo/documents`),
positions are the largest thing in the file — larger than everything else put
together.

**They cost bytes and not reads.** A run is fetched only by a query that tests a
phrase; every other query never touches the section, and the bound in §4 is
unchanged for them. A phrase query pays one further read per term, so `3k + m`
rather than `2k + m`.

That distinction is the whole argument for carrying them. An index that is
downloaded before it is used pays for positions on every query, whether or not
it asks a question that needs them, which is why formats meant to be downloaded
tend to make positions optional and their users tend to leave them off. An index
that is read in ranges pays only for what it reads.

### 5.7 Document lengths (`0x0032`)

Optional. The length in tokens of every document, in document ID order, as
bit-packed groups (§5.4.1) of exactly `num_docs` values. No header: the count
comes from the header, and the section is either present in full or absent.

The section is read whole, or not at all. A scorer wants the lengths of every
candidate document at once and its candidates are scattered, so fetching them
one at a time would cost more reads than holding them all costs bytes.
Measured on 2,523 documents it is 1.7 bytes a document.

Its presence is the signal that it is available; there is no flag, because
nothing else in the file depends on it.

#### Why the format carries this and not the score

Scoring is outside the specification (§1) because which model is right is a
property of the corpus. What is *not* corpus-dependent is which statistics any
of those models need, and the header alone does not carry them: it gives the
document count and the token total, which yield the mean document length but
not the length of the document actually being scored. BM25 normalizes by that
length per document, and so does almost everything else worth using.

Without it a reader cannot rank, and without ranking it cannot ask for the best
ten — so it fetches the payload of every match. Measured on an index of 2,523
documents, adding the section cost 4,359 bytes and took a three-term query from
39 range reads to 6.

That is the line the format draws: it supplies what a scorer needs and has no
opinion about the scorer.

### 5.8 Document attributes (`0x0033`)

Optional. Named columns of numbers, one value per document, in document ID
order.

| Field | Encoding |
|---|---|
| `column_count` | `varint` |
| per column | `varint name_len`, `name_len` bytes, then `i8 scale_exponent` |
| per column | bit-packed groups (§5.4.1) of exactly `num_docs` zigzag values |

Column names are bytes; ASCII is recommended and nothing is required. Values are
zigzag-encoded (§5.5.3) before packing, so a column may be negative. A stored
`v` stands for `v × 10^exponent`, recorded and not interpreted, exactly as for
payload.

Columns MUST have distinct names, and a reader MUST reject a section whose
values do not fill it exactly.

#### What this is for, and what it is not

It is payload that can be read in bulk and read early. Payload proper is
addressed one document at a time and holds whatever the application likes;
these are for the few numbers a reader needs about *many* documents **before**
it decides which payload to fetch at all — a ranking prior, a date to prefer
recency by, a cell to filter by position.

The format does not interpret them. That is the same line drawn around payload
in §5.5: how a number is written down is declared, what it measures is not.

It is deliberately **not** the document lengths section (§5.7). Lengths have a
meaning this specification defines, because scorers depend on it; a reader finds
them without matching a name against a string. These have no meaning to the
format at all.

#### Cost

The same as any bit-packed column: the width of the largest value it holds.
Document lengths, which are one such column, measured at 1.7 bytes a document
over 2,523 documents. A column of small integers costs less; a column of
timestamps costs about five bytes a document, and is still resident for a corpus
of millions.

## 6. Metadata (`0x0001`)

Canonical CBOR (RFC 8949 §4.2). A reader MUST tolerate unknown keys.

| Key | Type | Required | Description |
|---|---|---|---|
| `generation` | uint | yes | Monotonic generation number |
| `source` | array | yes | One entry per artifact the index was derived from |
| `source[].content_hash` | text | yes | `<algorithm>:<lowercase hex>`; `algorithm` is `sha2-256` in this version |
| `source[].uri` | text | no | Where the source can be obtained |
| `source[].modified` | text | no | When the source's publisher last changed it, in RFC 3339 |
| `source[].retrieved` | text | no | When the builder obtained it, in RFC 3339 |
| `generator.name` | text | yes | Writer implementation |
| `generator.version` | text | yes | Writer version |
| `generator.params` | map | yes | Full build parameters |
| `tokenizer.id` | text | yes | Identifier from the tokenizer registry |
| `tokenizer.params` | map | yes | Tokenizer parameters |
| `license` | text | no | The license the index is published under, as an SPDX license expression |
| `attribution` | text | no | What a consumer shows alongside results from this file |
| `attrs` | map | no | Application-defined |

`generator.params` and `tokenizer.params` together with the source content
hashes MUST be sufficient to reproduce the file byte-for-byte (§3.4).

A source entry names an artifact the index was derived from. That is usually a
source of truth — a register, a corpus — but an index is often produced from
one by a program, and a chain that names the register and the result while
omitting the step between them is checkable only in parts. Recording the
transform's own hash as a source closes it, and needs no other mechanism.

A hash says which bytes a source was, and not when. `modified` and `retrieved`
say that: `modified` is the time the publisher gives for its last change — an
HTTP `Last-Modified`, a release date — and is how current the data is;
`retrieved` is when the builder obtained it, and bounds how stale it can be when
the publisher gives no time. A writer MUST write either as an RFC 3339
`full-date` (`2026-11-15`) or `date-time` (`2026-11-15T15:41:42Z`). A reader
MUST NOT reject a file because one does not parse as a date, which costs the
record and not the index.

`uri` SHOULD be where someone other than the builder can obtain the source, not
a path on the builder's machine; and where it is, the content hash SHOULD be of
the bytes served there — the archive as published, not a file unpacked from it
— so that anyone can fetch the source and check it against the file.

`license` is the license of **the index**, not of its sources. An index built
from a database is usually a derivative of it, and where that database's license
is share-alike — OpenStreetMap's ODbL — the index is covered too. And the index
is what is distributed: to a reader that fetches it in ranges as much as to one
that downloads it whole. The value
SHOULD be an SPDX license expression (`ODbL-1.0`, `CC-BY-4.0`,
`ODbL-1.0 AND CC-BY-4.0`); a reader MUST NOT reject a file for one it does not
recognize.

`attribution` is the credit that has to travel with results derived from this
file — `© OpenStreetMap contributors`, say. A reader that shows results to a
person SHOULD show it with them. It is a single piece of text, for the file as a
whole: sources that need crediting differently are better kept in separate
files, and their credits combined by whatever queries them together.

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

A Mokuji file is untrusted input. Every offset, length, and count in the header,
the section table, and the offset sections is attacker-controlled.

A reader MUST validate that every offset and length lies within the file before
use, MUST fail cleanly rather than crash on malformed input, and MUST NOT
allocate based on a count or length taken from the file without bounding it.

The last is easy to agree with and easy to get wrong: a four-byte block count
can ask for gigabytes before a single byte of the blocks has arrived. Reserving
a bounded amount and letting the structure grow as bytes actually arrive costs
nothing and removes the whole class.

## Appendix A. Decision log

Recorded because the reasoning is the part that is hard to reconstruct later.

**Multi-term I/O bound is `O(k)`, and nothing is reserved for making it
better.** Precomputed postings for frequent term pairs would push multi-term
queries toward `O(1)`, at a build cost that grows quadratically and for a
benefit a dictionary lookup does not need. An earlier draft set a section
identifier aside for it. That identifier is gone: a reserved identifier for an
undefined feature is a decision made by default, and this format would rather
make them deliberately. A linear bound is still a bound, which is the property
that was missing from the field.

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

**Positions are optional and located from the dictionary entry.**
Address lookup does not need phrase queries, and an index that carries positions
is several times the size of one that does not. Putting them inside the postings
run would have made every query pay for them; putting them in their own section,
found through two more varints in an entry that is being read anyway, means they
cost bytes and not reads. A format read in ranges can afford that where one
downloaded whole cannot, which is the clearest case where the transport changes
what is worth storing.

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

**The format carries document lengths but not scores.**
Scoring is outside the specification because the model is corpus-dependent.
Which statistics the models need is not corpus-dependent, and the header was
short one: it gave the mean document length but not each document's own, which
is what BM25 normalizes by. A reader that cannot rank cannot ask for the best
ten, so it pays for every match — measured, 39 range reads where 6 would do.
Supplying the statistic while declining to supply the score is where the line
belongs; it had simply been drawn one field too early.

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

**Flag bit 0 is reserved rather than used.**
It named a monolithic transducer mode, and §5.3 described one without defining
its encoding — a specification pointing at something it does not specify, which
is the defect this format exists to complain about. Having decided the encoding
is not worth defining (above), the mode goes with it. The bit stays reserved so
that it is not reused by something that would collide with an implementation
that read the earlier draft.

**The term dictionary is blocked rather than a single FST.**
See §5.3. A monolithic FST cannot satisfy the I/O bound over a range-read
transport, and the I/O bound is the point of the format. The FST remains, one
level up, as the index over block boundaries, which preserves the ordering
properties that motivated it.

**Completions are a recorded choice of documents, not a score.**
A short prefix at planet scale spans hundreds of dictionary blocks (§4.1.1), and
nothing in the layout makes that smaller: the dictionary is sorted, so the terms
are where they are. Edge n-grams would make every prefix a term; on a word
index of the same places, an answer for every prefix of up to five characters
alone measured 19 MB. What measured small was the opposite: keep an answer only
for the prefixes that are expensive — 59 KB on that word index, 165 KB on a name
index, where long names share long prefixes. They are few because a longer
prefix matches a subset of what a shorter one does, so past some length none
qualifies, and none longer can.

That is stored results, and the format keeps scoring outside (§1). The line is
drawn where document lengths drew it (§5.7). The section carries two kinds of
fact. One is checkable against the rest of the file: each listed document
contains a term that begins with the prefix, and every expensive prefix has an
entry. The other is a choice, recorded as a choice: which documents came first.
A different ranking is a different file, with different `generator.params`,
just as a different tokenizer is.

The coverage fields are what make it more than a cache. Without them a reader
learns nothing from a prefix's absence. With them, absence means cheap, and the
bound in §4.1 becomes a number a publisher can state for a whole file.

Coverage is defined over byte strings rather than characters, because the
dictionary compares bytes. Byte prefixes that stop inside a multi-byte character
therefore need entries too. Measured on the name index, 1,549 of its 11,243
entries end inside a character: 14% more entries, for a definition that needs
no knowledge of an encoding.

It is not a completion for everything someone types. An entry answers one prefix
alone; a finished word followed by an unfinished one is an intersection, and the
best ten of an intersection are not among the best ten of either side. Measured
on the same places, completing the second word from a word index cost a median
of 29 reads and up to 13 MB. Indexing each name as one term (`name-list`,
`TOKENIZERS.md`) turns the same query into one prefix, and completions then
answered every one of 2,776 partly typed names in at most 3 reads and 24 KB
before the payload — the same answers, in all 2,776, as walking the dictionary
without them.

**The license and the credit are in the metadata, not in `attrs`.**
`attrs` is where an application puts what only it understands, and a reader
that did not write a file cannot know which key there means what. A license is
not like that: every file has one whether it says so or not, and the one reader
that most needs to find it — a page that shows results to a person, and owes
them a credit — is a reader that knows nothing about the application that built
the file. So the key is named here, and its value is left as text.

It is the index's license rather than a list of its sources' because the
question a publisher has to answer is what the file they are distributing may
be used for, and a derived database answers that for itself. Where the sources'
licenses matter as provenance, they belong beside the sources' hashes; nothing
is set aside for that until something needs it.

One credit per file, rather than one per document, follows from keeping sources
that license differently in different files. A file that mixes an ODbL source
with others can bring the others within the ODbL's reach, which is a reason to
keep them apart that has nothing to do with this format, and once they are apart, a file
has one credit to state.

**A source records when, as well as which.**
An index of an address register is a claim about the register on some day, and
which day is the first thing someone relying on it asks. The content hash cannot
answer that: it identifies the bytes, and the register's publisher replaces those
bytes under the same name every month. Measured on the Address Base Registry,
the three tables a town-level geocoder is built from were last changed in
November 2026, October 2025 and March 2024; a file that records only their
hashes leaves a reader to discover that by downloading each of them and finding
a match, which stops working the day the publisher replaces one.

Two times rather than one, because they answer different questions and either
can be missing. `modified` is the data's own age, and only the publisher can
state it. `retrieved` is the builder's, always known, and an upper bound on
staleness when the publisher states nothing.

They sit on each source rather than once on the file because the sources of one
index are not of one age, as above, and the oldest is the one that matters. And
they are in the metadata rather than in `attrs` for the reason the license is: a
reader that knows nothing about the application that built a file is the one
that needs to say how old its answers are.

Neither is part of what makes a build reproducible, any more than `uri` is. They
are recorded as given, so a rebuild with the same values produces the same
bytes, and a rebuild with today's `retrieved` produces a file that truthfully
differs.

## Appendix B. A worked example

`conformance/fixtures/tiny.mokuji` is 863 bytes and indexes four documents:

```
tokyo chiyoda    35.694,139.754
tokyo chuo       35.671,139.772
tokyo shibuya    35.664,139.698
osaka chuo       34.681,135.513
```

Its full annotated hex dump is in `conformance/fixtures/tiny.hex`, and
`dict.mokuji` alongside it is the same walk for a dictionary index — postings
implied throughout, payload stored as delta-coded integers. The fixture
is stored raw, so that it can be read against §5 by hand. What follows walks one
query through it, `chuo`.

**Header**, at offset 0:

```
00000000  4d 4f 4b 55 4a 49 76 30 00 01 0e 00 09 00 00 00  |MOKUJIv0........|
00000010  04 00 00 00 00 00 00 00 08 00 00 00 00 00 00 00  |................|
00000020  40 00 00 00 00 00 00 00 d8 00 00 00 00 00 00 00  |@...............|
```

Magic `MOKUJIv0`; version 0.1; flags `0x000e`, so `HAS_PAYLOAD`, `HAS_FREQS`
and `HAS_POSITIONS`; nine sections; four documents totalling eight tokens; the
section table is 0xd8 bytes at offset 0x40.

**Term index** (`0x0010`), at offset 600:

```
00000258  03 00 00 00 00 07 63 68 69 79 6f 64 61 00 05 6f  |......chiyoda..o|
00000268  73 61 6b 61 00 05 74 6f 6b 79 6f                 |saka..tokyo|
```

Three blocks, then three front-coded keys: `chiyoda`, `osaka`, `tokyo`, each
with `shared_len` zero because they share no prefix. The greatest key not
exceeding `chuo` is `chiyoda`, so block 0 is the only block that can hold it.

**Term block offsets** (`0x0012`), at 627: block 0 spans `0x2d8..0x2f3`.

**Term block 0**, at 728, 27 bytes:

```
000002d8  02 00 00 07 63 68 69 79 6f 64 61 01 b9 05 00 cc  |....chiyoda.....|
000002e8  05 02 02 02 75 6f 02 00 05 02 04                 |....uo.....|
```

Two entries, first term ID 0. Each carries two more fields than it would
otherwise, because `HAS_POSITIONS` is set.

Entry 0: `shared_len` 0, `suffix_len` 7, `chiyoda`; `doc_freq` 1;
`postings_offset` `b9 05`, the first entry so absolute — 0x39 plus 5 shifted by
seven is 697; `postings_len` **0**, so this term's postings are implied
(§5.3.4); `positions_offset` `cc 05`, again absolute, 716; `positions_len` 2.

Entry 1: `shared_len` 2, `suffix_len` 2, `uo`; `doc_freq` 2; postings offset
delta 0, because the entry before it consumed no bytes, so 697 again;
`postings_len` 5; positions offset delta 2, so 718; `positions_len` 4.

**Postings** for `chuo`, at 697, 5 bytes:

```
000002b9  00 02 01 03 00                                   |.....|
```

Codec 0; two documents. The document group has `bit_width` 1 and one byte,
`0x03`, which is the bits `1, 1`. The first value is a document ID and the rest
are gaps less one, so the documents are 1 and 1 + 1 + 1 = 3. The frequency
group has `bit_width` 0 and therefore no bytes at all: every frequency is 1,
stored as 0.

**Positions** for `chuo`, at 718, 4 bytes:

```
000002ce  01 01 01 01                                      |....|
```

Two documents, one occurrence each, so two groups of one value. Each is
`bit_width` 1 and one byte holding `1`: `chuo` is the second word of both
documents, at position 1.

**Document lengths** (`0x0032`), at 695, two bytes:

```
000002b7  02 aa                                            |..|
```

One bit-packed group of four values at `bit_width` 2, then one byte holding
them least significant bit first: `0xaa` is `10 10 10 10`, so every document is
two tokens long. Four documents, two bytes, and a scorer can normalize.

**Payload offsets** (`0x0031`), at 659:

```
00000293  00 00 00 00 02 00 00 00 02 00 00 00              |............|
```

Payload codec 0, opaque, so no value count and no scale; two documents to a
block; two blocks; then the offsets.

**Cost.** One term block read, one postings read, and one payload read per
distinct payload block. For `tokyo AND chuo`, which matches document 1: four
reads plus one, against a bound of `2k + m` = 5.

For `chiyoda` alone: one term block read and one payload read. The postings
read does not happen, because there were no postings to read. Nor does the
positions read, because nothing asked for a phrase — the section is 12 bytes of
a 863 byte file that most queries never touch, which is the shape the argument
in §5.6a is about.
