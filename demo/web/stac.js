// Searching a static STAC catalog through a Mokuji index, in the browser.
//
// A static catalog is a tree of JSON files: to find the items over a place in a
// period, a client walks the tree and reads every item. The index built by
// ../stac/catalog.py answers the same question from terms instead — a word of
// the collection title, the geohash cells an item's bbox touches, and its date —
// and the query is the intersection of the three.
//
// Like the reverse geocoder next door, the index narrows and the page decides:
// a cell touching the viewport does not mean the item's bbox does, so the page
// tests the bbox that came back in the payload.
//
// See ../stac/README.md for how the index is built.

import { geohash } from "./reverse.js";

/** Width and height of a geohash cell, in degrees. */
export function cellSize(precision) {
  const bits = precision * 5;
  return [360 / 2 ** Math.ceil(bits / 2), 180 / 2 ** Math.floor(bits / 2)];
}

/** Every cell of one precision that a bbox touches. Same walk as catalog.py. */
export function cover([minx, miny, maxx, maxy], precision) {
  const [w, h] = cellSize(precision);
  const cells = new Set();
  for (let y = miny; ; y += h) {
    for (let x = minx; ; x += w) {
      cells.add(geohash(Math.min(x, maxx), Math.min(y, maxy), precision));
      if (x >= maxx) break;
    }
    if (y >= maxy) break;
  }
  return [...cells].sort();
}

/** The finest precision the index holds whose covering stays small. */
export function precisionFor(bbox, precisions, maxCells = 16) {
  let chosen = Math.min(...precisions);
  for (const p of [...precisions].sort((a, b) => a - b)) {
    if (cover(bbox, p).length <= maxCells) chosen = p;
  }
  return chosen;
}

export function intersects(a, b) {
  return a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
}

/** Postings for entries, one read per run of neighbours.
 *
 * `postingsSpan` reads from the first entry to the last in one request, which
 * is right for neighbours and wasteful for two words at opposite ends of the
 * dictionary. Entries are split wherever the gap between them is wider than
 * `maxGap` bytes.
 */
async function postingsOf(index, entries, maxGap = 4096) {
  const sorted = entries.filter(Boolean).sort((a, b) => a.postingsOffset - b.postingsOffset);
  const groups = [];
  for (const entry of sorted) {
    const last = groups.at(-1)?.at(-1);
    if (last && entry.postingsOffset - (last.postingsOffset + last.postingsLength) <= maxGap) {
      groups.at(-1).push(entry);
    } else {
      groups.push([entry]);
    }
  }
  const out = [];
  for (const group of groups) out.push(...(await index.postingsSpan(group)));
  return out;
}

const union = (lists) => [...new Set(lists.flatMap((p) => p.docs))].sort((a, b) => a - b);

function intersect(a, b) {
  const set = new Set(b);
  return a.filter((d) => set.has(d));
}

function parse(payload) {
  const [url, datetime, bbox, title] = String(payload).split("|");
  return { url, datetime, bbox: bbox.split(",").map(Number), title };
}

/**
 * Items matching every condition given, and what each condition cost.
 *
 * words      words of the collection title, all of which must match
 * bbox       [minx, miny, maxx, maxy]; items whose bbox touches it
 * from, to   YYYY-MM-DD, both inclusive
 * maxClouds  a percentage; read from a resident column, so it costs nothing
 */
export async function search(index, { words = [], bbox = null, from = null, to = null, maxClouds = null, limit = 50 }) {
  const conditions = [];

  for (const word of words) {
    const [entry] = await index.exactMany([`w:${word.toLowerCase()}`]);
    const docs = entry ? union(await postingsOf(index, [entry])) : [];
    conditions.push({ kind: "word", label: word, terms: 1, docs });
  }

  let cells = [];
  let precision = null;
  if (bbox) {
    const precisions = (index.meta?.attrs?.["geohash-precisions"] ?? "1,2,3,4,5").split(",").map(Number);
    precision = precisionFor(bbox, precisions);
    cells = cover(bbox, precision);
    const entries = await index.exactMany(cells.map((c) => `g${precision}:${c}`));
    const docs = union(await postingsOf(index, entries));
    conditions.push({ kind: "place", label: `${cells.length} cells at precision ${precision}`, terms: cells.length, docs });
  }

  if (from || to) {
    // A date is a string whose order is the order of time, so a period is a
    // run of neighbouring terms: one dictionary walk, one postings read.
    const entries = await index.range(`d:${from ?? "0000-00-00"}`, `d:${to ?? "9999-99-99"}`);
    const docs = union(await postingsOf(index, entries));
    conditions.push({ kind: "period", label: `${from ?? "…"} to ${to ?? "…"}`, terms: entries.length, docs });
  }

  if (conditions.length === 0) throw new Error("give at least one of a word, a place or a period");

  let matched = [...conditions].sort((a, b) => a.docs.length - b.docs.length).reduce(
    (acc, c) => (acc === null ? c.docs : intersect(acc, c.docs)),
    null,
  );

  if (maxClouds !== null) {
    const column = index.column("clouds");
    if (column) {
      const scale = 10 ** column.scaleExponent;
      matched = matched.filter((d) => {
        const v = column.values[d] * scale;
        return v >= 0 && v <= maxClouds;
      });
    }
  }

  const shown = matched.slice(0, limit);
  const payloads = await index.payloadSpan(shown);
  const candidates = shown.map((doc, i) => ({ doc, ...parse(payloads[i]) }));

  return {
    conditions,
    precision,
    cells,
    matched: matched.length,
    candidates,
    results: bbox ? candidates.filter((c) => intersects(c.bbox, bbox)) : candidates,
  };
}
