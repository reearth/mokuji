# Address lookup in a browser

The same `.colex` file, read by a page. No server logic, no API, no
WebAssembly, and no bundled decompressor.

```sh
cd ../addresses && python3 generate.py && \
  cargo run --release -q -p colex-cli -- build addresses.tsv addresses.colex --tokenizer identity
cd ../web && python3 serve.py --index ../addresses/addresses.colex
```

Then open <http://localhost:8787/> and type a prefix. The panel counts the
range requests as you type.

Completing a prefix over 300,000 addresses costs **three `Range` requests and
about 700 bytes** from a 3.8 MB file, and the same three at any file size.

## Why there is no WebAssembly here

`colex.js` is about 300 lines and pulls in nothing. That is possible because
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
