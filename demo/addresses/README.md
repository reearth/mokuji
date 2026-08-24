# Address lookup

Forward geocoding from a single static file, over range reads, with no server.

The corpus is Japan's Address Base Registry, at two levels: town level for the
whole country, and residence level — the level a street address is actually
written at — for one prefecture.

## Town level: the whole country

```sh
./fetch-abr.sh abr
python3 abr.py --fullset abr/mt_town_fullset_all.csv --pos abr/pos --out abr.tsv
colex build abr.tsv abr.colex --tokenizer ja-address \
  --source abr/mt_town_fullset_all.csv --source abr.py
colex prefix abr.colex "東京都千代田区丸の内"
```

```
東京都千代田区丸の内1丁目	139.767201,35.68156
東京都千代田区丸の内2丁目	139.763447,35.680022
東京都千代田区丸の内3丁目	139.763476,35.676952

query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    1088 of 5685043
```

**725,814 addresses nationwide, in 5.7 MB. Two range reads and 1.1 KB to answer.**

| | |
|---|---|
| Source | 159 MB of CSV, from two registry tables |
| Corpus | 35 MB, one address and its coordinates per line |
| Index | 5.7 MB |
| Build | 2.4 s, including hashing the 159 MB register |
| Full verification | 4.8 s, reading and decoding every block, and re-hashing the register |

### Provenance

`--source` records the register's own content hash alongside the corpus's, so
the index names what it was actually derived from rather than an intermediate
nobody keeps:

```
$ colex verify abr.colex --source abr/mt_town_fullset_all.csv
tokenizer       ja-address
derived from    sha2-256:93581da1…   abr.tsv
derived from    sha2-256:8f9b02e4…   abr/mt_town_fullset_all.csv
derived from    sha2-256:12cfe1f9…   abr.py
source          abr/mt_town_fullset_all.csv
hashes to       sha2-256:8f9b02e4…
                matches the recorded provenance
ok
```

Every link is named. The register, the program that read it, the corpus that
came out, and the normalization applied on the way in — that last one because
`ja-address` is a registered tokenizer rather than a step hidden in a script.

Normalization belongs to the tokenizer for exactly this reason. It decides what
a term *is*, so leaving it in `abr.py` would put the most consequential
transformation in the pipeline outside anything the index records. `abr.py` now
only assembles fields from the register.

### Attribution

The Address Base Registry is published by the Digital Agency of Japan under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Anything derived
from it must carry that attribution. No registry data is committed here; the
scripts fetch it.

## Residence level: Tokyo

Town level names a block of a neighbourhood. Residence level names a door, and
is what a street address is actually written at.

```sh
./fetch-abr.sh abr 13
python3 abr.py rsdt --rsdt abr/mt_rsdtdsp_rsdt_pref13.csv \
  --pos abr/mt_rsdtdsp_rsdt_pos_pref13.csv --pref 東京都 --out tokyo.tsv
colex build tokyo.tsv tokyo.colex --tokenizer ja-address \
  --source abr/mt_rsdtdsp_rsdt_pref13.csv --source abr.py
colex prefix tokyo.colex "東京都千代田区丸の内一丁目"
```

```
query normalized to '東京都千代田区丸の内1丁目' by the 'ja-address' tokenizer
東京都千代田区丸の内1丁目10番10号	139.765575675,35.678505258
東京都千代田区丸の内1丁目10番11号	139.765623217,35.678634717
...
query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    832 of 18283401
```

**1,744,962 addresses, every one with coordinates, in 18 MB. Two range reads.**

| | |
|---|---|
| Source | 225 MB of CSV, two tables |
| Corpus | 120 MB |
| Index | 18.3 MB |
| Build | 5.0 s |

| Results | Dictionary | Postings | Payload | Total reads | Bytes |
|---|---|---|---|---|---|
| 20 | 1 | 0 | 1 | 2 | 795 B |
| 100 | 2 | 0 | 2 | 4 | 1.6 KB |
| 500 | 5 | 0 | 9 | 14 | 6.2 KB |
| 2,000 | 17 | 0 | 32 | 49 | 22 KB |

Two range reads and under a kilobyte, from a file ten times the size of the
town-level one holding two and a half times as many addresses. Nothing in the
read column moved.

### The dictionary gets cheaper as addresses get more specific

| | addresses | dictionary | per address |
|---|---|---|---|
| Town level, nationwide | 725,814 | 3.81 MB | **5.25 B** |
| Residence level, Tokyo | 1,744,962 | 2.77 MB | **1.58 B** |

Residence-level addresses are longer but share far more: thousands of doors
hang off one neighbourhood name, so front-coding pays for the prefix once and
the rest of each key is a block and a number. Two and a half times as many
addresses cost three quarters of the dictionary.

Nationwide residence level would sit above Tokyo's figure, since 47 prefectures
share less than one does, but the dominant effect is doors per neighbourhood
rather than prefectures per country. At two bytes an address, 19 million
addresses would be a dictionary of roughly 40 MB. That is well under the
50–150 MB estimated earlier from synthetic data — the estimate was
conservative, which is what estimates from synthetic data usually are.

### Payload is now the whole problem

15.1 MB of the 18.3 MB — 83% of the file — is coordinate text, and the registry
publishes nine and ten decimal places:

```
139.765575675,35.678505258
```

The tenth decimal place of a longitude is a tenth of a millimetre. That is
almost certainly not a claim about accuracy: it is what printing a `float64`
looks like after converting from a plane rectangular coordinate system, and the
registry is being faithful rather than boastful. The fault is ours for carrying
the residue of someone else's coordinate conversion into an index and then
compressing it.

Rounding to six decimals, which is about eleven centimetres, gives:

| | index |
|---|---|
| As published | 18.3 MB |
| Coordinates at six decimals | **12.9 MB** |

A 29% smaller file for precision nobody has. The format has nothing to say
about this — payload contents are outside the specification (`SPEC.md` §5.5) —
which is exactly why it is worth saying here: the largest remaining saving in
this index is not the format's to make.

### What the real data changed

The dictionary costs **5.3 bytes per address** compressed, against 1.9 on the
synthetic corpus — 2.7x worse, and the reason the synthetic numbers are
reported separately.

That is the measurement the design notes asked for. Extrapolating 5.3 bytes to
a nationwide block-level register of roughly 19 million addresses gives about
100 MB for the dictionary, which lands inside the 50–150 MB that was estimated
from a synthetic benchmark. Real block-level addresses are longer and share
longer prefixes than town-level ones, so the true figure is likely lower; that
is a guess, and the way to settle it is to index them.

Two further things the real data shows:

- **Coordinates are sparse at town level.** 324,471 of 725,823 addresses have a
  representative point, because `mt_town_pos` covers less than the town master
  does. The rest are indexed with an empty payload, which the format allows and
  a product would have to decide about.
- **The registry spells chome two ways.** `一丁目` in one municipality and
  `１丁目` in the next. Indexed as written, the same concept sorts in two
  different places and neither is where someone typing would look, which is
  what `ja-address` is for. Normalized, `丸の内` comes back as 1, 2, 3 rather
  than in the arbitrary order the kanji give.

  It is not fully fixed. `10丁目` still sorts between `1丁目` and `2丁目`,
  because prefix order is byte order and zero-padding would break the matching
  it fixed the ordering for. Getting both means parsing the address into
  fields, which is a geocoder's job.

- **41 addresses collided under normalization** and their lines were dropped,
  reported by `build` rather than silently merged. Two registry rows spelling
  the same chome differently are one address, and a key names one record — but
  which payload survives is a data decision, so the count is printed.

### Queries are normalized too

`prefix` reads the tokenizer identifier out of the index's metadata and puts the
query through it before searching:

```
query normalized to '東京都千代田区丸の内1丁目' by the 'ja-address' tokenizer
```

Without that, an index built from `一丁目` is unreachable by anyone typing
`一丁目`, which is everyone. This is what recording the tokenizer is *for*: the
format does not specify how text becomes terms, but it does make sure a reader
can find out what was done and do the same.

## Why prefix and not search

An address is a hierarchical key. It is looked up whole or by prefix, never by
intersecting its parts — nobody searches for documents containing both `東京都`
and `丸の内`. So the whole normalized address is one term (`--tokenizer
identity`), and the query is a prefix range over the sorted dictionary.

Because terms are sorted, everything sharing a prefix is contiguous; because
postings are written in term order, so are their runs; and because documents
were added in the same order, so is their payload. A prefix range is one
contiguous slice of every section at once.

## In a browser

`../web` serves the same file to a page that reads it with no WebAssembly and
no bundled decompressor, because the required codec is one browsers already
have.

```sh
cd ../web && python3 serve.py --index ../addresses/tokyo.colex
```

Typing `一丁目` finds `1丁目`, because the page reads the tokenizer identifier
out of the metadata and applies the same normalization the addresses went
through.

## Sections

The Tokyo index. Blocks are deflated individually, so a reader still fetches one
block and decompresses one block; read counts are unchanged by compression and
only the bytes moved shrink.

```
                      stored         raw     codec
meta                    503 B       503 B    raw       resident
term_index             45.8 KB     178.6 KB  deflate   resident
term_block_offsets    109.1 KB     109.1 KB  raw       resident
payload_offsets       218.1 KB     218.1 KB  raw       resident
postings                    0           0    raw       implied
term_blocks             2.8 MB      16.7 MB  deflate
payload                15.1 MB      46.0 MB  deflate
                       -------     -------
                       18.3 MB      63.3 MB   from a 120 MB corpus
```

The offset tables and the metadata are stored raw on purpose. Metadata stays
readable without a decompressor because it is the provenance record, and tooling
that only wants to know where a file came from should not have to implement the
format to find out. The offset tables stay raw because their size feeds back
into the placement they describe; compressing them is worthwhile and wants a
settled layout to build on.

## Still missing

**Compression uses no shared dictionary.** A trained shared dictionary takes
term blocks from about 7x to about 27x, measured on blocks of this kind. That is
by far the largest remaining win in the dictionary, and it is deliberately not
taken: browsers cannot supply a preset dictionary through their native
decompression, so it would cost a decompressor in the page. See `SPEC.md`
§5.6.1.

**Payload is coordinate text**, and on the Tokyo index it is 83% of the file.
Rounding is not the largest saving available, and most of what remains *is* the
format's to make. Measured on the Tokyo payloads (`tools/payload-encodings.py`),
per address after per-block deflate:

| encoding | index payload | per address |
|---|---|---|
| Text, as published | 15.2 MB | 8.69 B |
| Text, rounded to six decimals | 9.8 MB | 5.61 B |
| Fixed point 1e-6, `int32` pair | 8.8 MB | 5.05 B |
| **Fixed point 1e-6, zigzag varint delta** | **5.9 MB** | **3.39 B** |
| Fixed point 1e-5 (about a metre), delta | 3.7 MB | 2.11 B |

Delta encoding wins because of the monotonic ID rule (`SPEC.md` §3.2), not
despite it. Documents are in term order, addresses sort hierarchically, and a
hierarchy of addresses is very nearly a spatial ordering — so consecutive
documents are metres apart and their coordinates differ in the last few digits.
The rule was adopted to keep the dictionary small; it turns out to pay a second
time, in a section that does not know the dictionary exists.

That splits the problem three ways, and only the last part is genuinely outside
the format:

| | whose | measured here |
|---|---|---|
| Structure — what the ordering makes possible | the format | taken |
| Encoding — how a coordinate is written down | the format | **not taken: 2.6x** |
| Precision — how many digits are real | the publisher | 1.5x, correctly theirs |

The encoding layer wants a declared payload codec, the way PMTiles declares a
tile type without saying what a tile means. And the precision layer is not
beyond the format either, once it is understood as provenance rather than
compression: an index that rounds and *records that it rounded* is a shortened
copy that says so. One that rounds silently is a damaged one.
