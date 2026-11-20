# Reverse geocoding, without a spatial index

A concordance has no R-tree, no quadtree, nothing that knows what a coordinate
is. It does not need one to answer *which polygon holds this point*.

A geohash cell is a string whose **prefixes are the cells that contain it**.
`xn76urx` sits inside `xn76ur`, which sits inside `xn76u`, and so on up. Spatial
containment is already string containment, so it is already the thing a sorted
term dictionary does — and the whole spatial part of the query collapses into
ordinary term lookups.

Each polygon is covered by a set of geohash cells: coarse ones where its
interior is thick, fine ones along its boundary. Those cells are its terms. A
cell that straddles a boundary names every polygon it touches, so a lookup
returns a **candidate set**, not an answer, and the client settles it with a
point-in-polygon test against the geometry in the payload.

The index narrows. It does not decide.

```sh
./fetch-boundaries.sh 13
python3 covering.py --geojson boundaries/boundaries.geojson --out reverse.tsv \
    --max-precision 7 --label-fields PREF_NAME,CITY_NAME,S_NAME
mokuji build reverse.tsv reverse.mokuji --tokenizer space-split \
    --no-freqs --no-doc-lengths \
    $(python3 ../provenance.py args boundaries/pref13.zip) --source covering.py \
    --meta geohash-precisions=6,7 --meta geometry=delta-int-1e6 \
    --attribution "出典：政府統計の総合窓口(e-Stat)（令和2年国勢調査 小地域境界）を加工して作成"
```

```
$ python3 locate.py reverse.mokuji 35.681236 139.767125   # Tokyo Station
cell      xn76urx
lookups   2 (xn76ur, xn76urx)
candidate 1
reads     3
answer    東京都千代田区丸の内一丁目

$ python3 locate.py reverse.mokuji 35.658581 139.745433   # Tokyo Tower
cell      xn76ggr
lookups   2 (xn76gg, xn76ggr)
candidate 3
reads     4
answer    東京都港区芝公園四丁目

$ python3 locate.py reverse.mokuji 27.094 142.192         # Chichijima, 1000 km south
cell      xhv45sm
lookups   2 (xhv45s, xhv45sm)
candidate 2
reads     3
answer    東京都小笠原村父島字西町
```

| | |
|---|---|
| Polygons | 6,021 町丁目, all of Tokyo |
| Vertices | 562,692, at full source precision |
| Cells | 170,266 — 28.3 per polygon |
| Terms | 105,552 |
| Index | 2.1 MB, from a 19 MB GeoJSON |
| One point | 3–4 range reads |

## In a browser

`../web/reverse.html` is the same query on a map: click a point, watch the range
requests. It draws the candidates the index returned in orange and the one that
survived the point-in-polygon test in blue, which is the whole design in one
picture.

```sh
python3 ../web/serve.py --index ../addresses/tokyo.mokuji --reverse reverse.mokuji
```

## Choosing how tightly to cover

The only tuning knob is how fine the boundary cells go. Finer means the covering
hugs the polygon, so fewer wrong candidates come back — and it means far more
terms. The geometry is the same 1.77 MB in every row below; what changes is the
index on top of it.

| Max precision | Cell | Cells | Terms | Index | File | Candidates per query |
|---|---|---|---|---|---|---|
| 6 | 1.2 km × 0.6 km | 20,573 | 4,380 | 38 KB | 1.81 MB | 9.28 |
| **7** | **153 m × 153 m** | **170,266** | **105,552** | **224 KB** | **1.99 MB** | **2.07** |
| 8 | 38 m × 19 m | 2,076,529 | 1,721,295 | 1.72 MB | 3.49 MB | 1.20 |

Precision 7 is the choice here. Precision 8 buys 0.87 fewer candidates for
eight times the index, and it is the wrong shape besides: geohash cells are
square at odd precisions and 2:1 at even ones, so each step down alternates
between halving one side and halving the other.

Precision 6 is not obviously wrong — it is the smallest file by far — but a
candidate costs a payload read of real geometry, which is the expensive thing
here, and nine of them is worse than a bigger dictionary.

## Is it correct?

A covering has to be a **superset** of its polygon. Over-covering costs a wasted
candidate; under-covering loses an answer, silently, for points near a boundary
— which is exactly where reverse geocoding is asked and exactly where a
sampled test would not look.

So the test samples there on purpose: 20,000 points scattered within 2×10⁻⁶°
(about 20 cm) of a polygon vertex, each one answered from the index and from a
brute-force scan of all 6,021 polygons.

```
20000 boundary-adjacent points, 19382 inside a polygon
disagreements with brute force: 0
candidates/query 2.63
```

## What this demonstrates

**The I/O bound is the same one.** `SPEC.md` §4 gives 2k + m reads for a k-term
query returning m documents. Reverse geocoding is that query with k = 2: one
lookup per precision the index declares, plus the payload blocks the candidates
land in. Nothing about it is spatial from the format's side.

**A file can carry what the format has no opinion about.** A reader has to know
which precisions to probe, and the format has no business defining that — so
`--meta geohash-precisions=6,7` records it in the application-defined `attrs`
map the specification already reserved (§6). Without it a reader would have to
try every precision from 1 up: still bounded, just seven lookups instead of two.
This demo is why the flag exists.

**Documents in spatial order make payload blocks work.** The candidates for any
query are always polygons near each other, and a payload block holds 64
documents. `covering.py` sorts features by the geohash of their extent, so
neighbours share blocks and three candidates usually cost one or two reads
rather than three.

**The geometry is the file.** 1.77 MB of the 1.99 MB is polygon coordinates, and
the covering that makes them findable is 224 KB. That ratio is the honest shape
of the problem: reverse geocoding is mostly the cost of carrying boundaries.

**Postings were not tuned for this shape, and now are.** When Tokyo was first
built, 603 KB of its 718 KB index was the postings section — 3.5 bytes per
posting, for document IDs that fit in 13 bits. Most terms here name exactly one
polygon, and a list of one does not amortize the bit-packed groups a run is
built from. Such postings are now written in the dictionary entry (`SPEC.md`
§5.3.4); see *The whole country* below for what that saved.

## Where it stops

**FlatGeobuf is the established way to do this, and GIS tools read it.** Its
packed Hilbert R-tree narrows a point to bounding boxes, a covering narrows it
to cells, and either leaves the point-in-polygon test to the client. Measured
on the whole country, [below](#against-flatgeobuf), the R-tree narrows no
better and costs more: about eight requests and 150 KB a point, against two and
20 KB here, from a file four times the size.

What this is for is different: the same file holds the forward direction too.
An address key and a geohash cell occupy disjoint ranges of one sorted
dictionary — CJK and base32 share no prefixes — exactly as a postal code and an
address do in `../postal`. Pass `--key` and each polygon carries both:

```sh
python3 covering.py --geojson boundaries/boundaries.geojson --out both.tsv \
    --max-precision 7 --label-fields PREF_NAME,CITY_NAME,S_NAME --key
```

```
$ mokuji prefix both.mokuji 東京都千代田区丸の内
東京都千代田区丸の内三丁目	…|139765268,35675264,-27,-40,-974,225,…
東京都千代田区丸の内二丁目	…|139765596,35679395,-676,-1789,-1518,341,…
東京都千代田区丸の内一丁目	…|139760997,35685716,2000,-435,1513,-337,…
query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload

$ python3 locate.py both.mokuji 35.681236 139.767125
answer    東京都千代田区丸の内一丁目
reads     3
```

The second direction costs 27 KB on a 2.02 MB file — 1.3% — because the
geometry, which is nearly all of it, is already there. One object, one reader,
both ways.

Whether that is worth giving up an R-tree is a judgement about the deployment,
not about which structure is better.

## The whole country

All 47 prefectures in one file, built and checked rather than estimated. The
keys from the address registry that `link.py` writes are described
[below](#linked-to-the-address-registry):

```sh
./fetch-boundaries.sh all
python3 link.py --fullset ../addresses/abr/mt_town_fullset_all.csv \
    --pos ../addresses/abr/pos --boundaries 'boundaries/pref??.geojson' --out links.tsv
for code in $(seq -w 1 47); do echo $code; done | xargs -P 7 -I{} \
  python3 covering.py --geojson boundaries/pref{}.geojson --out boundaries/cover{}.tsv \
    --max-precision 7 --label-fields PREF_NAME,CITY_NAME,S_NAME --key --skip HCODE=8154 \
    --terms links.tsv
cat boundaries/cover??.tsv > national.tsv
mokuji build national.tsv reverse-jp.mokuji --tokenizer space-split \
    --no-freqs --no-doc-lengths --docs-per-payload-block 32 \
    $(python3 ../provenance.py args boundaries/pref??.zip \
        ../addresses/abr/mt_town_fullset_all.csv.zip) \
    --source covering.py --source link.py \
    --meta geohash-precisions=5,6,7 --meta geometry=delta-int-1e6 \
    --attribution "出典：政府統計の総合窓口(e-Stat)（令和2年国勢調査 小地域境界）を加工して作成。アドレス・ベース・レジストリ（デジタル庁）, CC BY 4.0"
```

| | |
|---|---|
| Polygons | 231,668 町丁目, all 47 prefectures |
| Cells | 17,134,266 — 633 at precision 5, 222,901 at 6, the rest at 7 |
| Terms | 13,589,325: cells, census names and registry addresses |
| **Index** | **126 MB**, from a 435 MB corpus; 91.5 MB [to the metre](#two-editions-as-published-and-to-the-metre) |
| Opening | 1 request, 256 KB |
| A point | 4–6 range reads from the CLI; 2.1 requests and 19.8 KB in the browser |
| Build | 50 s, after 3 minutes of covering on 7 cores |

The browser figure is the mean over 500 points inside polygons, a random
町丁目 each, through `../web/reverse.js`, which reads neighbouring blocks in one
request.

Where the bytes went:

| Section | |
|---|---|
| Geometry (payload) | 103.1 MB |
| Dictionary blocks, with most postings in them | 22.3 MB |
| Postings | 0.4 MB |
| Everything resident | 0.2 MB |

The first build was 196 MB, and 81.6 MB of it was postings: 13.0 million cells,
72% naming one polygon and 24% two, each paying a run's fixed three bytes before
its document, in a section nothing compresses. That is the case `SPEC.md`
§5.3.4 now writes in the dictionary entry instead, where the document lands in
a deflated block beside the cells next to it, which name the same polygons.
Dictionary and postings together went from 94.5 MB to 19.8 MB, and a point
costs one read fewer, since its postings arrive with the term block.

Payload blocks are 32 documents rather than the default 64. A point fetches one
to three polygons, and a smaller block carries fewer neighbours along with
them: in the browser, 13.5 KB a point against 22.1 KB at seven landmarks, for a
file 1.8 MB larger. Sixteen documents would make it 9.1 KB, and push the
resident set past the 256 KB a reader asks for at open, which then takes two
requests.

Before the registry's keys were added, at the same seven landmarks and reading
each block on its own:

| | 196 MB build | 123 MB build |
|---|---|---|
| Opening | 1 request | 1 request |
| A point, in the browser | 3.7 requests, 20.1 KB | 2.9 requests, 13.5 KB |

### Two editions: as published, and to the metre

The geometry is nearly all of the file, and it is stored at the census's 10⁻⁶°,
about 10 cm — finer than a boundary layer drawn for 1:25,000 maps can claim.
`covering.py --decimals 5` rounds it to 10⁻⁵°, about a metre, and computes the
covering from the rounded polygons, so that the cells reach exactly what a
reader decodes. Whether a metre is close enough depends on what the answer is
used for, so both are built and the file says which it is:

```sh
python3 covering.py ... --decimals 5         # the same arguments otherwise
mokuji build ... --meta geometry=delta-int-1e5
```

`locate.py` and `../web/reverse.js` read the scale from that attribute rather
than assuming one.

| | 10⁻⁶° (as published) | 10⁻⁵° (a metre) |
|---|---|---|
| Index | 126.1 MB | **91.5 MB** |
| Geometry (payload) | 103.1 MB | 68.5 MB |
| Polygons | 231,668 | 231,668 — rounding drops none |
| Opening | 1 request | 1 request |
| A point, in the browser | 2.1 requests, 19.8 KB | 2.1 requests, 14.2 KB |

How often the rounded edition answers differently, against the published
geometry, both by brute force over every polygon:

| Points | Answered differently |
|---|---|
| 22,718 uniform over the land area | 5 (0.02%) |
| 19,660 uniform within a random 町丁目's extent, so each counts once | 39 (0.2%) |
| 19,517 within a metre of a boundary vertex, the worst case | 2,729 (14%) |

A point a metre from a boundary is where a GPS fix cannot tell the two sides
apart either. The rounded edition's coverings reach every polygon holding a
point in all three samples, and 200 points answered through the built file
agreed with brute force over its own geometry.

Rural boundaries are larger than any precision-6 cell, so the national covering
uses precision 5 where Tokyo's never does, and the file says so:
`geohash-precisions=5,6,7`, three lookups to Tokyo's two.

**Water areas are left out.** The census layer includes 351 polygons for lakes
and bays (`HCODE` 8154), most with no 町丁目 name. Kept, a point in Tokyo Bay
answers *東京都* and nothing more; `--skip HCODE=8154` drops them, and the same
point answers that no polygon holds it. Lake Biwa's 沖島町 still answers for the
water around the island, because the census draws that 町's boundary there.

**It is checked the way Tokyo was.** 20,000 points within 2×10⁻⁶° of a polygon
vertex, drawn from all 47 prefectures, each answered from the coverings and by
brute force over every polygon: no polygon holding a point that its covering
fails to reach. 300 of them were also answered through the built file by
`locate.py` and agreed with brute force in every case.

```
$ python3 locate.py reverse-jp.mokuji 35.025414 135.762057   # Kyoto Imperial Palace
candidate 1
reads     4
answer    京都府京都市上京区滋野京都御苑

$ python3 locate.py reverse-jp.mokuji 35.360556 138.727778   # the summit of Fuji
candidate 1
reads     4
answer    静岡県富士宮市富士山国有林
```

`../web/reverse.html?url=<url>` reads it from a bucket; see `../web`.

### Linked to the address registry

The town-level index in `../addresses` answers an address with a point. This
one answers a point with a polygon, and it can answer the address too, if the
two agree on what the address is called. `link.py` gives each polygon the keys
the town index answers with — the registry's town address, normalized by the
same `ja-address` the town index's terms were — so the term a town lookup
returns is a term here:

```
東京都千代田区丸の内一丁目
  -> town index:  東京都千代田区丸の内1丁目            2 requests
  -> this index:  東京都千代田区丸の内一丁目 (polygon)  2 requests, 9.1 KB
```

There is no published table to join the two on. The registry's position table
has a column for the census small area, `cns_bnd_s_area_kcode`, and in the
2026-11-15 release it is empty in every row. So each of the registry's 726,285
町字 in force is linked the way a person would link it:

| How | 町字 | |
|---|---|---|
| The name | 527,006 | the municipality codes agree and the names do once numerals, ヶ/ケ, の/ノ, 大字 and 字 are folded |
| Within | 28,003 | the census divides a 大字 the registry does not, and the census area's name begins with it |
| The point | 38,909 | no name matches, and the registry's own point for it falls in the polygon |
| Its 大字 | 57,285 | none of those, so it takes the polygons of its 大字・町 |
| Not linked | 75,082 | |

Of the name links whose 町字 has a point of its own, 151,197 hold it in one of
the polygons they name and 6,019, 3.8%, do not: places where two areas of one
municipality share a name, or where the census and the registry draw a 町 in
different places. Sampled through both files, 1,797 of 2,006 town addresses,
90%, came back with a polygon, 1.5 polygons each.

The 884,333 keys add 2.9 MB, 2.4%, mostly to the dictionary.

The link runs the other way too, in what a point answers with. The payload
writes the label as the census does, `北海道松前町字小島`: no 郡, chome in
kanji, which is a key of the town index for 71.9% of areas. Two options make
it the registry's instead:

```sh
python3 link.py ... --out links.tsv     # now with a third column, below
python3 covering.py ... --terms links.tsv \
    --payload-fields KEY_CODE,PREF_NAME,MUNICIPALITY,S_NAME,LINKED_ADDRESS \
    --municipalities ../addresses/abr/mt_town_fullset_all.csv
mokuji build ... --meta payload-fields=KEY_CODE,PREF_NAME,MUNICIPALITY,S_NAME,LINKED_ADDRESS
```

- `link.py` writes, beside an area's keys, the one 町字 its name matched,
  when it matched exactly one: 171,225 of 220,625 areas, 78%.
  `covering.py --terms` gives the feature that key as `LINKED_ADDRESS`.
- `--municipalities` gives each feature a `MUNICIPALITY`: the municipality
  as the registry writes it, 郡 included (`松前郡松前町`), by the census's
  PREF and CITY, the first five digits of the registry's `lg_code`. Eight
  codes of 2020 are not in the registry and keep the census's name:
  Hamamatsu's seven wards, replaced in 2024, and an unnamed area of Tokyo.
- `--payload-fields` writes those properties into the payload in place of
  the label, separated by U+001F, so a reader gets the parts and the codes
  on their own; `--meta payload-fields` says which they are.

A reader then answers with `LINKED_ADDRESS` where there is one, and
otherwise with the prefecture, `MUNICIPALITY` and `S_NAME` normalized by
`ja-address`: over 2,993 areas sampled, a key of the town index for 82.7%,
against 71.9% for the census's names. The rest are areas the two sources
call differently (`豊田市小原大倉町`) or the Hamamatsu wards. The keys of the
terms are unchanged, `links.tsv`'s first two columns byte for byte; the
national 1 m edition with the fields is 93.3 MB, 1.9 MB more.
`../web/reverse.js` shows a label as it is, so it shows these fields
unseparated.

### Rectangles

The covering answers *which polygons meet this area* from the same file with
no change to it. `reverse.js` `within()` covers the rectangle with the finest
cells that keep it to 32, looks up every term beginning with one of them (a
polygon with a cell inside) and every cell that holds one (a polygon covered
more coarsely), and tests each candidate against the rectangle. Measured
against FlatGeobuf below, same points and rectangles, every answer the same:

| Rectangle | Polygons | This index | FlatGeobuf |
|---|---|---|---|
| Tokyo Station, 0.5 km | 5 | 3 requests, 17 KB | 5 requests, 237 KB |
| Tokyo Station, 2 km | 59 | 8 requests, 67 KB | 8 requests, 345 KB |
| Tokyo Station, 10 km | 839 | 14 requests, 418 KB | 11 requests, 1,627 KB |
| Sapporo, 2 km | 221 | 12 requests, 101 KB | 8 requests, 371 KB |
| Rural Hokkaido, 10 km | 3 | 3 requests, 86 KB | 8 requests, 229 KB |

The requests are close only because the reader asks for neighbouring blocks
together. The cells covering a rectangle are short prefixes in neighbouring
dictionary blocks, and the documents are numbered in spatial order, so their
payload blocks are neighbours too; read one block at a time, the 10 km
rectangle is 68 requests. `SPEC.md` §4 allows a reader to coalesce, since it
only ever lowers the count.

Coarser cells mean fewer lookups and more candidates to discard. Covering the
10 km rectangle with 2 cells rather than 9 fetches 5,362 polygons for 839
answers, 2.0 MB.

### Against FlatGeobuf

The same 231,668 polygons and labels, written by the `flatgeobuf` crate with
its packed Hilbert R-tree, as GDAL writes it, and read by the `flatgeobuf` npm
package, against 500 points inside polygons:

| | This index | 1 m edition | FlatGeobuf |
|---|---|---|---|
| File | 126 MB | 91.5 MB | 534 MB |
| Opening | 1 request, 256 KB | 1 request, 256 KB | 1 request, 13 KB |
| A point | 2.1 requests, 19.8 KB | 2.1 requests, 14.2 KB | 7.8 requests, 150 KB |
| Candidates a point | 2.31 | 2.32 | 2.28 |

Every answer was the same, bar one point beside a boundary in the 1 m edition.
The candidates are as few either way: an R-tree of bounding boxes narrows as
well as a covering does. The difference is the bytes — FlatGeobuf stores a
coordinate as two uncompressed doubles, sixteen bytes a vertex, and walks its
tree a level a request.

FlatGeobuf is still what GIS tools read; this is not. QGIS and GDAL open it with
nothing written, and its features are a standard geometry encoding, where these
are `covering.py`'s own.

### What it was estimated to cost

Before it was built, the covering was run over all 47 prefectures and the byte
cost fitted on the seven that were built into indexes:

```
fit: 5.50 bytes per cell + 3.07 bytes per vertex
 pref       actual       fitted     err
   01   17,689,064   17,690,055    0.0%    Hokkaido
   04    4,551,305    4,720,288    3.7%    Miyagi
   13    2,641,698    2,667,739    1.0%    Tokyo
   23    5,001,157    4,760,708   -4.8%    Aichi
   27    3,154,770    3,118,776   -1.1%    Osaka
   40    4,878,047    4,888,592    0.2%    Fukuoka
   47    1,735,282    1,903,772    9.7%    Okinawa
```

Two things are worth reading off that. Hokkaido is 10% of the polygons and 14%
of the cells, because a rural 大字 has a long boundary and a covering pays for
boundary; and Tokyo, at 28 cells per polygon against a national mean of 74, is
the cheapest kind of geography there is.

Whether it ships as one 126 MB object or 47 averaging under 3 MB is a
publishing choice, not a format one — the reads are the same either way.

## Where it comes from

The 2020 Census small-area boundaries, published by Japan's Statistics Bureau
through e-Stat. One polygon per 町丁目 — the same unit the town-level address
index in `../addresses` is keyed by, which is what makes the two directions
joinable; `link.py` [joins them](#linked-to-the-address-registry) where it can.
Boundaries stop at 2020: a 住居表示 or a renaming since then is not in them,
which is one reason some of the registry's 町字 find no polygon.

e-Stat's terms allow reuse, commercial use included, with the credit
`出典：政府統計の総合窓口(e-Stat)` and a statement that the data was processed;
the build records both as the file's `--attribution`. The terms are e-Stat's own
rather than a standard license, so the file states no SPDX `--license`.

e-Stat sends no `Last-Modified`, so `fetch-boundaries.sh` records the date of
the newest file in each archive as the publisher's — 2022-06-23 for every
prefecture — and says in the `.source` record that that is where it came from.

Coordinates are stored as integers at 10⁻⁶°, delta-coded, in decimal.

Base64 of packed varints is smaller, and by much less than the raw sizes
suggest — 2.37 MB against 4.37 MB before compression, but 1.70 MB against 1.78 MB
after the payload section is deflated. Decimal costs 4.6% of the geometry, 3% of
the file, and buys a corpus that is a text file: readable, diffable, checkable by
eye. That is the trade this demo takes; a publisher shipping this at national
scale should take the other one.
