# Mokuji Collections

**Version:** 0.1 (draft — not frozen, subject to breaking change)
**Status:** Draft
**Editor:** Re:Earth and contributors

This document is licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

---

## 1. Scope

A Mokuji file is immutable (`SPEC.md` §3.3). Data that changes — a gazetteer
revised daily, places a user uploads — needs an index that changes, and a new
generation of a planet-sized file for each change is out of proportion to it.

A **collection** is an index built out of files that never change: a base, the
**segments** added since, and the documents deleted from any of them, listed in
a small **manifest**. The files are published once and cached forever. The
manifest is the one thing ever rewritten.

This document specifies the manifest and what a collection means. It changes
nothing in `SPEC.md`: every segment is an ordinary Mokuji file, readable on its
own by any reader of that specification.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be interpreted
as described in RFC 2119.

## 2. The manifest

A manifest is a JSON text (RFC 8259) encoding one object:

```json
{
  "mokuji_manifest": "0.1",
  "generation": 3,
  "segments": [
    {
      "path": "base-2026-09.mokuji",
      "content_hash": "sha2-256:4f1c…",
      "length": 425329881,
      "deletes": [412, 9981]
    },
    {
      "path": "delta-2026-09-30.mokuji",
      "content_hash": "sha2-256:9a07…",
      "length": 50835
    }
  ]
}
```

| Key | Type | Required | Description |
|---|---|---|---|
| `mokuji_manifest` | string | yes | This document's version, `major.minor` |
| `generation` | integer | yes | Increases with every change to the manifest |
| `segments` | array | yes | Non-empty, oldest first |
| `segments[].path` | string | yes | A URI reference to the file, resolved against the manifest's own URI (RFC 3986 §5) |
| `segments[].content_hash` | string | yes | Of the whole file, as `SPEC.md` §6 writes a source's |
| `segments[].length` | integer | yes | Of the whole file, in bytes |
| `segments[].deletes` | array | no | Documents of this segment no longer in the collection, by their ID in it, strictly increasing |
| `segments[].region` | object | no | Where the segment's documents are (§7) |
| `segments[].region.countries` | array | yes, in a region | ISO 3166-1 alpha-2 codes, uppercase, none twice |

A reader MUST reject a manifest whose major version it does not implement, and
MUST ignore keys it does not recognize. It MUST reject a segment whose length
differs from `length`, whose deletes are not strictly increasing, or which
deletes a document it does not have. It SHOULD compare `content_hash` wherever
it holds the whole file anyway.

Every segment MUST record the same `tokenizer.id` and `tokenizer.params`
(`SPEC.md` §6), and a reader MUST reject a collection whose segments do not: a
query has to mean the same thing in all of them.

## 3. What a collection is

The collection is the **concatenation of its segments, in manifest order, less
their deletes**. That order is the collection's document order: every document
of an earlier segment precedes every document of a later one, and within a
segment documents keep their own order.

A delete names a document by the ID it has **in its own segment**. Segments
never change, so that ID never does, and no key column, and no reference from
one segment into another, is needed to say which document is gone. An update is
a delete in the segment that held the old document and an add in a new one.

### 3.1 Term queries

A document matches a query in the collection when it matches in its segment
and is not deleted. Results are in collection order.

### 3.2 Completion

The first `n` documents of a prefix are the first `n` live documents of the
collection, in collection order, that hold a term beginning with it. A reader
consults segments in order and stops once it has `n`.

In each segment, a completions section (`SPEC.md` §5.3.5) answers first. When
deletes take documents it gives, the reader asks for everything the entry
lists; if that is still short, it asks for `n` plus the segment's number of
deletes, which is enough however the deletes fall, and reads the segment's
dictionary. The answer is the same either way. What differs is the cost: see
§5.

### 3.3 Ranking across segments

Collection order ranks every document of an earlier segment above any of a later
one. A place added in a later segment therefore never completes a prefix that
the base already fills. Ranking segments against each other needs a score they
share, and scores are outside the format (`SPEC.md` §1).

A reader that has such a score asks **every** segment for its own first `n` and
ranks the lists itself. That costs every segment's reads instead of stopping
early. The segments are independent, so they can be asked at once.

## 4. The I/O contract

Opening a collection costs one read for the manifest, and then, for each
segment, the one read that populates its resident set (`SPEC.md` §4). The
segments do not depend on each other, so the second step is one round trip
however many there are.

A query costs the sum of what it costs in each segment consulted, and each
segment's cost is bounded as `SPEC.md` §4 bounds it. Asked at once, the round
trips are those of the costliest segment.

Measured on 12,854,245 places with 2,776 partly typed names, and deltas of
1,000 places each (`tools/README.md`). Reads are counted before any payload;
"round trips" is the most reads any one segment needed:

| Deltas | Opening | Completion, p99 reads | Across all segments, p99 reads | Round trips, p99 |
|---|---|---|---|---|
| 0 | 2 reads, 835 KB | 3 | 3 | 3 |
| 1 | 3 reads, 886 KB | 3 | 4 | 3 |
| 4 | 6 reads, 1.0 MB | 6 | 8 | 3 |
| 16 | 18 reads, 1.6 MB | 18 | 32 | 3 |

## 5. Deletes and completions

A delete that lands in a completion entry does not make the entry wrong. It
makes the entry short. While the entry still lists `n` live documents, it
answers at no read. Past that, the prefix reads its dictionary until the
collection is compacted.

Uniform deletes rarely matter: with 1% of the base deleted at random, 34 of
the 2,776 names read their dictionary. Real edits are not uniform. They fall
on places people know, and those are exactly the ones entries list. With 12,854
of the 100,000 most important places deleted:

| Entry lists | Opening | Names reading the dictionary | p99 reads | p99 bytes |
|---|---|---|---|---|
| 10 | 835 KB | 497 | 189 | 2.7 MB |
| 20 | 964 KB | **0** | **6** | **33 KB** |

A writer SHOULD therefore list more documents per entry than a reader shows,
so that deletes have room: for a page of ten, twenty cost 129 KB more at open
and absorbed every delete measured.

## 6. Changing a collection

A publisher writes the new segment under a new path first, and then replaces
the manifest. Segments are immutable and MAY be cached indefinitely. The
manifest SHOULD be cached briefly, if at all.

A reader holding the previous manifest keeps working for as long as the files
it names are still published. A publisher SHOULD keep a superseded segment
for longer than any reader is expected to hold a manifest.

### 6.1 Compaction

Compaction folds a collection into one file. It keeps collection order, so the
file answers every query exactly as the collection did. A new manifest then
names only that file.

When to compact is a publisher's decision, and the measurements above say what
it trades:

- **Deltas.** Each adds one read at open, and one read to a query that ranks
  across segments.
- **Deletes.** The number that matters is the most documents any completion
  entry has lost, set against how many it lists beyond a page. Past that
  margin, a prefix reads its dictionary.

## 7. Regions

A segment MAY state a **region**: the countries that every one of its documents
lies within. That is how a collection is sharded. The index is split into one
segment per region, and a reader asked about a place opens only the segments
there.

```json
{ "path": "asia.mokuji", "content_hash": "sha2-256:…", "length": 161336290,
  "region": { "countries": ["JP", "KR", "CN", "…"] } }
```

The collection **within** a set of countries is the collection restricted to
the segments whose region includes one of them, together with the segments that
state no region. Order and deletes carry over. A segment with no region may hold
documents anywhere, so it is never left out. A reader asked about a place MAY
restrict the collection this way, and then need not open the other segments at
all.

The region is a statement by the publisher, and a reader does not check it.
Checking it would mean reading every document's position, and positions are
payload, which the format does not interpret (`SPEC.md` §5.5).

### 7.1 What sharding buys

Measured on the same 12,854,245 places, cut by continent from GeoNames' own
country table, with every file built the same way and each opened with a 64 KB
read:

| | Places | File | Opening | p99 reads a keystroke |
|---|---|---|---|---|
| Japan alone | 98,189 | 5.5 MB | 1 read, 64 KB | 3 |
| Europe | 3,140,490 | 106 MB | 2 reads, 238 KB | 3 |
| Asia | 4,168,376 | 161 MB | 2 reads, 370 KB | 3 |
| The planet, one file | 12,854,245 | 425 MB | 2 reads, 964 KB | 3 |
| Seven continents, place unknown | 12,853,387 | 424 MB | 11 reads, 1.08 MB | 16, in 3 round trips |

A regional shard is cheaper to open, by a factor of fifteen for one country.
It does not make a keystroke cheaper. The bound in `SPEC.md` §4 holds at every
size, and a keystroke costs three reads at the 99th percentile in a country
and on the planet alike.

A query about no place in particular costs one opening read per segment, and
every segment's reads for each keystroke. Across seven continents that is 11
reads and 1.08 MB to open, and 9 reads a keystroke at the median rather than 2.
It stays at three round trips, because the segments are asked at once. The
cost grows with the number of shards, not with their size. So a service that
answers both kinds of query can publish both, a planet file for queries about
no place and regional shards for the rest. Splitting costs almost no space: the
seven continents total 424 MB against 425 MB for the planet.

The 858 places GeoNames gives no country are in no continent. A collection that
has to hold them puts them in a segment with no region.

## Appendix A. Decision log

**A manifest, and no change to the file format.**
The alternative was to make a file mutable, or to let one file refer into
another. Both give up the property the format is built on: a file is a
complete, immutable, verifiable thing. A manifest keeps every file exactly
that. It adds one small object whose only job is to say which files, in which
order, less which documents.

**JSON, not CBOR.**
The metadata section is CBOR because it lives inside a binary file and has to
be canonical. A manifest is a separate object, fetched whole, read by every
kind of client, and sometimes written by hand. JSON needs no decoder in a page.
Canonical form matters less here, because the files are what carry the hashes.

**Deletes are named by the ID a document has in its own segment.**
A stable external key would need a column in every segment. For a base of
millions of documents that column is resident, and it costs megabytes to hold.
A segment-local ID costs nothing, never changes because the segment never
changes, and needs no reference from one segment into another. The cost is
that deletes are only meaningful against a particular file. That is right: a
compaction renumbers everything, and it also consumes every delete.

**Deletes live in the manifest, not in a segment.**
A deletion is a change of state, and the manifest is where state changes. A
delete-only update then needs no new file, and every segment stays a plain
Mokuji file with nothing a reader of `SPEC.md` alone would miss.

**Collection order is concatenation.**
It is the one order that needs no score, and the one that compaction can
reproduce exactly. That makes "the compacted file answers as the collection
did" a property that can be tested, and it is: the CLI's tests fold
generated collections with random deletes and compare every prefix. Ranking
across segments is still possible (§3.3). It is left to a reader with a score,
which is where `SPEC.md` leaves ranking.

**Shards are segments with a region; the dictionary is not shared.**
The alternative is one dictionary for the planet with postings split by region.
A query about no place then reads the dictionary once instead of once per
shard. But a dictionary entry would have to locate postings in other files,
which changes the entry and gives up the self-contained file. Measured, it would
save little. The seven continents' dictionaries total 90.6 MB against 89.3 MB
for the planet's, because only 1.75% of terms occur on more than one continent.
Sharing the dictionary would save at most 1.4% of the space. For a query about
no place, it would save the dictionary reads, about one per shard. It would not
save the postings and payload reads each shard still pays. And a planet file
published beside the shards answers such a query in two reads with no format
change at all.

**A region is countries, not a bounding box.**
Countries are what a gazetteer records and what a shard is cut by. A reader
that has a viewport and not a country maps one to the other. That mapping is
the application's, and a box around a country with an island far away would
include half an ocean. Nothing is set aside for boxes until something needs
them.

**Compaction merges indexes, not corpora.**
Terms, postings and payload are read back and written again under new IDs.
Nothing is tokenized twice, so nothing can come out tokenized differently, and
a collection can be compacted without the sources it was built from.
