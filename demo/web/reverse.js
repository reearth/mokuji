// Reverse geocoding over a Mokuji index, in the browser.
//
// The spatial part is one function: a geohash cell is a string whose prefixes
// are the cells containing it, so "which polygon holds this point" is a handful
// of term lookups. Everything else here is the client's half of the bargain —
// decoding the geometry a lookup returned and testing the point against it,
// which the index deliberately does not do.
//
// See ../reverse/README.md for how the index is built.

const BASE32 = "0123456789bcdefghjkmnpqrstuvwxyz";

/** The geohash cell a point falls in. */
export function geohash(lon, lat, precision) {
  let lonLo = -180;
  let lonHi = 180;
  let latLo = -90;
  let latHi = 90;
  let out = "";
  let code = 0;
  let bit = 0;
  for (let i = 0; i < precision * 5; i++) {
    if (i % 2 === 0) {
      const mid = (lonLo + lonHi) / 2;
      if (lon >= mid) {
        code = code * 2 + 1;
        lonLo = mid;
      } else {
        code *= 2;
        lonHi = mid;
      }
    } else {
      const mid = (latLo + latHi) / 2;
      if (lat >= mid) {
        code = code * 2 + 1;
        latLo = mid;
      } else {
        code *= 2;
        latHi = mid;
      }
    }
    if (++bit === 5) {
      out += BASE32[code];
      code = 0;
      bit = 0;
    }
  }
  return out;
}

/** The rings encoded by covering.py, back into coordinates. */
export function decodeRings(text, scale = 1e6) {
  return text.split(";").map((part) => {
    const values = part.split(",");
    const ring = [];
    let x = 0;
    let y = 0;
    for (let i = 0; i < values.length; i += 2) {
      x += Number(values[i]);
      y += Number(values[i + 1]);
      ring.push([x / scale, y / scale]);
    }
    // The encoder drops the repeated closing vertex; the test below walks the
    // ring as a closed loop, so put it back.
    ring.push(ring[0]);
    return ring;
  });
}

/** Even-odd point-in-polygon over every ring, holes included. */
export function contains(rings, x, y) {
  let inside = false;
  for (const ring of rings) {
    for (let i = 0; i < ring.length - 1; i++) {
      const [x1, y1] = ring[i];
      const [x2, y2] = ring[i + 1];
      if (y1 > y !== y2 > y && x < x1 + ((y - y1) * (x2 - x1)) / (y2 - y1)) {
        inside = !inside;
      }
    }
  }
  return inside;
}

/** Which geohash precisions this index holds, as it records them itself.
 *
 * The format has no opinion about geohashes, so it does not define this. It
 * carries it: `--meta geohash-precisions=6,7` puts it in the application-defined
 * `attrs` map (SPEC.md §6). Without it a reader has to probe every precision —
 * still bounded, just wasteful.
 */
export function precisionsOf(index, fallback = [1, 2, 3, 4, 5, 6, 7]) {
  const declared = index.meta?.attrs?.["geohash-precisions"];
  if (typeof declared !== "string") return fallback;
  const parsed = declared
    .split(",")
    .map((p) => Number(p.trim()))
    .filter((p) => Number.isInteger(p) && p > 0 && p <= 12);
  return parsed.length > 0 ? parsed : fallback;
}

/** Integers per degree the geometry is stored at, as the file records it.
 *
 * `--meta geometry=delta-int-1e6` from the build; an index built with
 * `covering.py --decimals 5` says `1e5`. Absent, the encoder's default.
 */
export function scaleOf(index, fallback = 1e6) {
  const declared = index.meta?.attrs?.geometry;
  const match = typeof declared === "string" && /^delta-int-1e(\d{1,2})$/.exec(declared);
  return match ? 10 ** Number(match[1]) : fallback;
}

/** The polygons holding a point, and everything the lookup touched.
 *
 * Every prefix is looked up, not just the finest that matches: a point can be
 * inside one polygon's coarse interior cell and inside another polygon's fine
 * boundary cell at the same time, and stopping at the first hit would silently
 * drop the first.
 */
export async function locate(index, lat, lon) {
  const precisions = precisionsOf(index);
  const scale = scaleOf(index);
  const cell = geohash(lon, lat, Math.max(...precisions));

  const found = await index.exactMany(precisions.map((p) => cell.slice(0, p)));
  const entries = found.filter(Boolean);

  // Sorted by where their postings sit, so a span that is contiguous is read
  // once. Two cells one precision apart usually are.
  entries.sort((a, b) => a.postingsOffset - b.postingsOffset);
  const postings = await index.postingsSpan(entries);
  const docs = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b);
  const payloads = await index.payloadSpan(docs);

  const candidates = docs.map((doc, i) => {
    const payload = String(payloads[i]);
    const cut = payload.indexOf("|");
    return {
      doc,
      label: payload.slice(0, cut),
      rings: decodeRings(payload.slice(cut + 1), scale),
    };
  });

  return {
    cell,
    precisions,
    hits: entries.length,
    candidates,
    answers: candidates.filter((c) => contains(c.rings, lon, lat)),
  };
}
