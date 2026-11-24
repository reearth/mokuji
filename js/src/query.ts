// Questions that take more than one lookup, answered from the primitives of
// MokujiIndex: a prefix that falls back to the longest term a query begins
// with, and the polygons holding a point, from a file of geohash coverings.

import type { CborValue, MokujiIndex, Postings } from "./mokuji.js";

const utf8 = new TextDecoder();

/** How a hit of `lookupPrefix` relates to what was asked. */
export type PrefixMatch = "exact" | "prefix" | "within";

export interface PrefixHit {
  term: string;
  doc: number;
  payload: string;
  /** `exact` for the term asked for, `prefix` for one that begins with it,
   * `within` for the longest term a reading begins with: what was asked lies
   * within it. */
  match: PrefixMatch;
}

/**
 * The terms that begin with `normalized`, as many as `limit` documents, or,
 * when none does, the longest term that one of `readings` begins with.
 *
 * The fallback is for keys that a query runs past: an address typed with
 * its block and lot after the town, when the file keys towns, finds the
 * town. Every prefix of every reading, from two characters to one fewer than
 * the reading, is looked up at once; prefixes of one string fall in one or
 * two dictionary blocks, read together. `normalized` and `readings` are
 * already normalized, as the file's tokenizer does (`index.normalize`).
 */
export async function lookupPrefix(
  index: MokujiIndex,
  normalized: string,
  limit = 20,
  readings: string[] = [],
): Promise<PrefixHit[]> {
  let entries = await index.prefix(normalized, limit);
  let within = false;
  if (!entries.length && readings.length) {
    const prefixes = [
      ...new Set(
        readings.flatMap((reading) => {
          const chars = Array.from(reading);
          return chars.slice(1, -1).map((_, i) => chars.slice(0, i + 2).join(""));
        }),
      ),
    ];
    const found = prefixes.length ? await index.exactMany(prefixes) : [];
    let longest = -1;
    found.forEach((entry, i) => {
      if (entry && (longest < 0 || prefixes[i]!.length > prefixes[longest]!.length)) longest = i;
    });
    if (longest >= 0) {
      entries = [found[longest]!];
      within = true;
    }
  }
  const postings = await index.postingsSpan(entries);
  const hits: Omit<PrefixHit, "payload">[] = [];
  entries.forEach((entry, i) => {
    const term = utf8.decode(entry.term);
    const match: PrefixMatch = within ? "within" : term === normalized ? "exact" : "prefix";
    for (const doc of postings[i]!.docs) hits.push({ term, doc, match });
  });
  const shown = hits.slice(0, limit);
  const payloads = await index.payloadSpan(shown.map((h) => h.doc));
  return shown.map((h, i) => ({ ...h, payload: payloads[i]! }));
}

// --- geohash coverings ---------------------------------------------------------
//
// A file in the shape demo/reverse builds: each polygon is a document, keyed
// by the geohash cells that cover it, coarse in its interior and fine along
// its boundary, with its label and rings as payload (`label|rings`). The
// format has no opinion about geohashes; the file says how it was built in
// its application-defined `attrs` (SPEC.md §6), and this reads them.

const BASE32 = "0123456789bcdefghjkmnpqrstuvwxyz";

/** The geohash cell of `precision` characters holding (lat, lon). */
export function geohash(lat: number, lon: number, precision: number): string {
  let lonLo = -180;
  let lonHi = 180;
  let latLo = -90;
  let latHi = 90;
  let out = "";
  let code = 0;
  for (let i = 0; i < precision * 5; i++) {
    if (i % 2 === 0) {
      const mid = (lonLo + lonHi) / 2;
      code = code * 2 + (lon >= mid ? 1 : 0);
      if (lon >= mid) lonLo = mid;
      else lonHi = mid;
    } else {
      const mid = (latLo + latHi) / 2;
      code = code * 2 + (lat >= mid ? 1 : 0);
      if (lat >= mid) latLo = mid;
      else latHi = mid;
    }
    if (i % 5 === 4) {
      out += BASE32[code];
      code = 0;
    }
  }
  return out;
}

function attr(index: MokujiIndex, name: string): CborValue | undefined {
  const meta = index.meta;
  if (!meta || typeof meta !== "object" || Array.isArray(meta) || meta instanceof Uint8Array) return undefined;
  const attrs = meta.attrs;
  if (!attrs || typeof attrs !== "object" || Array.isArray(attrs) || attrs instanceof Uint8Array) return undefined;
  return attrs[name];
}

/** The geohash precisions the file is keyed at (`geohash-precisions`, such
 * as `5,6,7`), or null when it does not say. */
export function precisionsOf(index: MokujiIndex): number[] | null {
  const declared = attr(index, "geohash-precisions");
  if (typeof declared !== "string") return null;
  const parsed = declared.split(",").map((p) => Number(p.trim()));
  return parsed.length && parsed.every((p) => Number.isInteger(p) && p > 0 && p <= 12) ? parsed : null;
}

/** Integers per degree the rings are stored in (`geometry`, such as
 * `delta-int-1e5`), or null when the file does not say. */
export function scaleOf(index: MokujiIndex): number | null {
  const declared = attr(index, "geometry");
  const m = typeof declared === "string" ? /^delta-int-1e(\d{1,2})$/.exec(declared) : null;
  return m ? 10 ** Number(m[1]) : null;
}

/** The names of the fields a label is cut into at U+001F
 * (`payload-fields`), or null when a label is one name. */
export function fieldsOf(index: MokujiIndex): string[] | null {
  const declared = attr(index, "payload-fields");
  return typeof declared === "string" && declared ? declared.split(",").map((f) => f.trim()) : null;
}

/** A polygon as `[lon, lat]` rings, closed. */
export type Rings = [number, number][][];

/** Rings as `geometry=delta-int-1e…` writes them: `;` between rings, each the
 * first vertex then the difference from the vertex before, in integers of
 * 1/`scale` degree, without the closing vertex, which is put back. */
export function decodeRings(text: string, scale: number): Rings {
  return text.split(";").map((part) => {
    const values = part.split(",");
    const ring: [number, number][] = [];
    let x = 0;
    let y = 0;
    for (let i = 0; i + 1 < values.length; i += 2) {
      x += Number(values[i]);
      y += Number(values[i + 1]);
      ring.push([x / scale, y / scale]);
    }
    if (ring.length) ring.push(ring[0]!);
    return ring;
  });
}

/** Whether (x, y) is inside, even-odd over every ring, so a hole is one. */
export function contains(rings: Rings, x: number, y: number): boolean {
  let inside = false;
  for (const ring of rings) {
    for (let i = 0; i < ring.length - 1; i++) {
      const [x1, y1] = ring[i]!;
      const [x2, y2] = ring[i + 1]!;
      if (y1 > y !== y2 > y && x < x1 + ((y - y1) * (x2 - x1)) / (y2 - y1)) inside = !inside;
    }
  }
  return inside;
}

export interface Polygon {
  doc: number;
  /** The payload before its rings. */
  label: string;
  /** The label cut at U+001F and named by `payload-fields`, or null when the
   * file names no fields. */
  fields: Record<string, string> | null;
  rings: Rings;
}

export interface Located {
  /** The point's cell, at the finest precision the file is keyed at. */
  cell: string;
  /** How many polygons the cells named, before the point was tested. */
  candidates: number;
  /** The ones holding the point. */
  inside: Polygon[];
}

/**
 * The polygons holding (lat, lon), from a file of geohash coverings.
 *
 * Every precision is looked up, not only the finest that matches, since a
 * point can be in one polygon's coarse interior cell and another's fine
 * boundary cell at once. The cells are prefixes of one string, so they fall
 * in neighbouring dictionary blocks, read together; the candidates' rings
 * come with their payload, and an even-odd test settles them. Throws for a
 * file that does not say its precisions and geometry, unless they are given.
 */
export async function locate(
  index: MokujiIndex,
  lat: number,
  lon: number,
  options: { precisions?: number[]; scale?: number } = {},
): Promise<Located> {
  const precisions = options.precisions ?? precisionsOf(index);
  const scale = options.scale ?? scaleOf(index);
  if (!precisions || !scale) {
    throw new Error("the file does not say its geohash precisions and geometry scale");
  }
  const names = fieldsOf(index);
  const cell = geohash(lat, lon, Math.max(...precisions));
  const found = (await index.exactMany(precisions.map((p) => cell.slice(0, p)))).filter((e) => e !== null);
  const postings: Postings[] = await Promise.all(
    found.map((e) => (e.postingsLength === 0 ? index.implied(e) : index.postingsSpan([e]).then(([p]) => p!))),
  );
  const docs = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b);
  const payloads = await index.payloadSpan(docs);
  const polygons = docs.map((doc, i): Polygon => {
    const payload = payloads[i]!;
    const cut = payload.indexOf("|");
    const label = payload.slice(0, cut);
    const values = label.split("\x1f");
    const fields = names ? Object.fromEntries(names.map((name, j) => [name, values[j] ?? ""])) : null;
    return { doc, label, fields, rings: decodeRings(payload.slice(cut + 1), scale) };
  });
  return { cell, candidates: polygons.length, inside: polygons.filter((p) => contains(p.rings, lon, lat)) };
}
