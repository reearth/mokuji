# Questions that take more than one lookup, answered from the primitives of
# MokujiIndex: a prefix that falls back to the longest term a query begins
# with, and the polygons holding a point, from a file of geohash coverings.
# The same as js/src/query.ts.

import math
import re
from dataclasses import dataclass

from .reader import MokujiError, _field


@dataclass
class PrefixHit:
    """One document `lookup_prefix` found. `match` is `exact` for the term
    asked for, `prefix` for one that begins with it, `within` for the longest
    term a reading begins with: what was asked lies within it."""

    term: str
    doc: int
    payload: str
    match: str


def lookup_prefix(index, normalized, limit=20, readings=()):
    """The terms that begin with `normalized`, as many as `limit` documents,
    or, when none does, the longest term that one of `readings` begins with.

    The fallback is for keys that a query runs past: an address typed with its
    block and lot after the town, when the file keys towns, finds the town.
    Every prefix of every reading, from two characters to one fewer than the
    reading, is looked up at once; prefixes of one string fall in one or two
    dictionary blocks, read together. `normalized` and `readings` are already
    normalized, as the file's tokenizer does (`index.normalize`).
    """
    if limit == 0:
        # Nothing can be shown, so nothing is worth a read.
        return []
    entries = index.prefix(normalized, limit)
    within = False
    if not entries and readings:
        prefixes = list(dict.fromkeys(r[:n] for r in readings for n in range(2, len(r))))
        found = index.exact_many(prefixes) if prefixes else []
        longest = -1
        for i, entry in enumerate(found):
            if entry and (longest < 0 or len(prefixes[i]) > len(prefixes[longest])):
                longest = i
        if longest >= 0:
            entries = [found[longest]]
            within = True
    postings = index.postings_span(entries)
    hits = []
    for entry, run in zip(entries, postings):
        term = entry.term.decode(errors="replace")
        match = "within" if within else "exact" if term == normalized else "prefix"
        hits.extend((term, doc, match) for doc in run.docs)
    shown = hits[:limit]
    payloads = index.payload_span([doc for _, doc, _ in shown])
    return [PrefixHit(term, doc, payload, match) for (term, doc, match), payload in zip(shown, payloads)]


# --- geohash coverings -------------------------------------------------------
#
# A file in the shape demo/reverse builds: each polygon is a document, keyed by
# the geohash cells that cover it, coarse in its interior and fine along its
# boundary, with its label and rings as payload (`label|rings`). The format has
# no opinion about geohashes; the file says how it was built in its
# application-defined `attrs` (SPEC.md §6), and this reads them.

_BASE32 = "0123456789bcdefghjkmnpqrstuvwxyz"


def geohash(lat, lon, precision):
    """The geohash cell of `precision` characters holding (lat, lon)."""
    lon_lo, lon_hi, lat_lo, lat_hi = -180.0, 180.0, -90.0, 90.0
    out = []
    code = 0
    for i in range(precision * 5):
        if i % 2 == 0:
            mid = (lon_lo + lon_hi) / 2
            code = code * 2 + (lon >= mid)
            if lon >= mid:
                lon_lo = mid
            else:
                lon_hi = mid
        else:
            mid = (lat_lo + lat_hi) / 2
            code = code * 2 + (lat >= mid)
            if lat >= mid:
                lat_lo = mid
            else:
                lat_hi = mid
        if i % 5 == 4:
            out.append(_BASE32[code])
            code = 0
    return "".join(out)


def _attr(index, name):
    value = _field(_field(index.meta, "attrs"), name)
    return value if isinstance(value, str) else None


def precisions_of(index):
    """The geohash precisions the file is keyed at (`geohash-precisions`, such
    as `5,6,7`), or None when it does not say."""
    declared = _attr(index, "geohash-precisions")
    if declared is None:
        return None
    parts = [p.strip() for p in declared.split(",")]
    if not all(re.fullmatch(r"[0-9]+", p) and 0 < int(p) <= 12 for p in parts):
        return None
    return [int(p) for p in parts]


def scale_of(index):
    """Integers per degree the rings are stored in (`geometry`, such as
    `delta-int-1e5`), or None when the file does not say."""
    declared = _attr(index, "geometry")
    m = re.fullmatch(r"delta-int-1e([0-9]{1,2})", declared) if declared is not None else None
    return 10 ** int(m.group(1)) if m else None


def fields_of(index):
    """The names of the fields a label is cut into at U+001F
    (`payload-fields`), or None when a label is one name."""
    declared = _attr(index, "payload-fields")
    return [f.strip() for f in declared.split(",")] if declared else None


_INTEGER = re.compile(r"[+-]?[0-9]+", re.ASCII)


def decode_rings(text, scale):
    """Rings as `geometry=delta-int-1e…` writes them: `;` between rings, each
    the first vertex then the difference from the vertex before, in integers
    of 1/`scale` degree, without the closing vertex, which is put back. Each
    ring is a list of `(lon, lat)`, closed."""
    rings = []
    for part in text.split(";"):
        values = part.split(",")
        ring = []
        x = y = 0
        for i in range(0, len(values) - 1, 2):
            dx, dy = values[i].strip(), values[i + 1].strip()
            # ASCII digits only: `int` would also take `1_000` and other
            # scripts' digits, which no other reader does.
            if not (_INTEGER.fullmatch(dx) and _INTEGER.fullmatch(dy)):
                raise MokujiError("rings are not integers")
            x += int(dx)
            y += int(dy)
            ring.append((x / scale, y / scale))
        if ring:
            ring.append(ring[0])
        rings.append(ring)
    return rings


def contains(rings, x, y):
    """Whether (x, y) is inside, even-odd over every ring, so a hole is outside."""
    inside = False
    for ring in rings:
        for (x1, y1), (x2, y2) in zip(ring, ring[1:]):
            if (y1 > y) != (y2 > y) and x < x1 + (y - y1) * (x2 - x1) / (y2 - y1):
                inside = not inside
    return inside


@dataclass
class Polygon:
    doc: int
    # The payload before its rings.
    label: str
    # The label cut at U+001F and named by `payload-fields`, or None when the
    # file names no fields.
    fields: dict | None
    rings: list


@dataclass
class Located:
    # The point's cell, at the finest precision the file is keyed at.
    cell: str
    # How many polygons the cells named, before the point was tested.
    candidates: int
    # The ones holding the point.
    inside: list


def locate(index, lat, lon, precisions=None, scale=None):
    """The polygons holding (lat, lon), from a file of geohash coverings.

    Every precision is looked up, not only the finest that matches, since a
    point can be in one polygon's coarse interior cell and another's fine
    boundary cell at once. The cells are prefixes of one string, so they fall
    in neighbouring dictionary blocks, read together; the candidates' rings
    come with their payload, and an even-odd test settles them. Raises for a
    file that does not say its precisions and geometry, unless they are given.
    """
    # What the caller says counts only when it says something: no precisions,
    # or a scale that is not a positive number, defer to the file.
    if precisions is not None and len(precisions) > 0:
        if not all(isinstance(p, int) and not isinstance(p, bool) and 1 <= p <= 12 for p in precisions):
            raise MokujiError("the file does not say its geohash precisions and geometry scale")
    else:
        precisions = precisions_of(index)
    if not (isinstance(scale, (int, float)) and not isinstance(scale, bool) and math.isfinite(scale) and scale > 0):
        scale = scale_of(index)
    if not precisions or not scale:
        raise MokujiError("the file does not say its geohash precisions and geometry scale")
    names = fields_of(index)
    cell = geohash(lat, lon, max(precisions))
    found = [e for e in index.exact_many([cell[:p] for p in precisions]) if e is not None]
    # One entry at a time: the cells' runs need not be near each other, and
    # one span from the first to the last would read what lies between.
    docs = sorted({doc for e in found for doc in index.postings_span([e])[0].docs})
    payloads = index.payload_span(docs)
    polygons = []
    for doc, payload in zip(docs, payloads):
        label, _, geometry = payload.partition("|")
        values = label.split("\x1f")
        fields = {name: values[j] if j < len(values) else "" for j, name in enumerate(names)} if names else None
        polygons.append(Polygon(doc, label, fields, decode_rings(geometry, scale)))
    return Located(cell, len(polygons), [p for p in polygons if contains(p.rings, lon, lat)])
