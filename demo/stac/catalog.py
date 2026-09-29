#!/usr/bin/env python3
"""Turn the items of a static STAC catalog into a corpus for `mokuji build`.

One item is one document. Its terms are three kinds of key, each under its own
prefix so that the kinds sort into separate ranges and none disturbs another's
scans:

    w:<word>        a word of the collection title, or the platform
    g<p>:<cell>     a geohash cell of precision p that the item's bbox touches
    d:<date>        the acquisition date, as YYYY-MM-DD

A date is a string whose order is the order of time, so a period is a range of
terms rather than a list of them. A cell is grouped by precision (`g4:` before
`g5:`) so that the cells a viewport needs sit together in the dictionary.

The payload is what a result needs to be shown and followed: the item's URL
relative to `--base`, its datetime, its bbox, and the collection title.

    python3 catalog.py --items items.jsonl --out stac.tsv \
        --base https://maxar-opendata.s3.amazonaws.com/events/

`items.jsonl` comes from harvest.py.
"""

import argparse
import json
import pathlib
import re
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "reverse"))
from covering import geohash  # noqa: E402

PRECISIONS = (1, 2, 3, 4, 5)


def cell_size(precision):
    """Width and height of a geohash cell, in degrees."""
    bits = precision * 5
    lon_bits = (bits + 1) // 2
    lat_bits = bits // 2
    return 360.0 / (1 << lon_bits), 180.0 / (1 << lat_bits)


def cover(bbox, precision):
    """Every cell of one precision that a bbox touches."""
    minx, miny, maxx, maxy = bbox
    w, h = cell_size(precision)
    cells = set()
    y = miny
    while True:
        x = minx
        while True:
            cells.add(geohash(min(x, maxx), min(y, maxy), precision))
            if x >= maxx:
                break
            x += w
        if y >= maxy:
            break
        y += h
    return cells


def words(text):
    return [w for w in re.split(r"[^0-9a-z]+", text.lower()) if w]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--items", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--base", default="", help="stripped from every item URL")
    args = parser.parse_args()

    items = [json.loads(line) for line in open(args.items, encoding="utf-8")]
    # Documents in the order a query is likely to ask for them: a collection,
    # then time. Results of one query then share payload blocks.
    items.sort(key=lambda i: (i["title"] or "", i["datetime"] or "", i["url"]))

    with open(args.out, "w", encoding="utf-8") as out:
        for item in items:
            terms = []
            terms += [f"w:{w}" for w in dict.fromkeys(words(item["title"] or ""))]
            if item.get("platform"):
                terms.append(f"w:{item['platform'].lower()}")
            for p in PRECISIONS:
                terms += [f"g{p}:{c}" for c in sorted(cover(item["bbox"], p))]
            date = (item["datetime"] or "")[:10]
            if date:
                terms.append(f"d:{date}")

            url = item["url"].removeprefix(args.base)
            bbox = ",".join(f"{v:.6f}" for v in item["bbox"])
            payload = "|".join([url, item["datetime"] or "", bbox, item["title"] or ""])
            clouds = item["clouds"] if item.get("clouds") is not None else -1
            gsd = item["gsd"] if item.get("gsd") is not None else -1
            out.write(f"{' '.join(terms)}\t{payload}\t{clouds}\t{gsd}\n")

    print(f"{len(items)} items -> {args.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
