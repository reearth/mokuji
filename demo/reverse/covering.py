#!/usr/bin/env python3
"""Turn a polygon layer into a corpus that answers "which polygon holds this point".

Reverse geocoding is a spatial query, and a concordance has no spatial index. It
does not need one. A geohash cell is a string whose *prefixes are the cells that
contain it*, so containment becomes the thing a sorted term dictionary is
already good at, and the whole spatial part collapses into ordinary term
lookups.

Each polygon is covered by a set of geohash cells of mixed precision: coarse
cells where the polygon's interior is thick, fine cells along its boundary. Those
cells are the polygon's terms. A cell that straddles a boundary names every
polygon it touches, so the answer a lookup returns is a *candidate set*, and the
client settles it with a point-in-polygon test against the geometry in the
payload. The index narrows; it does not decide.

The query is then: take the point's geohash at full precision and look up each
of its prefixes. That is a fixed number of term lookups — one per precision —
whatever the size of the layer.

Input is GeoJSON. Output is one document per feature, in the corpus format
`mokuji build` reads:

    <cell> <cell> ...<TAB><label>|<encoded geometry>

    python3 covering.py --geojson tokyo-cho.geojson --out reverse.tsv \\
        --label-fields PREF_NAME,CITY_NAME,S_NAME
    mokuji build reverse.tsv reverse.mokuji --tokenizer space-split

The covering is a superset of the polygon: every point inside the polygon lies
in some emitted cell. It is never a subset, which is what correctness needs —
an over-covering costs a wasted candidate, an under-covering loses an answer.
"""

import argparse
import json
import pathlib
import sys

BASE32 = "0123456789bcdefghjkmnpqrstuvwxyz"

# Coordinates are stored as integers this many per degree. Roughly 0.1 m, which
# is finer than any boundary layer is actually surveyed to, but it is what the
# source is published at and rounding is the publisher's call, not this tool's.
SCALE = 1_000_000


def ring_edges(rings):
    """Every ring's segments, flattened. Holes included: a hole's edges bound the
    polygon exactly as an outer ring's do, and the even-odd test below treats
    them the same way."""
    edges = []
    for ring in rings:
        for i in range(len(ring) - 1):
            x1, y1 = ring[i]
            x2, y2 = ring[i + 1]
            if x1 != x2 or y1 != y2:
                edges.append((x1, y1, x2, y2))
    return edges


def contains(rings, x, y):
    """Even-odd point-in-polygon over every ring, holes included."""
    inside = False
    for ring in rings:
        for i in range(len(ring) - 1):
            x1, y1 = ring[i]
            x2, y2 = ring[i + 1]
            if (y1 > y) != (y2 > y):
                if x < x1 + (y - y1) * (x2 - x1) / (y2 - y1):
                    inside = not inside
    return inside


def hits(edge, xlo, ylo, xhi, yhi):
    """Whether a segment meets an axis-aligned box, touching counted as meeting.

    Bounding-box rejection, then the separating axis along the segment's own
    normal. An edge that lies wholly inside the box counts as meeting it, which
    is what the caller needs: a box is only known to be free of the boundary
    when no edge is anywhere in it.
    """
    x1, y1, x2, y2 = edge
    if x1 > x2:
        if x1 < xlo or x2 > xhi:
            return False
    elif x2 < xlo or x1 > xhi:
        return False
    if y1 > y2:
        if y1 < ylo or y2 > yhi:
            return False
    elif y2 < ylo or y1 > yhi:
        return False
    dx = x2 - x1
    dy = y2 - y1
    a = dx * (ylo - y1) - dy * (xlo - x1)
    b = dx * (ylo - y1) - dy * (xhi - x1)
    c = dx * (yhi - y1) - dy * (xlo - x1)
    d = dx * (yhi - y1) - dy * (xhi - x1)
    return not (a > 0 and b > 0 and c > 0 and d > 0) and not (
        a < 0 and b < 0 and c < 0 and d < 0
    )


def cover(rings, max_precision):
    """The geohash cells covering `rings`, coarse inside and fine at the edge.

    Descends the geohash bit by bit, carrying only the edges that still meet the
    current box. A box no edge meets is wholly inside the polygon or wholly
    outside it, so one point test settles it and the descent stops — that is
    where the coarse interior cells come from. A box the boundary crosses is
    split until the precision limit, and emitted there whether or not it is
    mostly outside.

    Returns (cell, interior) pairs. `interior` marks a cell that lies wholly
    within the polygon, where a client can skip the point-in-polygon test.
    """
    out = []
    edges = ring_edges(rings)

    def walk(prefix, char, bit, lon_lo, lon_hi, lat_lo, lat_hi, edges, known):
        if not edges:
            # No boundary in this box, so the whole box shares the answer. Once
            # that is known it stays known for every box below, and the point
            # test is not worth repeating on the way down to a character
            # boundary.
            if known is None:
                known = contains(rings, (lon_lo + lon_hi) / 2, (lat_lo + lat_hi) / 2)
            if not known:
                return
            if bit == 0:
                out.append((prefix, True))
                return
        elif bit == 0 and len(prefix) >= max_precision:
            out.append((prefix, False))
            return

        # Geohash interleaves longitude and latitude, longitude first, and takes
        # five bits per character.
        index = len(prefix) * 5 + bit
        if index % 2 == 0:
            mid = (lon_lo + lon_hi) / 2
            halves = ((lon_lo, mid), (mid, lon_hi))
            boxes = [(lo, hi, lat_lo, lat_hi) for lo, hi in halves]
        else:
            mid = (lat_lo + lat_hi) / 2
            halves = ((lat_lo, mid), (mid, lat_hi))
            boxes = [(lon_lo, lon_hi, lo, hi) for lo, hi in halves]

        for value, (xlo, xhi, ylo, yhi) in enumerate(boxes):
            kept = [e for e in edges if hits(e, xlo, ylo, xhi, yhi)]
            code = char * 2 + value
            if bit == 4:
                walk(prefix + BASE32[code], 0, 0, xlo, xhi, ylo, yhi, kept, known)
            else:
                walk(prefix, code, bit + 1, xlo, xhi, ylo, yhi, kept, known)

    walk("", 0, 0, -180.0, 180.0, -90.0, 90.0, edges, None)
    return out


def centroid(rings):
    """The middle of the first ring's extent. Only ever used to sort, so the
    crudest measure of where a polygon is will do."""
    ring = rings[0]
    xs = [p[0] for p in ring]
    ys = [p[1] for p in ring]
    return (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2


def encode(rings):
    """The geometry, as delta-coded fixed-point integers in decimal.

    Rings separated by `;`, coordinates by `,`, each one the difference from the
    one before. Base64 of packed varints is smaller — measured, and by less than
    it looks: 1.8x smaller raw, but only 4.6% smaller once the payload section is
    deflated. Decimal is kept for the 4.6%, because it leaves the corpus a text
    file that can be read, diffed and checked by eye.
    """
    parts = []
    for ring in rings:
        px = py = 0
        values = []
        for x, y in ring[:-1]:
            ix = round(x * SCALE)
            iy = round(y * SCALE)
            values.append(str(ix - px))
            values.append(str(iy - py))
            px, py = ix, iy
        parts.append(",".join(values))
    return ";".join(parts)


def geohash(lon, lat, precision):
    """The geohash cell a point falls in. The query side of `cover`."""
    lon_lo, lon_hi, lat_lo, lat_hi = -180.0, 180.0, -90.0, 90.0
    out = []
    code = 0
    bit = 0
    for i in range(precision * 5):
        if i % 2 == 0:
            mid = (lon_lo + lon_hi) / 2
            if lon >= mid:
                code = code * 2 + 1
                lon_lo = mid
            else:
                code = code * 2
                lon_hi = mid
        else:
            mid = (lat_lo + lat_hi) / 2
            if lat >= mid:
                code = code * 2 + 1
                lat_lo = mid
            else:
                code = code * 2
                lat_hi = mid
        bit += 1
        if bit == 5:
            out.append(BASE32[code])
            code = 0
            bit = 0
    return "".join(out)


def rings_of(geometry):
    """Every ring of a Polygon or MultiPolygon, as one flat list.

    A multipart feature becomes one document with all its rings, and the even-odd
    test handles that correctly as long as the parts do not overlap — which for
    an administrative boundary layer they do not.
    """
    kind = geometry["type"]
    if kind == "Polygon":
        return [ring for ring in geometry["coordinates"]]
    if kind == "MultiPolygon":
        return [ring for part in geometry["coordinates"] for ring in part]
    return []


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--geojson", required=True)
    parser.add_argument("--out", default="reverse.tsv")
    parser.add_argument("--max-precision", type=int, default=7)
    parser.add_argument(
        "--label-fields",
        default="",
        help="comma-separated properties to join into the label",
    )
    parser.add_argument(
        "--key",
        action="store_true",
        help="index the label as a term too, so one file answers both directions",
    )
    parser.add_argument("--stats", action="store_true")
    args = parser.parse_args()

    fields = [f for f in args.label_fields.split(",") if f]
    data = json.loads(pathlib.Path(args.geojson).read_text(encoding="utf-8"))

    # Documents in spatial order, so that the payload blocks group neighbours.
    # The candidates a query has to fetch are always polygons near each other,
    # and a block holds several documents: ordering them this way turns a
    # handful of scattered payload reads into one or two.
    features = [f for f in data["features"] if rings_of(f.get("geometry") or {})]
    skipped = len(data["features"]) - len(features)
    features.sort(key=lambda f: geohash(*centroid(rings_of(f["geometry"])), 12))

    lines = []
    cells = 0
    interior = 0
    by_precision = {}
    vertices = 0
    for feature in features:
        rings = rings_of(feature["geometry"])
        properties = feature.get("properties", {})
        label = "".join(str(properties.get(f) or "") for f in fields)
        covering = cover(rings, args.max_precision)
        if not covering:
            skipped += 1
            continue
        cells += len(covering)
        interior += sum(1 for _, inside in covering if inside)
        for cell, _ in covering:
            by_precision[len(cell)] = by_precision.get(len(cell), 0) + 1
        vertices += sum(len(r) - 1 for r in rings)
        cells_text = " ".join(cell for cell, _ in covering)
        # An address and a geohash cell occupy disjoint ranges of one sorted
        # dictionary — CJK and base32 do not share prefixes — so a document can
        # carry both keys and the file answers in either direction.
        text = f"{label} {cells_text}" if args.key and label else cells_text
        lines.append(f"{text}\t{label}|{encode(rings)}")

    pathlib.Path(args.out).write_text("\n".join(lines) + "\n", encoding="utf-8")

    documents = len(lines)
    print(f"{documents} documents, {skipped} skipped", file=sys.stderr)
    print(f"{cells} cells, {cells / max(documents, 1):.1f} per document", file=sys.stderr)
    print(f"{interior} interior ({100 * interior / max(cells, 1):.0f}%)", file=sys.stderr)
    print(f"{vertices} vertices", file=sys.stderr)
    if args.stats:
        for precision in sorted(by_precision):
            print(f"  precision {precision}: {by_precision[precision]}", file=sys.stderr)


if __name__ == "__main__":
    main()
