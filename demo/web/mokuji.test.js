// The browser reader, against the conformance suite.
//
// `mokuji.js` is a third implementation of SPEC.md and the only one written for
// the transport the format was designed for, so it is the one most worth
// holding to the same cases as the others. Run with `node --test demo/web`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { MokujiCollection, MokujiIndex, jaAddress, nameList } from "./mokuji.js";

const CONFORMANCE = fileURLToPath(new URL("../../conformance/", import.meta.url));

/** A byte source over a file, so the reader can be run without a server. */
async function fileSource(name) {
  const bytes = new Uint8Array(await readFile(`${CONFORMANCE}fixtures/${name}.mokuji`));
  return {
    read: (offset, length) => bytes.subarray(offset, offset + length),
  };
}

/**
 * The subset of TOML the conformance cases use.
 *
 * Reading them rather than restating them: a copy of the expectations would
 * drift, and the whole point is that every implementation answers to the same
 * ones.
 */
function parseTables(text) {
  const tables = {};
  let current = null;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/(^|\s)#.*$/, "").trim();
    if (!line) continue;
    const table = line.match(/^\[\[(\w+)\]\]$/);
    if (table) {
      current = {};
      (tables[table[1]] ??= []).push(current);
      continue;
    }
    const [, key, value] = line.match(/^(\w+)\s*=\s*(.*)$/) ?? [];
    if (!key) continue;
    current[key] = value.startsWith("[")
      ? JSON.parse(value.replace(/'/g, '"'))
      : value.startsWith('"')
        ? JSON.parse(value)
        : value === "true" || value === "false"
          ? value === "true"
          : Number(value);
  }
  return tables;
}

const tables = parseTables(await readFile(`${CONFORMANCE}cases.toml`, "utf8"));
const cases = tables.case;

test("the conformance suite is not empty", () => {
  assert.ok(cases.length > 0);
});

for (const testCase of cases) {
  test(`${testCase.name} (${testCase.fixture})`, async () => {
    const index = await MokujiIndex.open(await fileSource(testCase.fixture));
    index.resetStats();

    // The cases give one query per term; a single-term case is a prefix, and a
    // multi-term one is an intersection, which this reader does not do — so
    // only the single-term cases apply.
    if (testCase.query.length !== 1) return;

    const { results } = await index.lookup(testCase.query[0], 100);
    const exact = results.filter((r) => r.term === index.normalize(testCase.query[0]));

    assert.deepEqual(exact.map((r) => r.doc), testCase.docs);
    assert.deepEqual(exact.map((r) => r.payload), testCase.payloads);

    // Resident, so asking for it is arithmetic rather than a request.
    if (testCase.attribute) {
      const column = index.column(testCase.attribute);
      assert.ok(column, `no column '${testCase.attribute}'`);
      assert.deepEqual(
        testCase.docs.map((doc) => column.values[doc]),
        testCase.values,
      );
    }

    // The one section a reader pays for only when it asks, and asking costs
    // exactly one read.
    if (testCase.positions) {
      const entry = await index.exact(index.normalize(testCase.query[0]));
      assert.ok(entry, "term is absent");
      const [postings] = await index.postingsSpan([entry]);
      const before = index.stats.requests;
      const places = await index.positions(entry, postings);
      assert.deepEqual(places, testCase.positions);
      assert.equal(index.stats.requests - before, 1, "positions must cost one read");
    }
  });
}

for (const testCase of tables.completion ?? []) {
  test(`completion: ${testCase.name} (${testCase.fixture})`, async () => {
    const index = await MokujiIndex.open(await fileSource(testCase.fixture));
    index.resetStats();
    const { docs, payloads, fromTable } = await index.complete(testCase.prefix, testCase.limit);
    assert.deepEqual(docs, testCase.docs);
    assert.deepEqual(payloads, testCase.payloads);
    assert.equal(fromTable, testCase.from_table);
    assert.equal(index.stats.requests, testCase.reads);
  });
}

for (const testCase of tables.collection ?? []) {
  test(`collection: ${testCase.name}`, async () => {
    const manifest = JSON.parse(
      await readFile(`${CONFORMANCE}fixtures/${testCase.manifest}.json`, "utf8"),
    );
    const sources = new Map();
    for (const s of manifest.segments) sources.set(s.path, await fileSource(s.path.replace(/\.mokuji$/, "")));
    const collection = await MokujiCollection.open(manifest, (path) => sources.get(path));
    collection.resetStats();
    const { hits, payloads } = await collection.complete(testCase.prefix, testCase.limit);
    assert.deepEqual(hits.map((h) => h.segment), testCase.segments);
    assert.deepEqual(hits.map((h) => h.doc), testCase.docs);
    assert.deepEqual(payloads, testCase.payloads);
    assert.equal(collection.stats.requests, testCase.reads);
  });
}

test("the completion cases were read", () => {
  assert.ok((tables.completion ?? []).length > 0);
});

test("name-list agrees with the other implementations", () => {
  assert.equal(nameList("  Black\u00a0 Sea "), "black sea");
  assert.equal(nameList("ＴＯＫＹＯ"), "tokyo");
  assert.equal(nameList("İstanbul"), "i\u0307stanbul");
  assert.equal(nameList("ΟΔΟΣ"), "οδος");
});

test("carries the license and the credit a file states", async () => {
  const names = await MokujiIndex.open(await fileSource("names"));
  assert.equal(names.license, "CC0-1.0");
  assert.equal(names.attribution, "Mokuji conformance fixtures");
  const tiny = await MokujiIndex.open(await fileSource("tiny"));
  assert.equal(tiny.license, null);
  assert.equal(tiny.attribution, null);
});

test("reads a deflated file", async () => {
  // The fixtures the other implementations reproduce byte for byte are stored
  // raw, so without this the browser's decompression is never exercised.
  const index = await MokujiIndex.open(await fileSource("dict-deflate"));
  const { results } = await index.lookup("東京都千代田区丸の内1丁目1番1号", 10);

  assert.equal(results.length, 1);
  assert.equal(results[0].payload, "139.767201,35.681560");
});

test("a prefix range costs the reads the specification says", async () => {
  const index = await MokujiIndex.open(await fileSource("dict"));
  index.resetStats();

  const { results } = await index.lookup("東京都千代田区丸の内", 100);
  assert.equal(results.length, 3);
  // Two term blocks of two terms hold the three matches, and their documents
  // span two payload blocks. No postings: they are implied.
  assert.equal(index.stats.requests, 4);
});

test("range returns the terms between two keys, both inclusive", async () => {
  const index = await MokujiIndex.open(await fileSource("dict"));
  const terms = async (low, high, limit) =>
    (await index.range(low, high, limit)).map((e) => new TextDecoder().decode(e.term));

  assert.deepEqual(await terms("東京都千代田区丸の内1丁目1番2号", "東京都港区赤坂1丁目1番1号"), [
    "東京都千代田区丸の内1丁目1番2号",
    "東京都千代田区丸の内2丁目1番1号",
    "東京都港区赤坂1丁目1番1号",
  ]);
  // Neither end has to be a term.
  assert.deepEqual(await terms("東京都千代田区丸の内1丁目1番1号a", "東京都千代田区丸の内3"), [
    "東京都千代田区丸の内1丁目1番2号",
    "東京都千代田区丸の内2丁目1番1号",
  ]);
  assert.deepEqual(await terms("東京都", "東京都千代田区丸の内9", 1), ["東京都千代田区丸の内1丁目1番1号"]);
  assert.deepEqual(await terms("北海道", "千葉県"), [], "an empty range reads nothing past its first block");
});

test("holds the tokenizer the index declares", async () => {
  const index = await MokujiIndex.open(await fileSource("dict"));
  assert.equal(index.tokenizerId, "identity");
});

test("ja-address agrees with the other implementations", () => {
  // The same vectors the Rust tests use. A tokenizer is outside the format, but
  // an index is unreachable if two implementations disagree about it.
  const vectors = [
    ["東京都千代田区丸の内一丁目", "東京都千代田区丸の内1丁目"],
    ["東京都千代田区丸の内１丁目", "東京都千代田区丸の内1丁目"],
    ["札幌市北区北十一丁目", "札幌市北区北11丁目"],
    ["二十丁目", "20丁目"],
    ["百丁目", "100丁目"],
    ["千代田区一番町", "千代田区一番町"],
    ["京都市下京区五条", "京都市下京区五条"],
    ["一二丁目", "一二丁目"],
    ["  東京都　", "東京都"],
    ["ﾏﾙﾉｳﾁ", "マルノウチ"],
  ];
  for (const [input, expected] of vectors) {
    assert.equal(jaAddress(input), expected, input);
  }
});

test("exact finds a term without completing it", async () => {
  const index = await MokujiIndex.open(await fileSource("tiny"));

  // `tiny` is a document collection, so a term is a word.
  const entry = await index.exact("tokyo");
  assert.ok(entry, "expected 'tokyo' to be in the dictionary");
  assert.equal(new TextDecoder().decode(entry.term), "tokyo");

  assert.equal(await index.exact("toky"), null, "a proper prefix is not a term");
  assert.equal(await index.exact("tokyoo"), null, "an extension is not a term");
  assert.equal(await index.exact(""), null, "the empty string is not a term");
});

test("exact costs one dictionary read", async () => {
  const bytes = new Uint8Array(
    await readFile(`${CONFORMANCE}fixtures/tiny.mokuji`),
  );
  let reads = 0;
  const index = await MokujiIndex.open({
    read: (offset, length) => {
      reads += 1;
      return bytes.subarray(offset, offset + length);
    },
  });

  // The block a term would be in is answerable from the resident index, so a
  // term that is absent costs exactly what a term that is present does.
  const before = reads;
  await index.exact("tokyo");
  assert.equal(reads - before, 1);
  await index.exact("nowhere");
  assert.equal(reads - before, 2);
});

test("ja-address folds the spellings of a place name onto one", () => {
  // The registry writes both, so a reader that folded only one of them would
  // find the address it was not given.
  assert.equal(jaAddress("東京都千代田区丸ノ内一丁目"), jaAddress("東京都千代田区丸の内1丁目"));
  assert.equal(jaAddress("神奈川県茅ケ崎市"), jaAddress("神奈川県茅ヶ崎市"));
  assert.equal(jaAddress("三重県いなべ市北勢町二之瀬"), jaAddress("三重県いなべ市北勢町二の瀬"));

  // Left alone on purpose: `が` is an ordinary particle and folding it bought
  // nothing measurable, and `ハイツ` is a word.
  assert.notEqual(jaAddress("三重県亀山市みずきが丘"), jaAddress("三重県亀山市みずきヶ丘"));
  assert.equal(jaAddress("三重県亀山市関町富士ハイツ"), "三重県亀山市関町富士ハイツ");

  // Inside a katakana word these are letters, not connectives.
  assert.equal(jaAddress("マルノウチ"), "マルノウチ");
  assert.equal(jaAddress("ケーキ"), "ケーキ");
});
