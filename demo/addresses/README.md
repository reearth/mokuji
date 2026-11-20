# Address lookup

Forward geocoding from a single static file, over range reads, with no server.

The corpus is Japan's Address Base Registry, at two levels: town level for the
whole country, and residence level — the level a street address is actually
written at — for one prefecture.

## Town level: the whole country

```sh
./fetch-abr.sh abr
python3 abr.py town --fullset abr/mt_town_fullset_all.csv --pos abr/pos \
  --city-pos abr/mt_city_pos_all.csv --pos-level --out abr.tsv
mokuji build abr.tsv abr.mokuji --tokenizer ja-address --payload ints:6 \
  --attribute pos_level \
  $(python3 ../provenance.py args abr/mt_town_fullset_all.csv.zip \
      abr/mt_city_pos_all.csv.zip abr/pos/*.zip) \
  --source abr.py \
  --license CC-BY-4.0 \
  --attribution "アドレス・ベース・レジストリ（デジタル庁）, CC BY 4.0"
mokuji prefix abr.mokuji "東京都千代田区丸の内"
```

```
東京都千代田区丸の内1丁目	139.767201,35.681560
東京都千代田区丸の内2丁目	139.763447,35.680022
東京都千代田区丸の内3丁目	139.763476,35.676952

query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    3118 of 4682032
```

**725,546 addresses nationwide, every one with coordinates, in 4.7 MB. Two
range reads and 3.1 KB to answer.** Built from the registry as published on
2026-11-15.

| | |
|---|---|
| Source | 159 MB of CSV, from three registry tables |
| Corpus | 47 MB, one address, its coordinates and their level per line |
| Index | 4.7 MB |
| Build | 2.3 s |
| Full verification | 4.5 s, reading and decoding every block, and re-hashing the register |

### Where the coordinates come from

The town table has a representative point for most 大字・町 and 丁目 and for
almost no 小字, and 小字 are most of the rows. Joined on its own key, an address
has a point 27% of the time. So `abr.py` takes the point from the nearest level
above that has one:

| Placed at | addresses | `pos_level` |
|---|---|---|
| Its own point | 196,011 | 0 |
| Its 大字・町's — the same `machiaza_id` with the last three digits zeroed | 473,161 | 1 |
| Its municipality's, from `mt_city_pos_all` | 56,798 | 2 |

A 小字 placed at its 大字's point is placed by kilometres rather than metres, and
one placed at its municipality's by more. The index says which, as the
`pos_level` attribute column (`SPEC.md` §5.8), rather than handing back a
municipality's centre as if it were the koaza's. The column costs 221 KB, held
in memory once the file is open; `../web` shows it beside each result.

Without `--pos-level` the column is left out and the file is 4.5 MB. Without
`--city-pos` the 56,798 addresses with no point at either town level are
indexed with an empty payload, which `--payload ints:6` refuses, since every
document has to carry the same count of numbers; pass `--require-position` to
drop them instead.

### Provenance

`--source` records the register's own content hash alongside the corpus's, so
the index names what it was actually derived from rather than an intermediate
nobody keeps. `fetch-abr.sh` keeps each archive as the registry served it, with
a record of its URL, the registry's `Last-Modified` and when it was fetched
(`../provenance.py`), and `provenance.py args` passes all three to the build. So
the index says not only which bytes it came from but where anyone can get them
and how old they were:

```
$ mokuji verify abr.mokuji --source abr/mt_town_fullset_all.csv.zip
tokenizer       ja-address
license         CC-BY-4.0
attribution     アドレス・ベース・レジストリ（デジタル庁）, CC BY 4.0
derived from    sha2-256:0431a430…   abr.tsv
derived from    sha2-256:acb18e19…
                https://data.address-br.digital.go.jp/mt_town_fullset/mt_town_fullset_all.csv.zip
                modified 2026-11-15T15:41:42Z
                retrieved 2026-11-20T03:52:51Z
derived from    sha2-256:bf4e2662…
                https://data.address-br.digital.go.jp/mt_city_pos/mt_city_pos_all.csv.zip
                modified 2024-03-14T08:09:16Z
                retrieved 2026-11-20T03:53:02Z
...             and 47 mt_town_pos archives, modified 2025-10-27
derived from    sha2-256:5d2ebf1e…   abr.py
...
source          abr/mt_town_fullset_all.csv.zip
hashes to       sha2-256:acb18e19…
                matches the recorded provenance

ok
```

The three tables are of three ages: the addresses were published on
2026-11-15, the town coordinates on 2025-10-27, and the municipalities' points
on 2024-03-14. An index that
records only hashes would leave that to be found out by downloading each one.

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
from it must carry that attribution. The town-level build above writes it into
the index's metadata with `--license` and `--attribution` (`SPEC.md` §6), so the
credit travels with the file, and `../web` shows the one the file states. No
registry data is committed here; the scripts fetch it.

## Residence level: Tokyo

Town level names a block of a neighbourhood. Residence level names a door, and
is what a street address is actually written at.

```sh
./fetch-abr.sh abr 13
python3 abr.py rsdt --rsdt abr/mt_rsdtdsp_rsdt_pref13.csv \
  --pos abr/mt_rsdtdsp_rsdt_pos_pref13.csv --pref 東京都 --out tokyo.tsv
mokuji build tokyo.tsv tokyo.mokuji --tokenizer ja-address --payload ints:6 \
  --source abr/mt_rsdtdsp_rsdt_pref13.csv --source abr.py
mokuji prefix tokyo.mokuji "東京都千代田区丸の内一丁目"
```

```
query normalized to '東京都千代田区丸の内1丁目' by the 'ja-address' tokenizer
東京都千代田区丸の内1丁目10番10号	139.765576,35.678505
東京都千代田区丸の内1丁目10番11号	139.765623,35.678635
...
query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    872 of 7099942
```

**1,744,962 addresses, every one with coordinates, in 7.1 MB. Two range reads.**

## Residence level: all 47 prefectures

The same build, run over every prefecture. Coordinates are required, so an
address the registry has no point for is left out.

| | |
|---|---|
| Registry rows | 21,984,788 |
| Indexed | **21,814,175** (99.2%) |
| Corpus | 1.63 GB |
| **Index** | **85 MB**, 19.1x smaller |
| Per address | 3.90 bytes |
| Mean per prefecture | 464,131 addresses, 1.81 MB |
| Largest | Osaka, 2,301,148 addresses, 8.8 MB |
| A query | still 2 range reads |

Eighty-five megabytes is the whole of Japan at the level a street address is
written at, and 1.8 MB is the average prefecture — small enough that a page can
hold one and never ask for anything again.

**Coverage is uneven, and that is the registry rather than the build.**
Residence display (`住居表示`) is adopted municipality by municipality, and where
it is not, addresses are still written as parcel numbers, which this table does
not carry.

| | |
|---|---|
| Osaka | 2,301,148 |
| Fukuoka | 1,793,972 |
| Hokkaido | 1,750,313 |
| … | |
| Tottori | 65,167 |
| Shimane | 57,566 |
| Saga | 54,971 |
| Tokushima | 40,350 |
| **Kyoto** | **32,713** |

Kyoto has 2.5 million people and 32,713 residence-level addresses, because
Kyoto City addresses by street name rather than by block and number. So this is
21.8 million addresses, not every address in Japan; the town-level index above
is the one that covers the country evenly, and the two are meant to be used
together.

| | |
|---|---|
| Source | 225 MB of CSV, two tables |
| Corpus | 120 MB |
| Index | 7.1 MB |
| Build | 5.5 s |

| Results | Dictionary | Postings | Payload | Total reads | Bytes |
|---|---|---|---|---|---|
| 20 | 1 | 0 | 1 | 2 | 724 B |
| 100 | 1 | 0 | 2 | 3 | 940 B |
| 500 | 1 | 0 | 9 | 10 | 2.4 KB |
| 2,000 | 3 | 0 | 32 | 35 | 8.8 KB |

Two range reads, from a file holding two and a half times as many addresses as
the town-level one.

Or one read, if the reader holds the dictionary:

```
$ mokuji prefix tokyo.mokuji "東京都渋谷区" --resident
query:    1 read(s) total - 0 dictionary, 0 postings, 1 payload
bytes:    220 of 7099942
```

That costs one read at open and 864 KB of memory — the dictionary as stored,
decompressing one 9 KB block per lookup. Same file, different deployment; see
`SPEC.md` §4.2.

### The dictionary gets cheaper as addresses get more specific

| | addresses | dictionary | per address |
|---|---|---|---|
| Town level, nationwide | 725,546 | 2.91 MB | **4.01 B** |
| Residence level, Tokyo | 1,744,962 | 861 KB | **0.49 B** |

Residence-level addresses are longer but share far more: thousands of doors
hang off one neighbourhood name, so front-coding pays for the prefix once and
the rest of each key is a block and a number. Two and a half times as many
addresses cost under a third of the dictionary.

Nationwide residence level would sit above Tokyo's figure, since 47 prefectures
share less than one does, but the dominant effect is doors per neighbourhood
rather than prefectures per country. At a byte an address, 19 million
addresses would be a dictionary of roughly 20 MB. That is well under the
50–150 MB estimated earlier from synthetic data — the estimate was
conservative, which is what estimates from synthetic data usually are.

### Payload was the whole problem

Before the payload codec, 15.1 MB of an 18.3 MB index was coordinate text, and
the registry publishes nine and ten decimal places:

```
139.765575675,35.678505258
```

The tenth decimal place of a longitude is a tenth of a millimetre. That is
almost certainly not a claim about accuracy: it is what printing a `float64`
looks like after converting from a plane rectangular coordinate system, and the
registry is being faithful rather than boastful. The fault was ours for carrying
the residue of someone else's coordinate conversion into an index and then
compressing it.

`--payload ints:6` stores each coordinate as an integer number of microdegrees,
delta-coded against the previous document (`SPEC.md` §5.5.3):

| | payload | index |
|---|---|---|
| Text, as published | 15.1 MB | 18.3 MB |
| **Fixed point, delta-coded, six decimals** | **6.0 MB** | **9.1 MB** |

Half the file, and the read counts are unchanged. The stored payload is not even
compressed: delta-coded varints leave deflate nothing to find, so the writer
measured and stored them raw, which also means a payload read decompresses
nothing.

It works because of the monotonic ID rule (`SPEC.md` §3.2). Documents are in
term order, addresses sort hierarchically, and a hierarchy of addresses is very
nearly a spatial ordering — so consecutive documents are metres apart and their
coordinates differ only in the last digits. A rule adopted to keep the
dictionary small pays a second time in a section that does not know the
dictionary exists.

Of the two halves of the saving, only one was ever the publisher's:

| | whose | worth |
|---|---|---|
| Structure — what the ordering makes possible | the format | taken |
| Encoding — how a number is written down | the format | 2.6x, now taken |
| Precision — how many digits are real | the publisher | 1.5x, and theirs |

And the precision half is not beyond the format either, once it is read as
provenance rather than compression. The metadata records `payload: ints:6`, so
an index that rounds says that it rounded. A shortened copy that declares the
shortening is still a true copy; one that shortens silently is not.

### What the real data changed

The dictionary costs **4.0 bytes per address** compressed, against 1.9 on the
synthetic corpus — 2.1x worse, and the reason the synthetic numbers are
reported separately.

That is the measurement the design notes asked for. Extrapolating 4.0 bytes to
a nationwide block-level register of roughly 19 million addresses gives about
76 MB for the dictionary, which lands inside the 50–150 MB that was estimated
from a synthetic benchmark. Block-level addresses share longer prefixes than
town-level ones, so the true figure is lower: Tokyo at residence level costs
0.49 bytes an address (above), which puts the nationwide register nearer
20 MB.

Two further things the real data shows:

- **Coordinates are sparse at town level.** `mt_town_pos` has 337,641 points,
  and only 209,329 of them name a row of the town master, because the master's
  rows are mostly 小字 and the points are mostly for 大字・町 and 丁目. How the
  rest are placed is above, under *Where the coordinates come from*.
- **The registry spells chome two ways.** `一丁目` in one municipality and
  `１丁目` in the next. Indexed as written, the same concept sorts in two
  different places and neither is where someone typing would look, which is
  what `ja-address` is for. Normalized, `丸の内` comes back as 1, 2, 3 rather
  than in the arbitrary order the kanji give.

  It is not fully fixed. `10丁目` still sorts between `1丁目` and `2丁目`,
  because prefix order is byte order and zero-padding would break the matching
  it fixed the ordering for. Getting both means parsing the address into
  fields, which is a geocoder's job.

- **The registry spells place names two ways as well.** `保津町宮ノ上` and
  `保津町宮の上` are both in it, in the same town, and so are `塔ケ崎` and
  `塔ヶ崎` and `二之瀬` and `二の瀬`. `ja-address` folds those spellings onto one
  (`TOKENIZERS.md`), which is why `丸ノ内` completes and why the payload — not
  the key — is where the spelling the registry uses survives.

- **424 addresses collided under normalization** and their lines were dropped,
  reported by `build` rather than silently merged. 383 of them differ only in a
  folded character, which is the collision the folding exists to cause: two rows
  for one place become one record. Of the 413 folded keys with more than one
  original, **411 give at most one coordinate or agree on it**; the two that
  disagree — `塔ケ崎`/`塔ヶ崎` and `沢ケ内`/`沢ヶ内` — are two representative
  points for the same place, 400 and 900 metres apart.

  Which payload survives is still a data decision, so the count is printed
  rather than assumed to be harmless.

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

## The other shape

`../documents` indexes a collection of documents on the same layout: thousands
of terms per record, a ranked intersection for a query, and a section profile
that is the inverse of this one — postings two thirds of the file where here
they are nothing at all.

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
cd ../web && python3 serve.py --index ../addresses/tokyo.mokuji
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
meta                    274 B       274 B    raw       resident
term_index             11.1 KB      44.6 KB  deflate   resident
term_block_offsets     13.6 KB      13.6 KB  raw       resident
payload_offsets       218.1 KB     218.1 KB  raw       resident
postings                    0           0    raw       implied
term_blocks           861.4 KB      16.2 MB  deflate
payload                 6.0 MB       6.0 MB  raw       delta-coded integers
                       -------     -------
                        7.1 MB      22.5 MB   from a 120 MB corpus
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

**A coarser scale is available and is a decision, not an optimization.**
`ints:5` is about a metre and would take payload from 6.0 MB to 3.7 MB. Whether
a metre is enough is the publisher's call; the format's part is that the choice
ends up in the metadata either way.

**Larger dictionary blocks are still on the table.** The default is 1024 terms;
4096 makes this index 6.8 MB and a query 1.5 KB. Which is better depends on
whether bytes or requests are the scarce thing where it is served.
