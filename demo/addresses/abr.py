#!/usr/bin/env python3
"""Turn the Address Base Registry into a corpus `colex build` can index.

The registry is Japan's authoritative address dataset, published by the Digital
Agency as CSV under CC BY 4.0. Two of its tables are enough for a forward
geocoder at town level:

    mt_town_fullset_all.csv        the address text, nationwide
    mt_town_pos/mt_town_pos_pref*  representative coordinates, per prefecture

They join on `(lg_code, machiaza_id)`.

    https://dataset.address-br.digital.go.jp/

Output is one address per line: the address, a tab, then `lon,lat`.

The address text is written out as the registry spells it. Normalization is
left to `colex build --tokenizer ja-address`, so that the transformation which
decides what a term is gets recorded in the index's metadata rather than
happening invisibly in this script. See `TOKENIZERS.md`.

The payload is the coordinates, which is the division a geocoder wants: the key
is what a person types, and the value is what they were asking for.

Usage:
    python3 abr.py --fullset mt_town_fullset_all.csv --pos pos/ --out abr.tsv
"""

import argparse
import csv
import pathlib
import sys

# The address text, outermost first. ABR leaves unused levels empty rather than
# absent, so concatenation is the whole of the assembly.
LEVELS = ["pref", "county", "city", "ward", "oaza_cho", "chome", "koaza"]


def load_positions(directory):
    """Representative coordinates, keyed by (lg_code, machiaza_id)."""
    positions = {}
    files = sorted(pathlib.Path(directory).glob("mt_town_pos_pref*.csv"))
    if not files:
        sys.exit(f"no mt_town_pos_pref*.csv under {directory}")
    for path in files:
        with path.open(encoding="utf-8") as handle:
            for row in csv.DictReader(handle):
                lon, lat = row.get("rep_lon"), row.get("rep_lat")
                if lon and lat:
                    positions[(row["lg_code"], row["machiaza_id"])] = (lon, lat)
    return positions


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fullset", default="mt_town_fullset_all.csv")
    parser.add_argument("--pos", default="pos")
    parser.add_argument("--out", default="abr.tsv")
    parser.add_argument(
        "--require-position",
        action="store_true",
        help="drop addresses with no representative point",
    )
    args = parser.parse_args()

    positions = load_positions(args.pos)
    print(f"{len(positions):,} positions", file=sys.stderr)

    # A dictionary, not a list: the registry carries alias rows, so the same
    # address text can appear more than once. Terms that collide only after
    # normalization are merged by the builder, which is the component that
    # knows what normalization was applied.
    addresses = {}
    rows = collisions = missing = 0
    with open(args.fullset, encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            rows += 1
            term = "".join(row[level] for level in LEVELS).strip()
            if not term:
                continue

            position = positions.get((row["lg_code"], row["machiaza_id"]))
            if position is None:
                missing += 1
                if args.require_position:
                    continue
            payload = ",".join(position) if position else ""

            if term in addresses:
                collisions += 1
                # Keep the first, and prefer any payload over none.
                if not addresses[term] and payload:
                    addresses[term] = payload
                continue
            addresses[term] = payload

    out = pathlib.Path(args.out)
    with out.open("w", encoding="utf-8") as handle:
        for term in sorted(addresses):
            handle.write(f"{term}\t{addresses[term]}\n")

    print(
        f"{rows:,} rows -> {len(addresses):,} addresses "
        f"({collisions:,} duplicate lines merged, {missing:,} without a position)",
        file=sys.stderr,
    )
    print(f"wrote {out} ({out.stat().st_size:,} bytes)", file=sys.stderr)


if __name__ == "__main__":
    main()
