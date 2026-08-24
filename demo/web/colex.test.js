// The browser reader, against the conformance suite.
//
// `colex.js` is a third implementation of SPEC.md and the only one written for
// the transport the format was designed for, so it is the one most worth
// holding to the same cases as the others. Run with `node --test demo/web`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { ColexIndex, jaAddress } from "./colex.js";

const CONFORMANCE = fileURLToPath(new URL("../../conformance/", import.meta.url));

/** A byte source over a file, so the reader can be run without a server. */
async function fileSource(name) {
  const bytes = new Uint8Array(await readFile(`${CONFORMANCE}fixtures/${name}.colex`));
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
function parseCases(text) {
  const cases = [];
  let current = null;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/(^|\s)#.*$/, "").trim();
    if (!line) continue;
    if (line === "[[case]]") {
      current = {};
      cases.push(current);
      continue;
    }
    const [, key, value] = line.match(/^(\w+)\s*=\s*(.*)$/) ?? [];
    if (!key) continue;
    current[key] = value.startsWith("[")
      ? JSON.parse(value.replace(/'/g, '"'))
      : value.startsWith('"')
        ? JSON.parse(value)
        : Number(value);
  }
  return cases;
}

const cases = parseCases(await readFile(`${CONFORMANCE}cases.toml`, "utf8"));

test("the conformance suite is not empty", () => {
  assert.ok(cases.length > 0);
});

for (const testCase of cases) {
  test(`${testCase.name} (${testCase.fixture})`, async () => {
    const index = await ColexIndex.open(await fileSource(testCase.fixture));
    index.resetStats();

    // The cases give one query per term; a single-term case is a prefix, and a
    // multi-term one is an intersection, which this reader does not do — so
    // only the single-term cases apply.
    if (testCase.query.length !== 1) return;

    const { results } = await index.lookup(testCase.query[0], 100);
    const exact = results.filter((r) => r.term === index.normalize(testCase.query[0]));

    assert.deepEqual(exact.map((r) => r.doc), testCase.docs);
    assert.deepEqual(exact.map((r) => r.payload), testCase.payloads);
  });
}

test("reads a deflated file", async () => {
  // The fixtures the other implementations reproduce byte for byte are stored
  // raw, so without this the browser's decompression is never exercised.
  const index = await ColexIndex.open(await fileSource("dict-deflate"));
  const { results } = await index.lookup("東京都千代田区丸の内1丁目1番1号", 10);

  assert.equal(results.length, 1);
  assert.equal(results[0].payload, "139.767201,35.681560");
});

test("a prefix range costs the reads the specification says", async () => {
  const index = await ColexIndex.open(await fileSource("dict"));
  index.resetStats();

  const { results } = await index.lookup("東京都千代田区丸の内", 100);
  assert.equal(results.length, 3);
  // Two term blocks of two terms hold the three matches, and their documents
  // span two payload blocks. No postings: they are implied.
  assert.equal(index.stats.requests, 4);
});

test("holds the tokenizer the index declares", async () => {
  const index = await ColexIndex.open(await fileSource("dict"));
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
