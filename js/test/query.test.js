// lookupPrefix and locate, against the conformance fixtures and a file of
// geohash coverings built here.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  MokujiBuilder,
  MokujiError,
  MokujiIndex,
  contains,
  decodeRings,
  encodeMetadata,
  geohash,
  locate,
  lookupPrefix,
} from "../dist/index.js";

const FIXTURES = fileURLToPath(new URL("../../conformance/fixtures/", import.meta.url));
const bytesSource = (bytes) => ({ read: (offset, length) => bytes.subarray(offset, offset + length) });
const open = (bytes) => MokujiIndex.open(bytesSource(bytes));
const fixture = async (name) => open(new Uint8Array(await readFile(`${FIXTURES}${name}.mokuji`)));

const MARUNOUCHI = "東京都千代田区丸の内1丁目1番1号";

test("lookupPrefix: the terms a prefix begins, and whether each is the one asked", async () => {
  const dict = await fixture("dict");
  const hits = await lookupPrefix(dict, "東京都千代田区丸の内1丁目1番", 10);
  assert.deepEqual(
    hits.map((h) => [h.term, h.doc, h.match]),
    [
      [MARUNOUCHI, 1, "prefix"],
      ["東京都千代田区丸の内1丁目1番2号", 2, "prefix"],
    ],
  );
  assert.equal(hits[0].payload, "139.767201,35.681560");
  assert.deepEqual((await lookupPrefix(dict, MARUNOUCHI, 10)).map((h) => h.match), ["exact"]);
});

test("lookupPrefix: an address typed past the key finds the term it begins with", async () => {
  const dict = await fixture("dict");
  const typed = `${MARUNOUCHI}5階`;
  assert.deepEqual(await lookupPrefix(dict, typed, 10), []);
  const hits = await lookupPrefix(dict, typed, 10, [typed]);
  assert.deepEqual(
    hits.map((h) => [h.term, h.doc, h.match, h.payload]),
    [[MARUNOUCHI, 1, "within", "139.767201,35.681560"]],
  );
  assert.deepEqual(await lookupPrefix(dict, "京都府", 10, ["京都府京都市"]), []);
});

test("lookupPrefix: the limit counts documents", async () => {
  const dict = await fixture("dict");
  assert.equal((await lookupPrefix(dict, "東京都", 2)).length, 2);
});

test("geohash agrees with the published example", () => {
  assert.equal(geohash(57.64911, 10.40744, 11), "u4pruydqqvj");
});

test("decodeRings closes each ring; contains is even-odd, so a hole is outside", () => {
  const rings = decodeRings("0,0,10,0,0,10,-10,0;3,3,4,0,0,4,-4,0", 1);
  assert.deepEqual(rings[0], [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]]);
  assert.ok(contains(rings, 1, 1));
  assert.ok(!contains(rings, 5, 5));
  assert.ok(!contains(rings, 11, 5));
  assert.throws(() => decodeRings("0,x", 1), /not integers/);
});

/** Rings as delta-int-1e5: the first vertex, then differences, unclosed. */
function encodeRings(rings) {
  return rings
    .map((ring) => {
      const ints = ring.map(([x, y]) => [Math.round(x * 1e5), Math.round(y * 1e5)]);
      return ints.map(([x, y], i) => (i ? [x - ints[i - 1][0], y - ints[i - 1][1]] : [x, y]).join(",")).join(",");
    })
    .join(";");
}

const square = (x0, y0, x1, y1) => [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];

/** Two blocks side by side, the first with a courtyard, each keyed by the
 * cells its corners are in at precisions 5 and 6. */
async function coverings() {
  const places = [
    { code: "A", name: "Marunouchi", rings: [square(139.76, 35.68, 139.765, 35.685), square(139.762, 35.682, 139.763, 35.683)] },
    { code: "B", name: "Otemachi", rings: [square(139.765, 35.68, 139.77, 35.685)] },
  ];
  const builder = new MokujiBuilder({ termsPerBlock: 4, docsPerPayloadBlock: 2 });
  const postings = new Map();
  places.forEach((place, doc) => {
    builder.addDocument(`${place.code}\x1f${place.name}|${encodeRings(place.rings)}`);
    for (const [lon, lat] of place.rings[0]) {
      for (const precision of [5, 6]) {
        const cell = geohash(lat, lon, precision);
        postings.set(cell, new Set([...(postings.get(cell) ?? []), doc]));
      }
    }
  });
  for (const cell of [...postings.keys()].sort()) {
    for (const doc of [...postings.get(cell)].sort()) builder.addPosting(cell, doc);
  }
  const bytes = await builder.finish(
    encodeMetadata({
      sources: [{ contentHash: "sha2-256:" + "0".repeat(64) }],
      tokenizerId: "identity",
      attrs: { "geohash-precisions": "5,6", geometry: "delta-int-1e5", "payload-fields": "CODE,NAME" },
    }),
  );
  return open(bytes);
}

test("locate: the polygons holding a point, named by the file's fields", async () => {
  const index = await coverings();
  const found = await locate(index, 35.6801, 139.7601);
  assert.equal(found.cell, geohash(35.6801, 139.7601, 6));
  assert.ok(found.candidates >= 1);
  assert.deepEqual(
    found.inside.map((p) => [p.doc, p.label, p.fields]),
    [[0, "A\x1fMarunouchi", { CODE: "A", NAME: "Marunouchi" }]],
  );
  assert.equal(found.inside[0].rings.length, 2);

  assert.deepEqual((await locate(index, 35.6801, 139.7699)).inside.map((p) => p.fields.CODE), ["B"]);
  // In the courtyard: a candidate, but in neither.
  const courtyard = await locate(index, 35.6825, 139.7625);
  assert.ok(courtyard.candidates >= 1);
  assert.deepEqual(courtyard.inside, []);
});

test("locate: a file that does not say how it was built is refused", async () => {
  const dict = await fixture("dict");
  await assert.rejects(locate(dict, 35.68, 139.76), (e) => e instanceof MokujiError && /geohash precisions/.test(e.message));
  // Unless the caller says.
  const found = await locate(dict, 35.68, 139.76, { precisions: [5], scale: 1e5 });
  assert.deepEqual(found.inside, []);
});
