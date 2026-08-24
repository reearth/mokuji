# Address lookup in a browser

The same `.mokuji` file, read by a page. No server logic, no API, no
WebAssembly, and no bundled decompressor.

Build the Tokyo index first — see [../addresses](../addresses) — then:

```sh
python3 serve.py --index ../addresses/tokyo.mokuji
```

Open <http://localhost:8787/> and type a prefix. The panel counts the range
requests as you type.

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
