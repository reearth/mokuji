// The writer is held to the fixtures, not the other way round.
//
// The raw fixtures were built by conformance/fixtures/build_fixtures.py, an
// independent implementation written from SPEC.md, and the Rust writer
// reproduces them byte for byte. If this writer does too, they agree on the
// layout down to the last varint. Run with `npm test` in js/; it tests the
// built dist/.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { MokujiBuilder, MokujiIndex, encodeCbor, encodeMetadata, split, writeParams } from "../dist/index.js";

const FIXTURES = fileURLToPath(new URL("../../conformance/fixtures/", import.meta.url));
const encoder = new TextEncoder();

const bytesSource = (bytes) => ({ read: (offset, length) => bytes.subarray(offset, offset + length) });
const open = (bytes) => MokujiIndex.open(bytesSource(bytes));
const fixture = async (name) => new Uint8Array(await readFile(`${FIXTURES}${name}.mokuji`));

/** A fixture's metadata section, verbatim, so a comparison of layouts is not one of CBOR. */
async function metaOf(file) {
  const meta = (await open(file)).sections.get(0x0001);
  return file.subarray(meta.offset, meta.offset + meta.length);
}

async function corpus(name) {
  const text = await readFile(`${FIXTURES}${name}.corpus.txt`, "utf8");
  return text
    .replace(/\n$/, "")
    .split("\n")
    .map((line) => {
      const tab = line.indexOf("\t");
      return [line.slice(0, tab), line.slice(tab + 1)];
    });
}

function compareBytes(a, b) {
  for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
}

/** Postings in the order a builder takes them: terms by UTF-8 bytes, documents ascending, with places. */
function invert(texts, tokenizer) {
  const places = new Map();
  texts.forEach((text, doc) => {
    split(tokenizer, text).forEach((token, at) => {
      if (!places.has(token)) places.set(token, new Map());
      const byDoc = places.get(token);
      if (!byDoc.has(doc)) byDoc.set(doc, []);
      byDoc.get(doc).push(at);
    });
  });
  return [...places]
    .map(([term, byDoc]) => [term, [...byDoc].sort((a, b) => a[0] - b[0])])
    .sort((a, b) => compareBytes(encoder.encode(a[0]), encoder.encode(b[0])));
}

async function buildTiny(meta, compress) {
  const builder = new MokujiBuilder({ termsPerBlock: 2, docsPerPayloadBlock: 2, positions: true, docLengths: true, compress });
  const rows = await corpus("tiny");
  for (const [, payload] of rows) builder.addDocument(payload);
  for (const [term, docs] of invert(rows.map(([text]) => text), "space-split")) {
    for (const [doc, places] of docs) builder.addPostingAt(term, doc, places);
  }
  return builder.finish(meta);
}

const parseValues = (text, decimals) =>
  text.split(",").map((value) => {
    const [whole, fraction = ""] = value.trim().split(".");
    return BigInt(whole + (fraction + "0".repeat(decimals)).slice(0, decimals));
  });

async function buildDict(meta, compress) {
  const builder = new MokujiBuilder({
    termsPerBlock: 2,
    docsPerPayloadBlock: 2,
    ints: { valuesPerDoc: 2, scaleExponent: -6 },
    compress,
  });
  const rows = await corpus("dict");
  for (const [, payload] of rows) builder.addDocumentValues(parseValues(payload, 6));
  // The corpus is already in term order, which is what lets every term's
  // postings be implied.
  rows.forEach(([term], doc) => builder.addPosting(term, doc, 1));
  builder.addColumn({ name: "importance", scaleExponent: 0, values: [40, 90, 85, 70, 60] });
  return builder.finish(meta);
}

async function buildNames(name, meta, compress) {
  const builder = new MokujiBuilder({
    termsPerBlock: 2,
    docsPerPayloadBlock: 2,
    compress,
    completions: { minTerms: 2, docsPerEntry: 2 },
    // As the fixture builder writes `names`: every run stored.
    inlinePostings: 0,
  });
  const rows = await corpus(name);
  for (const [, payload] of rows) builder.addDocument(payload);
  for (const [term, docs] of invert(rows.map(([text]) => text), "name-list")) {
    for (const [doc, places] of docs) builder.addPosting(term, doc, places.length);
  }
  return builder.finish(meta);
}

const BUILDS = {
  tiny: buildTiny,
  dict: buildDict,
  names: (meta, compress) => buildNames("names", meta, compress),
  "names-delta": (meta, compress) => buildNames("names-delta", meta, compress),
};

test("reproduces the raw fixtures byte for byte", async () => {
  for (const [name, build] of Object.entries(BUILDS)) {
    const expected = await fixture(name);
    const meta = await metaOf(expected);
    assert.deepEqual(await build(meta, false), expected, name);
    // And again: one input, one file.
    assert.deepEqual(await build(meta, false), expected, name);
  }
});

test("metadata encodes as the fixtures' does", async () => {
  // Canonical CBOR has one encoding for one value.
  for (const name of Object.keys(BUILDS)) {
    const file = await fixture(name);
    const index = await open(file);
    const encoded = encodeMetadata({
      generation: index.meta.generation,
      sources: index.sources,
      generatorName: index.meta.generator.name,
      generatorVersion: index.meta.generator.version,
      generatorParams: index.meta.generator.params,
      tokenizerId: index.tokenizerId,
      tokenizerParams: index.meta.tokenizer.params,
      attrs: index.meta.attrs ?? null,
      license: index.license,
      attribution: index.attribution,
    });
    assert.deepEqual(encoded, await metaOf(file), name);
  }
});

test("canonical CBOR", () => {
  const hex = (bytes) => Buffer.from(bytes).toString("hex");
  assert.equal(hex(encodeCbor(0)), "00");
  assert.equal(hex(encodeCbor(24)), "1818");
  assert.equal(hex(encodeCbor(256)), "190100");
  assert.equal(hex(encodeCbor(2 ** 32)), "1b0000000100000000");
  assert.equal(hex(encodeCbor("a")), "6161");
  assert.equal(hex(encodeCbor([1, "b"])), "82016162");
  // Keys sorted by their encoding: shorter first, then bytewise.
  assert.equal(hex(encodeCbor({ bb: 1, c: 2, a: 3 })), "a361610361630262626201");
  for (const bad of [-1, 1.5, null, new Date(0)]) assert.throws(() => encodeCbor(bad));
});

/** Answers to the questions the readers' tests ask, about one file. */
async function answers(file) {
  const index = await open(file);
  const out = [];
  for (const prefix of ["", "b", "bl", "black", "c", "t", "tokyo", "東", "東京都千代田区丸の内", "x"]) {
    index.coalesce = false;
    index.resetStats();
    const lookup = await index.lookup(prefix, 100);
    const done = await index.complete(prefix, 3);
    out.push([prefix, lookup.results, done.docs, done.payloads, done.fromTable, index.stats.requests]);
  }
  const found = await index.search("tokyo chuo", { any: true });
  out.push(found.results.map((r) => [r.doc, r.payload]));
  return out;
}

test("compressed files answer as the fixtures they rebuild do", async () => {
  for (const [name, build] of Object.entries(BUILDS)) {
    const raw = await fixture(name);
    const compressed = await build(await metaOf(raw), true);
    // The same answers, at the same number of reads; only the bytes differ.
    assert.deepEqual(await answers(compressed), await answers(raw), name);
  }
  // Where nothing shrinks, as in names-delta, a compressed file is the raw
  // one; but the term index of dict shrinks under any compressor, or nothing
  // above was about deflate.
  const dict = await buildDict(await metaOf(await fixture("dict")), true);
  assert.equal((await open(dict)).sections.get(0x0010).codec, 1);
});

test("rejects what is out of order", async () => {
  const builder = new MokujiBuilder();
  builder.addDocument("a");
  builder.addDocument("b");
  builder.addPosting("tokyo", 1);
  assert.throws(() => builder.addPosting("tokyo", 0));
  assert.throws(() => builder.addPosting("osaka", 0));
  assert.throws(() => builder.addPosting("zz", 7));
  assert.throws(() => builder.addPosting("zz", 0, 0));
  assert.throws(() => builder.addPostingAt("zz", 0, [2, 1]));
  assert.throws(() => builder.addSection(0x0040, new Uint8Array()));
  assert.throws(() => builder.addDocumentValues([1]));
  // Terms ascend by UTF-8 bytes: U+FF5E sorts after U+1F600 as UTF-16 does
  // not.
  const bytewise = new MokujiBuilder();
  bytewise.addDocument("a");
  bytewise.addPosting("～", 0);
  bytewise.addPosting("😀", 0);
  await assert.rejects(new MokujiBuilder().finish(new Uint8Array()));
  assert.throws(() => new MokujiBuilder({ termsPerBlock: 0 }));
  assert.throws(() => new MokujiBuilder({ positions: true, hasFreqs: false }));
  assert.throws(() => new MokujiBuilder({ completions: { docsPerEntry: 1 } }));
});

test("integers past 2^53 are written exactly", async () => {
  const builder = new MokujiBuilder({ ints: { valuesPerDoc: 1, scaleExponent: 0 }, compress: false });
  const values = [2n ** 63n - 1n, -(2n ** 63n), 0n];
  values.forEach((v) => builder.addDocumentValues([v]));
  values.forEach((_, doc) => builder.addPosting(`k${doc}`, doc, 1));
  const file = await builder.finish(encodeMetadata({ sources: [], tokenizerId: "identity" }));
  // Read back by hand: the reader here holds payload integers as numbers, so
  // it cannot say these exactly. The block is a count, then each value's
  // difference from the one before, zigzagged.
  const payload = (await open(file)).sections.get(0x0030);
  let at = payload.offset;
  const varint = () => {
    let v = 0n;
    for (let shift = 0n; ; shift += 7n) {
      const b = file[at++];
      v |= BigInt(b & 0x7f) << shift;
      if (b < 0x80) return v;
    }
  };
  assert.equal(varint(), 3n);
  let value = 0n;
  const read = [];
  for (let i = 0; i < 3; i++) {
    const z = varint();
    value = BigInt.asIntN(64, (i === 0 ? 0n : value) + ((z >> 1n) ^ -(z & 1n)));
    read.push(value);
  }
  assert.deepEqual(read, values);
  assert.throws(() => new MokujiBuilder({ ints: { valuesPerDoc: 1, scaleExponent: 0 } }).addDocumentValues([2 ** 60]));
});

test("random corpora read back as they were written", async () => {
  // Every combination of options that changes the layout, read back: every
  // term finds exactly the documents, payloads and positions it was given,
  // and a completion answers as walking the dictionary would.
  let seed = 1;
  const random = () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31);
  const int = (lo, hi) => lo + Math.floor(random() * (hi - lo + 1));
  const words = ["a", "ab", "abc", "b", "東京", "東京都", "tokyo", "z", "zz", "丸の内"];
  for (let round = 0; round < 40; round++) {
    const texts = Array.from({ length: int(1, 40) }, () =>
      Array.from({ length: int(1, 5) }, () => words[int(0, words.length - 1)]).join(" "),
    );
    const options = {
      termsPerBlock: int(1, 4),
      docsPerPayloadBlock: int(1, 5),
      positions: random() < 0.5,
      docLengths: random() < 0.5,
      compress: random() < 0.5,
      inlinePostings: int(0, 9),
      completions: random() < 0.5 ? { minTerms: int(1, 3), docsPerEntry: int(1, 3) } : null,
    };
    const builder = new MokujiBuilder(options);
    texts.forEach((_, i) => builder.addDocument(`doc ${i}`));
    const inverted = invert(texts, "space-split");
    for (const [term, docs] of inverted) for (const [doc, places] of docs) builder.addPostingAt(term, doc, places);
    const meta = encodeMetadata({ sources: [], generatorParams: writeParams(options), tokenizerId: "space-split" });
    const index = await open(await builder.finish(meta));
    for (const [term, docs] of inverted) {
      const entry = await index.exact(term);
      const [postings] = await index.postingsSpan([entry]);
      assert.deepEqual(postings.docs, docs.map(([d]) => d), `${round} ${term}`);
      assert.deepEqual(postings.freqs, docs.map(([, p]) => p.length));
      assert.deepEqual(await index.payloadSpan(postings.docs), docs.map(([d]) => `doc ${d}`));
      if (options.positions) assert.deepEqual(await index.positions(entry, postings), docs.map(([, p]) => p));
    }
    for (const prefix of ["a", "東", "z", "q"]) {
      const p = encoder.encode(prefix);
      const want = [
        ...new Set(
          inverted
            .filter(([term]) => compareBytes(encoder.encode(term).subarray(0, p.length), p) === 0)
            .flatMap(([, docs]) => docs.map(([d]) => d)),
        ),
      ].sort((a, b) => a - b);
      assert.deepEqual((await index.complete(prefix, 2)).docs, want.slice(0, 2), `${round} ${prefix}`);
    }
  }
});
