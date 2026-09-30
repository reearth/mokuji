# Four pages, one reader

The same `.mokuji` file, read by a page. No server logic, no API, no
WebAssembly, and no bundled decompressor.

`index.html` completes an address prefix. `reverse.html` does the other
direction on a map: click a point, get the 町丁目 that holds it. `stac.html`
finds satellite images by word, place and period in a static STAC catalog; see
[../stac](../stac). `complete.html` completes a place name as it is typed,
from every place in GeoNames. All four are the same `mokuji.js` over the same
file layout.

Build the indexes first — [../addresses](../addresses) and
[../reverse](../reverse) — then:

```sh
python3 serve.py --index ../addresses/tokyo.mokuji --reverse ../reverse/reverse.mokuji
```

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
back. **3 to 5 range requests over 6,021 boundaries in 2.6 MB.**

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

`mokuji.js` is about 300 lines and pulls in nothing. That is possible because
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

Two things have to hold, and both fail at the transport rather than in the
format:

- The object must be served with `Accept-Ranges: bytes`, and any CDN in front
  of it must pass `Range` through rather than collapsing it into a full-object
  fetch.
- If the page is on a different origin from the object, CORS must allow the
  `Range` request header and expose `Content-Range`, which a reader needs to
  learn the file's length.

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

