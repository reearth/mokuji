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
    --source boundaries/boundaries.geojson \
    --meta geohash-precisions=6,7 --meta geometry=delta-int-1e6
```

```
$ python3 locate.py reverse.mokuji 35.681236 139.767125   # Tokyo Station
cell      xn76urx
lookups   2 (xn76ur, xn76urx)
candidate 1
reads     4
answer    東京都千代田区丸の内一丁目

$ python3 locate.py reverse.mokuji 35.658581 139.745433   # Tokyo Tower
cell      xn76ggr
lookups   2 (xn76gg, xn76ggr)
candidate 3
reads     5
answer    東京都港区芝公園四丁目

$ python3 locate.py reverse.mokuji 27.094 142.192         # Chichijima, 1000 km south
cell      xhv45sm
lookups   2 (xhv45s, xhv45sm)
candidate 2
reads     4
answer    東京都小笠原村父島字西町
```

| | |
|---|---|
| Polygons | 6,021 町丁目, all of Tokyo |
| Vertices | 563,084, at full source precision |
| Cells | 170,266 — 28.3 per polygon |
| Terms | 105,552 |
| Index | 2.6 MB, from a 19 MB GeoJSON |
| One point | 4–5 range reads |

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
| 6 | 1.2 km × 0.6 km | 20,573 | 4,380 | 53 KB | 1.92 MB | 9.28 |
| **7** | **153 m × 153 m** | **170,266** | **105,552** | **718 KB** | **2.60 MB** | **2.07** |
| 8 | 38 m × 19 m | 2,076,529 | 1,721,295 | 9.8 MB | 12.13 MB | 1.20 |

Precision 7 is the choice here. Precision 8 buys 0.87 fewer candidates for
fourteen times the index, and it is the wrong shape besides: geohash cells are
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

**The geometry is the file.** 1.77 MB of the 2.6 MB is polygon coordinates, and
the covering that makes them findable is 718 KB. That ratio is the honest shape
of the problem: reverse geocoding is mostly the cost of carrying boundaries.

**Postings are not tuned for this shape.** 603 KB of the 718 KB index is the
postings section — 3.5 bytes per posting, for document IDs that fit in 13 bits.
Most terms here name exactly one polygon, and a list of one does not amortize
the bitpacking groups the encoding is built around. A text index, where a term
names many documents, never shows this.

## Where it stops

**FlatGeobuf does this better, and does it exactly.** A packed Hilbert R-tree
answers a point query in three or four reads with no approximation at all, and
that is what it is for. A covering is an approximation that a client has to
finish.

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
query:    3 read(s) total - 1 dictionary, 1 postings, 1 payload

$ python3 locate.py both.mokuji 35.681236 139.767125
answer    東京都千代田区丸の内一丁目
reads     4
```

The second direction costs 44 KB on a 2.60 MB file — 1.7% — because the
geometry, which is nearly all of it, is already there. One object, one reader,
both ways.

Whether that is worth giving up an R-tree is a judgement about the deployment,
not about which structure is better.

## Where it comes from

The 2020 Census small-area boundaries, published by Japan's Statistics Bureau
through e-Stat. One polygon per 町丁目 — the same unit the town-level address
index in `../addresses` is keyed by, which is what makes the two directions
joinable.

Coordinates are stored as integers at 10⁻⁶°, delta-coded, in decimal.

Base64 of packed varints is smaller, and by much less than the raw sizes
suggest — 2.37 MB against 4.37 MB before compression, but 1.70 MB against 1.78 MB
after the payload section is deflated. Decimal costs 4.6% of the geometry, 3% of
the file, and buys a corpus that is a text file: readable, diffable, checkable by
eye. That is the trade this demo takes; a publisher shipping this at national
scale should take the other one.
