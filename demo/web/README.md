# Four pages, one reader

The same `.mokuji` file, read by a page. No server logic, no API, no
WebAssembly, and no bundled decompressor.

`index.html` completes an address prefix. `reverse.html` does the other
direction on a map: click a point, get the 町丁目 that holds it. `stac.html`
finds satellite images by word, place and period in a static STAC catalog; see
[../stac](../stac). `complete.html` completes a place name as it is typed,
from every place in GeoNames. All four read with the same reader, the
`@reearth/mokuji` package in [../../js](../../js), over the same file layout.
`mokuji.js` here only re-exports its build, and `serve.py` serves `js/` so that
a browser can load it.

Build the indexes first — [../addresses](../addresses) and
[../reverse](../reverse) — then:

```sh
python3 serve.py --index ../addresses/tokyo.mokuji --reverse ../reverse/reverse.mokuji
```

Give it a second index, the bigram one from [../fuzzy](../fuzzy), and
`index.html` asks that one whenever no address starts with what was typed — a
municipality's name without its prefecture, or an address typed wrong:

```sh
python3 serve.py --index ../addresses/abr.mokuji --fuzzy ../fuzzy/fuzzy.mokuji
```

`埼玉県和光` completes from the first index in 2 requests and 4.5 KB. `和光市` is
no address's prefix; the page opens the second, 2 requests and 621 KB the first
time, and finds `埼玉県和光市` in 6 requests and 19 KB.

Open <http://localhost:8787/> and type a prefix, or
<http://localhost:8787/reverse.html> and click the map. The panel counts the
range requests either way.

Completing a prefix over Tokyo's 1,744,962 addresses costs **two to four
`Range` requests and well under a kilobyte** from an 8.7 MB file, and the same
number at any file size. One request per dictionary block the matches span and
one per payload block; there is no postings request at all, because in an index
of a dictionary the entry states its own postings (`SPEC.md` §5.3.4).

Opening costs two requests and 365 KB — the header, the term index, and the
offset tables for 1.7 million addresses — and is paid once.

Coordinates arrive as delta-coded integers (`SPEC.md` §5.5.3) and the page turns
them back into decimals using the scale the file declares. The format states how
a number was written down; what it measures is still nobody's business but the
application's.

## Clicking a map is a term lookup

`reverse.html` has no spatial index and does not need one. A geohash cell is a
string whose prefixes are the cells containing it, so the polygon under a point
is found by looking up two terms — the cell and its parent — and the page
finishes the job with a point-in-polygon test against the geometry that came
back. **2 or 3 range requests over 6,021 boundaries in 2.0 MB**: the reader
asks for neighbouring blocks in one request, which a point's candidates, being
neighbours, usually are.

`reverse.js` also answers which polygons meet a rectangle, `within()`, from the
same file; `../reverse/README.md` has what it costs against FlatGeobuf.

The map draws both halves of that: orange for what the index returned, blue for
what survived the test. A cell straddling a boundary names every polygon it
touches, so at Shibuya crossing four candidates come back and one is the answer.
The index narrows; the client decides. See [../reverse](../reverse) for how the
covering is built and what it costs.

`reverse.js` is that client half — geohash, geometry decoding, point-in-polygon
— and none of it is in `mokuji.js`, because none of it is the format's business.
What the format contributes is `exactMany`: several terms in one read per
dictionary block, which is what makes two lookups cost one request rather than
two.

The map itself is Leaflet, loaded from a CDN with an integrity hash. It is the
one dependency in this directory, and drawing tiles and polygons is not a
problem worth solving again here. Leaflet and the basemap tiles are also the
only requests that page makes which are not range reads into the one file.

## Normalization happens here too

The index records that it was built with the `ja-address` tokenizer, so the page
puts the query through the same normalization before searching. Typing `一丁目`
finds `1丁目`, which matters because the Address Base Registry spells chome both
ways depending on the municipality.

That means `mokuji.js` implements the tokenizer as well as the format — a
reminder that the format deliberately does not specify tokenization, but does
make sure a reader can find out what was done. It reads the identifier from the
metadata section, which is why it also carries about forty lines of CBOR.

## Why there is no WebAssembly here

The reader is 13 KB gzipped and pulls in nothing. That is possible because
the required compression codec is raw deflate, which browsers implement as
`DecompressionStream("deflate-raw")`. Had the format required zstd, this page
would have to ship a decompressor several times its own size, and the argument
for reading an index directly from object storage would be weaker for it.

This is also the third implementation of `SPEC.md`, after the Rust one and the
Python fixture builder, and the first written against the transport the format
was designed for. It fetches exactly the byte ranges the Rust reader does.

## What the server is for

Python's `http.server` does not implement `Range`, and without it the browser
downloads the whole file — the demo would appear to work while doing the one
thing the format exists to avoid. `serve.py` adds range support and sets
`Cache-Control: no-store`, so the request count on screen is real rather than
partly served from cache.

## Publishing this to object storage

`index.html?url=<url>` reads the index from that URL instead of the one
`serve.py` serves, and `&fuzzy=<url>` the second index, so the page and the
files can live in different places: the page on any static host, the files in a
bucket. `reverse.html?url=<url>` does the
same for a reverse index, such as the national one in [../reverse](../reverse).
Both pages show the credit the file states and the dates its sources were
published (`SPEC.md` §6).

```
https://example.com/index.html?url=https://data.example.com/abr-town.mokuji&fuzzy=https://data.example.com/abr-fuzzy.mokuji
```

The town-level index in [../addresses](../addresses) is the one to publish for
the whole country: 4.7 MB, every address with a point, and a `pos_level`
column that the page shows as *Placed at* beside each result, so a 小字 placed
at its municipality's centre says so. The credit under the search box is the
one the file states.

Two things have to hold, and both fail at the transport rather than in the
format:

- The object must be served with `Accept-Ranges: bytes`, and any CDN in front
  of it must pass `Range` through rather than collapsing it into a full-object
  fetch.
- If the page is on a different origin from the object, CORS must allow the
  `Range` request header and expose `Content-Range`, which a reader needs to
  learn the file's length.

On Cloudflare R2 that is the bucket's CORS policy; R2 itself answers `Range`
requests:

```json
[
  {
    "AllowedOrigins": ["https://example.com"],
    "AllowedMethods": ["GET", "HEAD"],
    "AllowedHeaders": ["Range"],
    "ExposeHeaders": ["Content-Range", "Content-Length", "ETag"],
    "MaxAgeSeconds": 86400
  }
]
```

Name the object by what it was built from, such as
`abr-town-2026-08.mokuji`, and never overwrite it: a reader that opened the old
file and reads a range of the new one gets bytes from a different layout. A new
registry release is a new object, and the page's `?url=` moves to it.

## Completing a name as it is typed

`complete.html` answers every keystroke from one file of all 12,854,245 places
in GeoNames, each under every name it has. Build it — the sort numbers the
places most-named first, which is what makes the first documents of a prefix
the ones worth showing:

```sh
python3 ../poi/geonames.py --input allCountries.txt --out names.tsv --whole-names
LC_ALL=C sort -t$'\t' -k6,6nr -s names.tsv > names-ranked.tsv
mokuji build names-ranked.tsv names.mokuji --tokenizer name-list \
  --no-doc-lengths --docs-per-payload-block 256 --completions 1024,16384,10 \
  --license CC-BY-4.0 --attribution "GeoNames (geonames.org), CC BY 4.0"
python3 serve.py --names names.mokuji
```

Sorting the corpus rather than passing `--order-by` gives the same numbering
without a column, and the page wants none: columns and document lengths are
resident, and at this size they would make opening cost tens of megabytes.
As built, opening costs **2 requests and 816 KB** of a 406 MB file — the term
index, the offset tables, and the completions section.

A prefix with an entry in that section costs **no request before its
payload**: `s`, `mont`, `สถานี`. One without an entry is guaranteed cheap —
fewer than 1,024 terms and 16 KB of postings — and costs a few: `black se`,
`東京タ` and `zü` are each 2 requests and 6–12 KB before their payload. The panel says
which each keystroke was, and the log lists every range it read.

The credit under the map is the one the file states (`SPEC.md` §6), not one the
page was written to know: point the page at an index built from OpenStreetMap
and it shows OpenStreetMap's.

The page runs one query at a time and then the latest thing typed, so the count
beside a keystroke is that keystroke's alone.

## A collection: the base, and what changed since

`complete.html?manifest=<url>` reads a collection (`MANIFEST.md`) instead of
one file: a base, the segments added after it, and the documents deleted from
each. This one adds Takanawa Gateway Station, which opened in 2020 and which
GeoNames does not have. It also replaces Takanawadai Eki with Takanawadai
Station, which gains its Japanese name: the old document is deleted from the
base, and the new one is added in the delta.

```sh
mkdir collection && ln -s ../names.mokuji collection/base-2026-09.mokuji
printf 'Takanawa Gateway Station\x1fTakanawa Gateway\x1f高輪ゲートウェイ駅\x1f高輪ゲートウェイ\tTakanawa Gateway Station\t139.74077\t35.63551\t0\t4\n' > delta.tsv
printf 'Takanawadai Station\x1fTakanawadai Eki\x1f高輪台駅\tTakanawadai Station\t139.73019\t35.63155\t0\t3\n' >> delta.tsv
mokuji build delta.tsv collection/delta-2026-09-30.mokuji --tokenizer name-list \
  --no-doc-lengths --docs-per-payload-block 256 --completions 1024,16384,20 \
  --license CC-BY-4.0 --attribution "GeoNames (geonames.org), CC BY 4.0"
mokuji manifest new collection/names.json collection/base-2026-09.mokuji
mokuji manifest add collection/names.json collection/delta-2026-09-30.mokuji
mokuji search collection/names.json "takanawadai eki"      # base, document 498492
mokuji manifest delete collection/names.json base-2026-09.mokuji 498492
python3 serve.py --collection collection    # open /complete.html?manifest=collection/names.json
```

`takanawad` then completes to the post office in the base and to the station in
the delta, marked *added*. The old station is gone. `takanawa g` finds only
the new one. Opening costs one request for the manifest and one per segment,
asked at once. The log marks each range with the segment it read.

Collection order puts the base before the delta, so a new place never
completes a prefix the base already fills with ten. With *rank across
segments* checked, the page instead asks every segment for its first ten and
ranks them by how many names each place has, which is in the payload. The page
brings that score, not the format (`MANIFEST.md` §3.3). This delta's stations
have three or four names each, so they still lose to the base's; a place known
by more names would not.

`serve.py` reads only the range asked for. It used to read the whole file for
every request, which a 400 MB index made slow enough to notice.

## Shards: a collection cut by place

The same places, cut into seven continents by GeoNames' own country table,
each a segment whose manifest entry states its countries (`MANIFEST.md` §7).

```sh
curl -O https://download.geonames.org/export/dump/countryInfo.txt
for c in AF AN AS EU NA OC SA; do
  codes=$(grep -v '^#' countryInfo.txt | awk -F'\t' -v c=$c '$9==c{print $1}' | paste -sd, -)
  python3 ../poi/geonames.py --input allCountries.txt --out $c.tsv --whole-names --countries "$codes"
  LC_ALL=C sort -t$'\t' -k6,6nr -s $c.tsv > $c.ranked.tsv
  mokuji build $c.ranked.tsv shards/$c.mokuji --tokenizer name-list --no-doc-lengths \
    --docs-per-payload-block 256 --completions 1024,16384,20 \
    --license CC-BY-4.0 --attribution "GeoNames (geonames.org), CC BY 4.0"
done
mokuji manifest new shards/world.json shards/{AF,AN,AS,EU,NA,OC,SA}.mokuji
# then, for each continent:
mokuji manifest region shards/world.json AS.mokuji "$codes"
python3 serve.py --collection shards
```

`/complete.html?manifest=collection/world.json&countries=JP` opens Asia alone:
3 requests and 361 KB, with the manifest. Without `countries` it opens all
seven: 9 requests and 1.9 MB, since this reader asks for 256 KB when it opens a
file and most of these are small enough to fetch whole. Each keystroke costs the
same three round trips either way. Results are tagged with the shard they came
from, and ranked across shards by name count.

