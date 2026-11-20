# mokuji (JavaScript)

Read [Mokuji](../README.md) files over HTTP range requests, in a browser, a
Cloudflare Worker or Node. No dependencies, no WebAssembly, and no bundled
decompressor: the required codec is raw deflate, which every runtime already
has as `DecompressionStream("deflate-raw")`. About 13 KB gzipped.

It is an implementation of [SPEC.md](../SPEC.md) beside the Rust crate and the
Python reader, and answers to the same conformance cases, read counts
included.

```sh
npm install mokuji
```

## Use

```ts
import { MokujiIndex, MokujiCollection } from "mokuji";

// One file. A URL makes the reader fetch ranges itself.
const index = await MokujiIndex.open("https://example.com/places.mokuji");
const { docs, payloads, fromTable } = await index.complete("東京タ", 10);

// A collection: a manifest over several files (MANIFEST.md), restricted to a
// place so that only the files there are opened.
const collection = await MokujiCollection.open("https://example.com/world.json", undefined, {
  countries: ["JP"],
});
const found = await collection.complete("tokyo", 10);
```

The query goes through the tokenizer the file records (`TOKENIZERS.md`)
before it is looked up, so `一丁目` finds `1丁目` in a `ja-address` index.

`search` ranks by BM25, the same ranking as `mokuji search` with the same
options, and splits the query the way the file's tokenizer does — into bigrams,
for a `ja-address-bigram` index:

```ts
const fuzzy = await MokujiIndex.open("https://example.com/abr-fuzzy.mokuji");
const { results } = await fuzzy.search("和光市", { boost: "municipality" });
// results[0].payload === "埼玉県和光市|139.605693,35.781104"

// Typed wrong: any of its bigrams, the six rarest, without length normalization.
await fuzzy.search("千代田区丸乃内", { any: true, probe: 6, lengthNorm: 0 });
```

The scores agree with `mokuji search`'s and the Python reader's to about
1e-12, not to the last bit. JavaScript's `Math.log` is not correctly rounded
(V8 is one unit in the last place off for about one input in seventy), so two
documents whose scores differ by less than that may come out in either order.
Every result is accompanied by `index.stats` — the range requests and bytes it
cost — and `index.attribution` / `index.license`, the credit the file states.

### Reading a file on hand

Give `open` a `Blob`, and it reads a slice at a time: a file on disk is
searched without being loaded into memory, and without a network. In Node,
`openAsBlob` from `node:fs`; in a page, a `File` the user picked, or one from
the origin private file system.

```ts
import { openAsBlob } from "node:fs";

const index = await MokujiIndex.open(await openAsBlob("./postal.mokuji"));
```

```ts
const index = await MokujiIndex.open(input.files[0]);
```

### Reading from something else

Give `open` a `Source` instead: anything with
`read(offset, length)` returning the bytes. That is how a Worker reads from an
R2 binding through its own cache, and how the tests read a file on disk.

```ts
const source = {
  async read(offset: number, length: number) {
    const object = await env.BUCKET.get("places.mokuji", { range: { offset, length } });
    return new Uint8Array(await object!.arrayBuffer());
  },
};
const index = await MokujiIndex.open(source);
```

## Write

`MokujiBuilder` is the Rust crate's `mokuji::write`. Documents are added
first, in ID order; postings then, terms in UTF-8 byte order (which is not
JavaScript's string order) and each term's documents ascending. The writer
checks the order and owns no sort. `split` turns a document's text into terms
as a tokenizer from `TOKENIZERS.md` does.

```js
import { MokujiBuilder, encodeMetadata, split, writeParams } from "mokuji";

const options = {};
const builder = new MokujiBuilder(options);
const doc = builder.addDocument("35.671,139.772");
// ... every document, then every posting, in order
builder.addPosting("chuo", doc);

const meta = encodeMetadata({
  sources: [{ contentHash: "sha2-256:…", uri: "places.tsv" }],
  generatorParams: writeParams(options),
  tokenizerId: "space-split",
});
const file = await builder.finish(meta); // a Uint8Array
```

The metadata is the caller's to compose: it names the source the index was
derived from, which the writer never sees (SPEC.md §6). Integers past 2^53,
in a payload row or a column, are given as bigints.

Stored raw (`compress: false`), a file is the Rust writer's byte for byte,
and the tests rebuild the conformance fixtures to show it. Compressed, it is
deflated by the runtime's own `CompressionStream("deflate-raw")`, so the
bytes depend on the runtime: no specification of deflate fixes which of the
valid encodings a compressor emits. Such a file says the same thing and every
reader reads it; its `generator.name` is `mokuji-js`, and a rebuild is
checked on the same runtime.

## Install

```sh
npm install mokuji
```

The format and this API may change before 1.0, so pin the version.

## Develop

```sh
npm ci
npm test      # builds dist/, then runs the conformance cases against it
```

Commit `dist/` with any change to `src/`.
