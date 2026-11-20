# @reearth/mokuji

Read [Mokuji](../README.md) files over HTTP range requests, in a browser, a
Cloudflare Worker or Node. No dependencies, no WebAssembly, and no bundled
decompressor: the required codec is raw deflate, which every runtime already
has as `DecompressionStream("deflate-raw")`. About 13 KB gzipped.

It is the third implementation of [SPEC.md](../SPEC.md), after the Rust crates
and the Python fixture builder, and answers to the same conformance cases,
read counts included.

> Installed from git rather than npm while the format is a draft (below).

## Use

```ts
import { MokujiIndex, MokujiCollection } from "@reearth/mokuji";

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
Every result is accompanied by `index.stats` — the range requests and bytes it
cost — and `index.attribution` / `index.license`, the credit the file states.

### Reading from something other than a URL

Give `open` a `Source` instead of a URL: anything with
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

## Install

`dist/` is committed and checked by CI to be what `src/` builds, so the package
installs from git with no build step. With pnpm:

```sh
pnpm add "git+https://github.com/reearth/mokuji.git#<commit>&path:/js"
```

Pin a commit: the format and this API may change before 1.0.

## Develop

```sh
npm ci
npm test      # builds dist/, then runs the conformance cases against it
```

Commit `dist/` with any change to `src/`.
