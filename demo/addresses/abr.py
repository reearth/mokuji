#!/usr/bin/env python3
"""Turn the Address Base Registry into a corpus `mokuji build` can index.

The registry is Japan's authoritative address dataset, published by the Digital
Agency as CSV under CC BY 4.0.

    https://dataset.address-br.digital.go.jp/

Two levels are supported.

`town` builds the nationwide town-level dictionary from three tables:

    mt_town_fullset_all.csv        the address text, nationwide
    mt_town_pos/mt_town_pos_pref*  representative coordinates, per prefecture,
                                   joined on `(lg_code, machiaza_id)`
    mt_city_pos_all.csv            municipalities' representative coordinates,
                                   joined on `lg_code`

The town table has a point for most 大字・町 and 丁目 and almost no 小字, so an
address without one is placed at its 大字・町's, and failing that at its
municipality's. `--pos-level` records which, so the index does not claim a
precision it does not have.

`rsdt` builds a prefecture's residence-level dictionary — the level a street
address is actually written at — from two tables joined on the five identifiers
that name a residence:

    mt_rsdtdsp_rsdt_pref<NN>.csv      the address text, including block and
                                      residence numbers
    mt_rsdtdsp_rsdt_pos_pref<NN>.csv  representative coordinates

The address text is written out as the registry spells it. Normalization is
left to `mokuji build --tokenizer ja-address`, so that the transformation which
decides what a term is gets recorded in the index's metadata rather than
happening invisibly in this script. See `TOKENIZERS.md`.

The payload is the coordinates, which is the division a geocoder wants: the key
is what a person types, and the value is what they were asking for.

Usage:
    python3 abr.py town --fullset mt_town_fullset_all.csv --pos pos/ \
                        --city-pos mt_city_pos_all.csv --pos-level --out town.tsv
    python3 abr.py rsdt --rsdt mt_rsdtdsp_rsdt_pref13.csv \
                        --pos mt_rsdtdsp_rsdt_pos_pref13.csv \
                        --pref 東京都 --out tokyo.tsv
"""

import argparse
import csv
import pathlib
import sys

# The town-level address text, outermost first. ABR leaves unused levels empty
# rather than absent, so concatenation is the whole of the assembly.
TOWN_LEVELS = ["pref", "county", "city", "ward", "oaza_cho", "chome", "koaza"]

# The same, minus the prefecture, which a per-prefecture file does not repeat.
RSDT_LEVELS = ["city", "ward", "oaza_cho", "chome", "koaza"]

# What identifies one residence, and therefore what the two tables join on.
RSDT_KEY = ["lg_code", "machiaza_id", "blk_id", "rsdt_id", "rsdt2_id"]


def write(addresses, out, echo=False, columns=None):
    """Writes the corpus, sorted, one address and its payload per line.

    `echo` puts the address into the payload as well as the text. A geocoder
    does not need it — the caller typed the address and wants coordinates back
    — but a search that only approximates the address does: it has to show
    which one it found. See ../fuzzy.

    `columns`, if given, is one or more further fields per line, tab-separated,
    read by `mokuji build --attribute` in the order they are written.
    """
    path = pathlib.Path(out)
    with path.open("w", encoding="utf-8") as handle:
        for term in sorted(addresses):
            payload = f"{term}|{addresses[term]}" if echo else addresses[term]
            extra = f"\t{columns[term]}" if columns is not None else ""
            handle.write(f"{term}\t{payload}{extra}\n")
    print(f"wrote {path} ({path.stat().st_size:,} bytes)", file=sys.stderr)


def rows(path):
    with open(path, encoding="utf-8") as handle:
        yield from csv.DictReader(handle)


# How precisely a town-level address is placed, written to the corpus as an
# attribute column. The registry publishes representative points for most
# 大字・町 and 丁目 but almost no 小字, so a point is taken from the nearest level
# above that has one, and the index says which level that was rather than
# passing a municipality's centre off as a koaza's.
OWN, OAZA, CITY = 0, 1, 2


def oaza_id(machiaza_id):
    """The 大字・町 a 丁目 or 小字 belongs to: the same ID with its last three
    digits zeroed, which is how the registry numbers them."""
    return machiaza_id[:4] + "000"


def town(args):
    positions = {}
    files = sorted(pathlib.Path(args.pos).glob("mt_town_pos_pref*.csv"))
    if not files:
        sys.exit(f"no mt_town_pos_pref*.csv under {args.pos}")
    for path in files:
        for row in rows(path):
            if row.get("rep_lon") and row.get("rep_lat"):
                positions[(row["lg_code"], row["machiaza_id"])] = (
                    row["rep_lon"],
                    row["rep_lat"],
                )
    print(f"{len(positions):,} town positions", file=sys.stderr)

    cities = {}
    if args.city_pos:
        for row in rows(args.city_pos):
            if row.get("rep_lon") and row.get("rep_lat"):
                cities[row["lg_code"]] = (row["rep_lon"], row["rep_lat"])
        print(f"{len(cities):,} municipality positions", file=sys.stderr)

    if args.municipality_prior and not args.municipalities:
        sys.exit("--municipality-prior needs --municipalities")
    if args.municipalities and not cities:
        sys.exit("--municipalities needs --city-pos, which is where their points are")

    def locate(row):
        lg_code, machiaza_id = row["lg_code"], row["machiaza_id"]
        if (position := positions.get((lg_code, machiaza_id))) is not None:
            return position, OWN
        if (position := positions.get((lg_code, oaza_id(machiaza_id)))) is not None:
            return position, OAZA
        if (position := cities.get(lg_code)) is not None:
            return position, CITY
        return None, None

    addresses = {}
    levels = {}
    total = duplicates = missing = 0
    placed = [0, 0, 0]
    for row in rows(args.fullset):
        total += 1
        term = "".join(row[level] for level in TOWN_LEVELS).strip()
        if not term:
            continue

        position, level = locate(row)
        if position is None:
            missing += 1
            if args.require_position:
                continue
        payload = ",".join(position) if position else ""

        if term in addresses:
            duplicates += 1
            # The registry carries alias rows, so the same text can appear
            # twice. Keep the first, unless a later one is placed more
            # precisely.
            if position is not None and (
                levels[term] is None or level < levels[term]
            ):
                addresses[term], levels[term] = payload, level
            continue
        addresses[term], levels[term] = payload, level

    named = set()
    if args.municipalities:
        before = len(addresses)
        named = municipalities(args, cities, addresses, levels)
        print(
            f"{len(named):,} municipalities, {len(addresses) - before:,} of them added "
            "as addresses of their own",
            file=sys.stderr,
        )

    for level in levels.values():
        if level is not None:
            placed[level] += 1
    report(total, addresses, duplicates, missing)
    print(
        f"placed by its own point {placed[OWN]:,}, by its 大字・町 {placed[OAZA]:,}, "
        f"by its municipality {placed[CITY]:,}",
        file=sys.stderr,
    )
    fields = []
    if args.pos_level:
        if missing and not args.require_position:
            sys.exit("--pos-level needs every address placed: pass --city-pos or --require-position")
        fields.append(lambda term: str(levels[term]))
    if args.municipality_prior:
        prior = str(args.municipality_prior)
        fields.append(lambda term: prior if term in named else "0")
    columns = (
        {term: "\t".join(field(term) for field in fields) for term in addresses}
        if fields
        else None
    )
    write(addresses, args.out, args.echo_address, columns)


def rsdt(args):
    positions = {}
    for row in rows(args.pos):
        if row.get("rep_lon") and row.get("rep_lat"):
            # A single string key rather than a tuple: at a million and a half
            # residences the difference in memory is worth the ugliness.
            key = "".join(row[field] for field in RSDT_KEY)
            positions[key] = (row["rep_lon"], row["rep_lat"])
    print(f"{len(positions):,} positions", file=sys.stderr)

    addresses = {}
    total = duplicates = missing = 0
    for row in rows(args.rsdt):
        total += 1
        town_part = "".join(row[level] for level in RSDT_LEVELS)
        block = row.get("blk_num", "")
        number = row.get("rsdt_num", "")
        branch = row.get("rsdt2_num", "")
        if not town_part or not block or not number:
            continue

        # 東京都千代田区内幸町一丁目6番1号
        term = f"{args.pref}{town_part}{block}番{number}号"
        if branch:
            term += f"の{branch}"

        position = positions.get("".join(row[field] for field in RSDT_KEY))
        if position is None:
            missing += 1
            if args.require_position:
                continue
        payload = ",".join(position) if position else ""

        if term in addresses:
            duplicates += 1
            if not addresses[term] and payload:
                addresses[term] = payload
            continue
        addresses[term] = payload

    report(total, addresses, duplicates, missing)
    write(addresses, args.out, args.echo_address)


def municipalities(args, cities, addresses, levels):
    """Adds each municipality as an address of its own, at its own point.

    The town master names every 町字 and never a municipality by itself, so a
    search for 和光市 can only answer with 和光市's towns. Added, 埼玉県和光市 is
    one more record — the shortest that matches, which is what a ranked search
    puts first, and the first a prefix completes.

    A ward of a designated city is a municipality in the master, and so is the
    city: 札幌市 has its own code and point in `mt_city_pos_all`, though no town
    row names it. Its code is the nearest one at or below its wards' that no town
    row uses.

    Returns every municipality's name, including the few a town row already
    spells the same way, which keep that row.
    """
    names = {}
    for row in rows(args.fullset):
        if row["ablt_date"]:
            continue
        names.setdefault(row["lg_code"], (row, "".join(row[k] for k in ("pref", "county", "city", "ward"))))
    designated = sorted(code for code in cities if code not in names)

    entries = {}
    for code, (row, name) in names.items():
        entries[name] = code
        if row["ward"]:
            city = max((c for c in designated if c[:2] == code[:2] and c <= code), default=None)
            if city is not None:
                entries.setdefault("".join(row[k] for k in ("pref", "county", "city")), city)

    named = set()
    for name, code in entries.items():
        if code not in cities:
            continue
        named.add(name)
        if name not in addresses:
            addresses[name], levels[name] = ",".join(cities[code]), CITY
    return named


def report(total, addresses, duplicates, missing):
    print(
        f"{total:,} rows -> {len(addresses):,} addresses "
        f"({duplicates:,} duplicate lines merged, {missing:,} without a position)",
        file=sys.stderr,
    )


def main():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--out", default="abr.tsv")
    common.add_argument(
        "--echo-address",
        action="store_true",
        help="put the address in the payload too, for a search that has to show what it matched",
    )
    common.add_argument(
        "--require-position",
        action="store_true",
        help="drop addresses with no representative point",
    )

    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="level", required=True)

    t = sub.add_parser("town", parents=[common], help="nationwide, town level")
    t.add_argument("--fullset", default="mt_town_fullset_all.csv")
    t.add_argument("--pos", default="pos")
    t.add_argument(
        "--city-pos",
        help="mt_city_pos_all.csv: place an address the registry has no town point "
        "for at its municipality's",
    )
    t.add_argument(
        "--pos-level",
        action="store_true",
        help="write which level each point came from (0 its own, 1 its 大字・町, "
        "2 its municipality) as a column for --attribute pos_level",
    )
    t.add_argument(
        "--municipalities",
        action="store_true",
        help="add each municipality as an address of its own, at its point in "
        "--city-pos, so that a search for 和光市 can answer 埼玉県和光市",
    )
    t.add_argument(
        "--municipality-prior",
        type=int,
        metavar="WEIGHT",
        help="write WEIGHT for a municipality's own record and 0 for a town, as "
        "one more column after --pos-level's: built with --attribute municipality "
        "and searched with --boost municipality, it is added to the score, so 和光市 "
        "answers 埼玉県和光市 before the towns that repeat its name",
    )
    t.set_defaults(run=town)

    r = sub.add_parser("rsdt", parents=[common], help="one prefecture, residence level")
    r.add_argument("--rsdt", required=True)
    r.add_argument("--pos", required=True)
    r.add_argument(
        "--pref",
        required=True,
        help="the prefecture name, which a per-prefecture file does not repeat",
    )
    r.set_defaults(run=rsdt)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
