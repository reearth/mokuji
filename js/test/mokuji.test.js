// The JavaScript reader, against the conformance suite.
//
// This is a third implementation of SPEC.md and the only one written for the
// transport the format was designed for, so it is the one most worth holding to
// the same cases as the others. Run with `npm test` in js/; it tests the built
// dist/, which is what everyone else imports.

import assert from "node:assert/strict";
import { openAsBlob } from "node:fs";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { MokujiCollection, MokujiError, MokujiIndex, bigram, jaAddress, nameList } from "../dist/index.js";

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
    // The cases count block by block; coalescing is checked against them below.
    index.coalesce = false;
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
    index.coalesce = false;
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
    const collection = await MokujiCollection.open(manifest, (path) => sources.get(path), {
      countries: testCase.countries ?? null,
    });
    for (const segment of collection.segments) segment.coalesce = false;
    collection.resetStats();
    const { hits, payloads } = await collection.complete(testCase.prefix, testCase.limit);
    assert.deepEqual(hits.map((h) => h.segment), testCase.segments);
    assert.deepEqual(hits.map((h) => h.doc), testCase.docs);
    assert.deepEqual(payloads, testCase.payloads);
    assert.equal(collection.stats.requests, testCase.reads);
  });
}

test("reading neighbouring blocks together never costs more than the cases say", async () => {
  let fewer = 0;
  for (const testCase of cases.filter((c) => c.query.length === 1)) {
    const index = await MokujiIndex.open(await fileSource(testCase.fixture));
    index.resetStats();
    const { results } = await index.lookup(testCase.query[0], 100);
    const exact = results.filter((r) => r.term === index.normalize(testCase.query[0]));
    assert.deepEqual(exact.map((r) => r.doc), testCase.docs, testCase.name);
    assert.deepEqual(exact.map((r) => r.payload), testCase.payloads, testCase.name);
    assert.ok(index.stats.requests <= testCase.reads, testCase.name);
    if (index.stats.requests < testCase.reads) fewer += 1;
  }
  for (const testCase of tables.completion ?? []) {
    const index = await MokujiIndex.open(await fileSource(testCase.fixture));
    index.resetStats();
    const { docs, payloads } = await index.complete(testCase.prefix, testCase.limit);
    assert.deepEqual(docs, testCase.docs, testCase.name);
    assert.deepEqual(payloads, testCase.payloads, testCase.name);
    assert.ok(index.stats.requests <= testCase.reads, testCase.name);
    if (index.stats.requests < testCase.reads) fewer += 1;
  }
  // Payload blocks next to one another are common in the fixtures, so some
  // case has to come out cheaper, or coalescing is not happening at all.
  assert.ok(fewer > 0);
});

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

test("names what a file was derived from", async () => {
  const tiny = await MokujiIndex.open(await fileSource("tiny"));
  assert.equal(tiny.sources.length, 1);
  const [source] = tiny.sources;
  assert.match(source.contentHash, /^sha2-256:[0-9a-f]{64}$/);
  assert.equal(source.uri, "conformance/fixtures/tiny.corpus.txt");
  // Neither date is required, and a file that states none says nothing.
  assert.equal(source.retrieved, null);
  assert.equal(source.modified, null);
});

test("names what wrote a file, and how", async () => {
  const { generator } = await MokujiIndex.open(await fileSource("tiny"));
  assert.equal(generator.name, "build_fixtures.py");
  assert.equal(generator.version, "0.1");
  assert.equal(generator.params.terms_per_block, 2);
  assert.equal(generator.params.payload, "opaque");
});

test("reads a deflated file", async () => {
  // The fixtures the other implementations reproduce byte for byte are stored
  // raw, so without this the browser's decompression is never exercised.
  const index = await MokujiIndex.open(await fileSource("dict-deflate"));
  const { results } = await index.lookup("東京都千代田区丸の内1丁目1番1号", 10);

  assert.equal(results.length, 1);
  assert.equal(results[0].payload, "139.767201,35.681560");
});

test("reads a Blob as it reads any other source", async () => {
  // A Blob in memory, and one backed by the file on disk: both are read a
  // slice at a time, and answer and cost what a Source over the same bytes
  // does.
  const path = `${CONFORMANCE}fixtures/dict.mokuji`;
  const expected = await MokujiIndex.open(await fileSource("dict"));
  expected.resetStats();
  const want = await expected.lookup("東京都千代田区丸の内", 100);

  for (const blob of [new Blob([await readFile(path)]), await openAsBlob(path)]) {
    const index = await MokujiIndex.open(blob);
    assert.equal(index.fileLength, blob.size);
    index.resetStats();
    assert.deepEqual(await index.lookup("東京都千代田区丸の内", 100), want);
    assert.deepEqual(index.stats, expected.stats);
  }
});

test("decodes every CBOR type, since unknown metadata keys may hold any", async () => {
  const { decodeCbor } = await import("../dist/mokuji.js");
  // {"a": [-1, h'0102', true, false, null, 1.5 (half), 0.25 (single), 1.1 (double), 1(5)]}
  const bytes = new Uint8Array([
    0xa1, 0x61, 0x61, 0x89, 0x20, 0x42, 0x01, 0x02, 0xf5, 0xf4, 0xf6, 0xf9, 0x3e, 0x00,
    0xfa, 0x3e, 0x80, 0x00, 0x00, 0xfb, 0x3f, 0xf1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9a, 0xc1, 0x05,
  ]);
  assert.deepEqual(decodeCbor(bytes), {
    a: [-1, new Uint8Array([1, 2]), true, false, null, 1.5, 0.25, 1.1, 5],
  });
});

test("ja-address strips White_Space, not what String.trim strips", () => {
  // U+0085 is White_Space and String.trim keeps it; U+FEFF is not, and
  // String.trim takes it. TOKENIZERS.md names the property, as Rust and
  // Python read it.
  assert.equal(jaAddress("\u0085東京都\u0085"), "東京都");
  assert.equal(jaAddress("\ufeff東京都"), "\ufeff東京都");
});

test("a probe of zero reads no postings and matches nothing", async () => {
  const tiny = await MokujiIndex.open(await fileSource("tiny"));
  // An intersection is where it failed, indexing the first of no runs; a
  // union of none was already empty.
  for (const any of [false, true]) {
    const found = await tiny.search("tokyo chuo", { any, probe: 0 });
    assert.equal(found.matched, 0);
    assert.equal(found.skipped, 2);
  }
});

test("a prefix range costs the reads the specification says", async () => {
  const index = await MokujiIndex.open(await fileSource("dict"));
  index.coalesce = false;
  index.resetStats();

  const { results } = await index.lookup("東京都千代田区丸の内", 100);
  assert.equal(results.length, 3);
  // Two term blocks of two terms hold the three matches, and their documents
  // span two payload blocks. No postings: they are implied.
  assert.equal(index.stats.requests, 4);

  // The two payload blocks are neighbours, so a reader that coalesces reads
  // them as one.
  index.coalesce = true;
  index.resetStats();
  await index.lookup("東京都千代田区丸の内", 100);
  assert.equal(index.stats.requests, 3);
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

test("bigram splits as TOKENIZERS.md says", () => {
  assert.deepEqual(bigram("建築物"), ["建築", "築物"]);
  assert.deepEqual(bigram("CityGML"), ["citygml"]);
  assert.deepEqual(bigram("3D都市"), ["3d", "都市"]);
  assert.deepEqual(bigram("都"), ["都"]);
  // Split where ASCII meets CJK, after composition, and only there.
  assert.deepEqual(bigram("LOD2の建物"), ["lod2", "の建", "建物"]);
  assert.deepEqual(bigram("ＬＯＤ２の"), ["lod2", "の"]);
  assert.deepEqual(bigram("第3章"), ["第", "3", "章"]);
  assert.deepEqual(bigram("Aー"), ["a", "ー"]);
  assert.deepEqual(bigram("Zürich"), ["zü", "ür", "ri", "ic", "ch"]);
});

test("search ranks as mokuji search does", async () => {
  const tiny = await MokujiIndex.open(await fileSource("tiny"));
  // Scores to three places, as `mokuji search` prints them for the same file.
  const rounded = (r) => r.results.map((x) => [x.doc, x.score.toFixed(3), x.payload]);

  const all = await tiny.search("tokyo chuo");
  assert.deepEqual(rounded(all), [[1, "1.050", "35.671,139.772"]]);

  const any = await tiny.search("tokyo chuo", { any: true });
  assert.deepEqual(rounded(any), [
    [1, "1.050", "35.671,139.772"],
    [3, "0.693", "34.681,135.513"],
    [0, "0.357", "35.694,139.754"],
    [2, "0.357", "35.664,139.698"],
  ]);

  const probed = await tiny.search("tokyo chuo", { any: true, probe: 1 });
  assert.deepEqual(probed.probed.map((p) => p.term), ["chuo"]);
  assert.equal(probed.skipped, 1);

  assert.equal((await tiny.search("tokyo nowhere")).matched, 0);
  await assert.rejects(tiny.search("tokyo", { boost: "absent" }));
});

test("a filter leaves out documents before ranking", async () => {
  const tiny = await MokujiIndex.open(await fileSource("tiny"));
  const filter = (doc) => doc % 2 === 0;
  // Ranked as without the filter, the odd ones gone.
  const found = await tiny.search("tokyo chuo", { any: true, filter, limit: 1 });
  assert.equal(found.matched, 2);
  assert.deepEqual(found.results.map((x) => [x.doc, x.score.toFixed(3)]), [[0, "0.357"]]);
  assert.equal((await tiny.search("tokyo chuo", { filter })).matched, 0);
});

/**
 * Opens a malformed fixture and asks the case's question. Columns are decoded
 * when the file is opened, so an attribute case asks nothing more.
 */
async function askMalformed(testCase) {
  const index = await MokujiIndex.open(await fileSource(testCase.fixture));
  if (testCase.query !== undefined) {
    const entry = await index.exact(index.normalize(testCase.query));
    if (!entry) return;
    const [postings] = await index.postingsSpan([entry]);
    await index.payloadSpan(postings.docs);
  } else if (testCase.prefix !== undefined) {
    await index.complete(testCase.prefix, testCase.limit);
  }
}

for (const testCase of tables.malformed ?? []) {
  test(`malformed: ${testCase.name}`, async () => {
    // Refused with the reader's own error (SPEC.md §8). A TypeError or a
    // RangeError is the reader tripping over the file rather than refusing it.
    await assert.rejects(askMalformed(testCase), (error) => {
      assert.ok(error instanceof MokujiError, `${testCase.rule}: threw ${error}`);
      assert.ok(
        !(error instanceof TypeError || error instanceof RangeError || error instanceof ReferenceError),
        `${testCase.rule}: ${error.name}: ${error.message}`,
      );
      return true;
    }, `accepted: ${testCase.rule}`);
  });
}

test("the malformed cases were read", () => {
  assert.ok((tables.malformed ?? []).length > 0);
});
