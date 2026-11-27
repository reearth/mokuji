// What the four libraries share beyond the conformance cases: phrases and
// ranking without payload, a collection's search, manifests, the metadata a
// reader exposes, the errors it throws, and how a file is opened. The other
// languages' tests ask the same questions with the same answers.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  BuildError,
  MokujiBuilder,
  MokujiCollection,
  MokujiError,
  MokujiIndex,
  NoColumnError,
  SECTION_META,
  SECTION_PAYLOAD,
  SECTION_POSITIONS,
  SECTION_TERM_BLOCKS,
  decodeCbor,
  locate,
  lookupPrefix,
  encodeCbor,
  encodeMetadata,
  manifestToJson,
  parseManifest,
  within,
} from "../dist/index.js";

const FIXTURES = fileURLToPath(new URL("../../conformance/fixtures/", import.meta.url));
const bytesSource = (bytes) => ({ read: (offset, length) => bytes.subarray(offset, offset + length) });
const fixtureBytes = async (name) => new Uint8Array(await readFile(`${FIXTURES}${name}.mokuji`));
const fixture = async (name, options) => MokujiIndex.open(bytesSource(await fixtureBytes(name)), options);
const docs = (found) => found.results.map((r) => r.doc);

// --- phrases -------------------------------------------------------------------
//
// `tiny`: 0 "tokyo chiyoda", 1 "tokyo chuo", 2 "tokyo shibuya", 3 "osaka chuo".

test("phrase: the terms at consecutive positions, in the order typed", async () => {
  const tiny = await fixture("tiny");
  assert.deepEqual(docs(await tiny.search("tokyo chuo", { phrase: true })), [1]);
  assert.deepEqual(docs(await tiny.search("chuo tokyo", { phrase: true })), []);
  assert.deepEqual(docs(await tiny.search("osaka chuo", { phrase: true })), [3]);
  // A union is a phrase only where every term is.
  assert.deepEqual(docs(await tiny.search("tokyo chuo", { phrase: true, any: true })), [1]);
});

test("phrase: a probe tests the terms it read, at the places they were typed", async () => {
  const tiny = await fixture("tiny");
  // `chuo` is the rarer, so it alone is read, and alone it is a phrase
  // wherever it occurs: ranked as the plain search with the same probe.
  const probed = await tiny.search("tokyo chuo", { phrase: true, probe: 1 });
  assert.deepEqual(probed.probed.map((p) => p.term), ["chuo"]);
  assert.deepEqual(docs(probed), docs(await tiny.search("tokyo chuo", { probe: 1 })));
  assert.deepEqual(docs(probed), [1, 3]);
});

test("phrase: tested after the filter, and not read when nothing is left", async () => {
  const tiny = await fixture("tiny");
  tiny.resetStats();
  await tiny.search("tokyo chuo", { filter: (doc) => doc !== 1 });
  const plain = tiny.stats.requests;
  tiny.resetStats();
  const found = await tiny.search("tokyo chuo", { phrase: true, filter: (doc) => doc !== 1 });
  assert.deepEqual(docs(found), []);
  assert.equal(tiny.stats.requests, plain, "no positions read");
  // And one read per term when something is.
  tiny.resetStats();
  await tiny.search("tokyo chuo");
  const unphrased = tiny.stats.requests;
  tiny.resetStats();
  await tiny.search("tokyo chuo", { phrase: true });
  assert.equal(tiny.stats.requests, unphrased + 2);
});

test("phrase: a file without positions is refused", async () => {
  const dict = await fixture("dict");
  await assert.rejects(dict.search("東京都", { phrase: true }), (error) => {
    assert.ok(error instanceof MokujiError);
    assert.match(error.message, /this index carries no positions, so it cannot test a phrase/);
    return true;
  });
});

// --- ranking without payload ----------------------------------------------------

test("rank: the documents and scores search gives, and no payload read", async () => {
  const tiny = await fixture("tiny");
  const searched = await tiny.search("tokyo chuo");
  const payload = tiny.sections.get(SECTION_PAYLOAD);
  const inPayload = [];
  tiny.onRequest = ({ offset }) => {
    if (offset >= payload.offset && offset < payload.offset + payload.length) inPayload.push(offset);
  };
  const ranked = await tiny.rank(["tokyo", "chuo"]);
  assert.deepEqual(
    ranked.ranked,
    searched.results.map(({ doc, score }) => ({ doc, score })),
  );
  assert.deepEqual(ranked.probed, searched.probed);
  assert.equal(ranked.skipped, 0);
  assert.deepEqual(inPayload, [], "rank reads no payload");
  // Where search does read it.
  await tiny.search("tokyo chuo");
  assert.ok(inPayload.length > 0);
});

// --- a collection's search ------------------------------------------------------

/** A segment of documents split at spaces, one per text. */
async function segment(texts) {
  const builder = new MokujiBuilder({ termsPerBlock: 4, docsPerPayloadBlock: 2 });
  const postings = new Map();
  texts.forEach((text, doc) => {
    builder.addDocument(text);
    for (const term of new Set(text.split(" "))) postings.set(term, [...(postings.get(term) ?? []), doc]);
  });
  for (const term of [...postings.keys()].sort()) for (const doc of postings.get(term)) builder.addPosting(term, doc);
  return builder.finish(
    encodeMetadata({ sources: [{ contentHash: "sha2-256:" + "0".repeat(64) }], tokenizerId: "space-split" }),
  );
}

/** Two segments; the first's document 0 deleted. */
async function twoSegments() {
  const files = new Map([
    ["base.mokuji", await segment(["tokyo chiyoda", "tokyo chuo", "osaka chuo"])],
    ["delta.mokuji", await segment(["tokyo chuo", "kyoto"])],
  ]);
  const manifest = {
    mokuji_manifest: "0.1",
    generation: 2,
    segments: [
      { path: "base.mokuji", content_hash: "sha2-256:" + "a".repeat(64), length: files.get("base.mokuji").length, deletes: [0] },
      { path: "delta.mokuji", content_hash: "sha2-256:" + "b".repeat(64), length: files.get("delta.mokuji").length },
    ],
  };
  return MokujiCollection.open(manifest, (path) => bytesSource(files.get(path)));
}

test("collection search: every live document holding the terms, in collection order", async () => {
  const collection = await twoSegments();
  const chuo = await collection.search("chuo");
  assert.deepEqual(chuo.terms, ["chuo"]);
  assert.equal(chuo.matched, 3);
  assert.deepEqual(chuo.hits, [
    { segment: 0, doc: 1 },
    { segment: 0, doc: 2 },
    { segment: 1, doc: 0 },
  ]);
  assert.deepEqual(chuo.payloads, ["tokyo chuo", "osaka chuo", "tokyo chuo"]);

  // The delete takes `tokyo chiyoda`; the limit counts hits, not segments.
  const tokyo = await collection.search("tokyo", 1);
  assert.equal(tokyo.matched, 2);
  assert.deepEqual(tokyo.hits, [{ segment: 0, doc: 1 }]);
  assert.deepEqual(tokyo.payloads, ["tokyo chuo"]);

  assert.deepEqual((await collection.search("tokyo chuo")).hits, [
    { segment: 0, doc: 1 },
    { segment: 1, doc: 0 },
  ]);
  assert.equal((await collection.search("kyoto chuo")).matched, 0);
  assert.equal((await collection.search("")).matched, 0);
});

test("collection search: the conformance collection, its delete left out", async () => {
  const manifest = JSON.parse(await readFile(`${FIXTURES}collection.json`, "utf8"));
  const files = new Map();
  for (const s of manifest.segments) files.set(s.path, await fixtureBytes(s.path.replace(/\.mokuji$/, "")));
  const collection = await MokujiCollection.open(manifest, (path) => bytesSource(files.get(path)));
  const hits = async (query) => (await collection.search(query)).hits.map((h) => [h.segment, h.doc]);
  assert.deepEqual(await hits("Tokyo Tower"), [[1, 1]]);
  assert.deepEqual(await hits("Black Sea"), []);
  assert.deepEqual(await hits("Black Mountain"), [[1, 0]]);
  assert.deepEqual(await hits("Schwarzwald"), [[0, 1]]);
  assert.deepEqual((await collection.search("Schwarzwald")).payloads, ["Black Forest"]);
});

test("a collection refuses a segment it does not have, and with none a query is one term", async () => {
  const collection = await twoSegments();
  await assert.rejects(collection.liveIn(5, "tokyo", 1), /the collection has no segment 5/);
  await assert.rejects(collection.payloads([{ segment: 9, doc: 0 }]), (e) => e instanceof MokujiError);
  const empty = new MokujiCollection({ mokuji_manifest: "0.1", generation: 1, segments: [] }, []);
  const found = await empty.search("tokyo chuo");
  assert.deepEqual([found.terms, found.matched, found.hits], [["tokyo chuo"], 0, []]);
});

test("collection search: a missing term ends a segment without reading the rest", async () => {
  const collection = await twoSegments();
  for (const index of collection.segments) index.coalesce = false;
  collection.resetStats();
  // `nowhere` is in no segment: one dictionary block each, and nothing else.
  await collection.search("nowhere chuo");
  assert.equal(collection.stats.requests, 2);
});

// --- manifests ------------------------------------------------------------------

test("manifest: written down as the Rust writer writes it", async () => {
  const text = await readFile(`${FIXTURES}collection.json`, "utf8");
  assert.equal(manifestToJson(parseManifest(text)), text);
  // Unknown keys are read past, and not written back.
  const extra = text.replace("{", '{\n  "published": "2026-09-30",');
  assert.equal(manifestToJson(parseManifest(extra)), text);
});

test("manifest: within keeps what has no region and what includes the place", async () => {
  const manifest = parseManifest(await readFile(`${FIXTURES}collection.json`, "utf8"));
  assert.equal(within(manifest, ["JP"]).segments.length, 2);
  const elsewhere = within(manifest, ["FR"]);
  assert.deepEqual(elsewhere.segments.map((s) => s.path), ["names.mokuji"]);
  assert.deepEqual(elsewhere.segments[0].deletes, [0]);
});

test("manifest: refuses what it cannot trust", async () => {
  const good = JSON.parse(await readFile(`${FIXTURES}collection.json`, "utf8"));
  const bad = (change) => {
    const copy = structuredClone(good);
    change(copy);
    return JSON.stringify(copy);
  };
  const refusals = [
    bad((m) => (m.segments[0].deletes = [7, 2])),
    bad((m) => (m.segments[0].deletes = [2, 2])),
    bad((m) => (m.segments[1].content_hash = "abcd")),
    bad((m) => (m.mokuji_manifest = "1.0")),
    bad((m) => (m.segments = [])),
    bad((m) => (m.segments[0].path = "")),
    ...[[], ["jp"], ["JPN"], ["JP", "JP"]].map((countries) => bad((m) => (m.segments[1].region = { countries }))),
    "not json",
  ];
  for (const text of refusals) assert.throws(() => parseManifest(text), MokujiError, text);
});

test("manifest: a collection is opened only from one that parses", async () => {
  const collection = parseManifest(await readFile(`${FIXTURES}collection.json`, "utf8"));
  await assert.rejects(
    MokujiCollection.open({ ...collection, mokuji_manifest: "1.0" }, () => bytesSource(new Uint8Array())),
    MokujiError,
  );
});

test("manifest: a URL that answers with something other than JSON is refused", async () => {
  const fakeFetch = async () => new Response("<html>not a manifest</html>", { status: 200 });
  await assert.rejects(
    MokujiCollection.open("https://example.invalid/world.json", undefined, { fetch: fakeFetch }),
    (e) => e instanceof MokujiError && /^manifest:/.test(e.message),
  );
});

// --- metadata, CBOR and errors --------------------------------------------------

test("exposes the generation, the attrs and the tokenizer's parameters", async () => {
  const tiny = await fixture("tiny");
  assert.equal(tiny.generation, 1);
  assert.equal(tiny.attrs, null);
  assert.deepEqual(tiny.tokenizerParams, {});

  const builder = new MokujiBuilder();
  builder.addDocument("x");
  builder.addPosting("x", 0);
  const built = await MokujiIndex.open(
    bytesSource(
      await builder.finish(
        encodeMetadata({
          generation: 7,
          sources: [{ contentHash: "sha2-256:" + "0".repeat(64) }],
          tokenizerId: "identity",
          tokenizerParams: { fold: "nfkc" },
          attrs: { "geohash-precisions": "5,6" },
        }),
      ),
    ),
  );
  assert.equal(built.generation, 7);
  assert.deepEqual(built.attrs, { "geohash-precisions": "5,6" });
  assert.deepEqual(built.tokenizerParams, { fold: "nfkc" });
});

test("decodeCbor reads one item and refuses what follows it", () => {
  const bytes = encodeCbor({ a: [1, "two"], b: "c" });
  assert.deepEqual(decodeCbor(bytes), { a: [1, "two"], b: "c" });
  const longer = new Uint8Array([...bytes, 0]);
  assert.throws(() => decodeCbor(longer), MokujiError);
  assert.throws(() => decodeCbor(new Uint8Array()), MokujiError);
});

test("decodeCbor refuses what is cut short, and what claims more than it holds", () => {
  // A head whose argument is missing; an array of two holding one; an
  // array claiming four billion items in five bytes, refused at once.
  for (const bytes of [Uint8Array.of(0x19, 0x01), Uint8Array.of(0x82, 0x01), Uint8Array.of(0x9a, 0xff, 0xff, 0xff, 0xff)]) {
    assert.throws(() => decodeCbor(bytes), (e) => e instanceof MokujiError && /truncated/.test(e.message));
  }
  assert.throws(() => decodeCbor(Uint8Array.of(0x5a, 0xff, 0xff, 0xff, 0xff)), MokujiError);
  assert.throws(() => decodeCbor(Uint8Array.of(0xbb, 0, 0, 0, 1, 0, 0, 0, 0)), MokujiError);
});

/** A file of one document whose metadata section is `meta`, as given. */
async function withMeta(meta) {
  const builder = new MokujiBuilder();
  builder.addDocument("x");
  builder.addPosting("x", 0);
  return builder.finish(meta);
}

test("metadata followed by bytes it does not account for is refused", async () => {
  const meta = encodeMetadata({ sources: [{ contentHash: "sha2-256:" + "0".repeat(64) }], tokenizerId: "identity" });
  await MokujiIndex.open(bytesSource(await withMeta(meta)));
  const longer = new Uint8Array([...meta, 0]);
  await assert.rejects(MokujiIndex.open(bytesSource(await withMeta(longer))), MokujiError);
});

test("a generation that is not a non-negative integer reads as 0", async () => {
  const meta = encodeCbor({ generation: "seven", tokenizer: { id: "identity", params: {} } });
  assert.equal((await MokujiIndex.open(bytesSource(await withMeta(meta)))).generation, 0);
});

test("phrase: positions that do not number the postings' documents are refused", async () => {
  const tiny = await fixture("tiny");
  tiny.positions = async () => [];
  await assert.rejects(tiny.search("tokyo chuo", { phrase: true }), /positions disagree with the postings/);
});

test("phrase: a term asked twice reads its positions once", async () => {
  const tiny = await fixture("tiny");
  let reads = 0;
  const positions = tiny.positions.bind(tiny);
  tiny.positions = (...args) => ((reads += 1), positions(...args));
  await tiny.search("tokyo tokyo", { phrase: true });
  assert.equal(reads, 1);
});

test("a boost the file lacks is refused before a phrase it cannot test", async () => {
  const dict = await fixture("dict");
  await assert.rejects(dict.search("東京都", { phrase: true, boost: "absent" }), NoColumnError);
});

test("lookupPrefix with a limit of 0 reads nothing", async () => {
  const dict = await fixture("dict");
  dict.resetStats();
  assert.deepEqual(await lookupPrefix(dict, "東京都", 0, ["東京都千代田区"]), []);
  assert.equal(dict.stats.requests, 0);
});

test("locate: what the caller says counts only when it says something", async () => {
  const dict = await fixture("dict");
  // Nothing given that counts, and the file says nothing: refused.
  for (const options of [{ precisions: [], scale: 1e5 }, { precisions: [5], scale: 0 }, { precisions: [5], scale: NaN }]) {
    await assert.rejects(locate(dict, 35.68, 139.76, options), /geohash precisions/);
  }
  for (const precisions of [[0], [13], [5.5]]) {
    await assert.rejects(locate(dict, 35.68, 139.76, { precisions, scale: 1e5 }), MokujiError);
  }
  assert.deepEqual((await locate(dict, 35.68, 139.76, { precisions: [5], scale: 1e5 })).inside, []);
});

test("refusals are the library's own errors", async () => {
  const tiny = await fixture("tiny");
  await assert.rejects(tiny.search("tokyo", { boost: "absent" }), (error) => {
    assert.ok(error instanceof NoColumnError && error instanceof MokujiError);
    assert.equal(error.column, "absent");
    return true;
  });
  await assert.rejects(MokujiIndex.open(bytesSource(new Uint8Array(100))), MokujiError);
  assert.throws(() => new MokujiBuilder({ termsPerBlock: 0 }), BuildError);
});

test("section identifiers are the specification's", () => {
  assert.equal(SECTION_META, 0x0001);
  assert.equal(SECTION_TERM_BLOCKS, 0x0011);
  assert.equal(SECTION_POSITIONS, 0x0060);
});

// --- opening --------------------------------------------------------------------

test("a resident dictionary costs one read to open and none to look a term up", async () => {
  const fetched = await fixture("dict");
  const held = await fixture("dict", { residentDictionary: true });
  assert.equal(held.stats.requests, fetched.stats.requests + 1);
  for (const index of [fetched, held]) index.resetStats();
  const term = "東京都千代田区丸の内1丁目1番1号";
  assert.deepEqual(await held.exact(term), await fetched.exact(term));
  assert.deepEqual(await held.prefix("東京都", 100), await fetched.prefix("東京都", 100));
  assert.equal(held.stats.requests, 0);
  assert.ok(fetched.stats.requests > 0);
});

test("a URL is fetched with the fetch it is given", async () => {
  const bytes = await fixtureBytes("tiny");
  const asked = [];
  const fakeFetch = async (url, init) => {
    asked.push(url);
    const [, first, last] = init.headers.Range.match(/bytes=(\d+)-(\d+)/);
    const end = Math.min(Number(last) + 1, bytes.length);
    return new Response(bytes.slice(Number(first), end), {
      status: 206,
      headers: { "Content-Range": `bytes ${first}-${end - 1}/${bytes.length}` },
    });
  };
  const index = await MokujiIndex.open("https://example.invalid/tiny.mokuji", { fetch: fakeFetch });
  assert.deepEqual(docs(await index.search("tokyo chuo")), [1]);
  assert.ok(asked.length > 0 && asked.every((url) => url === "https://example.invalid/tiny.mokuji"));
});
