#!/usr/bin/env python3
"""Link census small areas to the Address Base Registry's 町字.

The town-level index in ../addresses answers an address with a point; this
index answers a point with a polygon. To go from the one to the other — find an
address, get its boundary — each polygon here is given the keys the town index
answers with: the registry's town address, normalized exactly as the town
index's `ja-address` tokenizer normalizes it, so the term a town lookup
returns is a term this index holds.

There is no published table to join on. The registry's position table has a
column for the census small area (`cns_bnd_s_area_kcode`), and in the
2026-11-15 release it is empty in every row. So the link is made the way a
person would make it, and each one says how:

  name      the municipality code agrees and the names agree, once numerals,
            ヶ/ケ, の/ノ, 大字 and 字 are folded and a parenthesized note is
            dropped
  within    the census area is a part of a registry 町字, which its name begins
            with: the census divides some 大字 the registry does not
  point     the registry's own representative point for the 町字 falls inside
            the polygon
  oaza      none of those, so the 町字 takes the polygons of its 大字・町

    python3 link.py --fullset ../addresses/abr/mt_town_fullset_all.csv \\
        --pos ../addresses/abr/pos --boundaries 'boundaries/pref??.geojson' \\
        --out links.tsv

The output is one census `KEY_CODE` per line with the keys it takes, which
`covering.py --terms links.tsv` adds to that polygon's document.
"""

import argparse
import collections
import csv
import glob
import json
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from covering import contains, rings_of

ROOT = pathlib.Path(__file__).resolve().parents[2]

# The levels the town index joins into one address (../addresses/abr.py).
TOWN_LEVELS = ["pref", "county", "city", "ward", "oaza_cho", "chome", "koaza"]

KANJI = "〇一二三四五六七八九"


def kanji(number):
    """An integer as the census writes it: 十一, 二十, 二十一."""
    n = int(number)
    if n >= 100:
        return "".join(KANJI[int(d)] for d in str(n))
    if n < 10:
        return KANJI[n]
    tens, ones = divmod(n, 10)
    return ("" if tens == 1 else KANJI[tens]) + "十" + ("" if ones == 0 else KANJI[ones])


WIDE = str.maketrans("０１２３４５６７８９", "0123456789")


def fold(name):
    """A place name with the differences between the two sources folded away.

    The registry writes 美園１０条 and ５丁目; the census writes 美園十条 and
    五丁目. ヶ and ケ, の and ノ, and a leading 大字 or 字 vary between them too.
    """
    name = re.sub(r"[0-9０-９]+", lambda m: kanji(m.group().translate(WIDE)), name)
    name = name.replace("ヶ", "ケ").replace("ヵ", "ケ").replace("之", "ノ").replace("の", "ノ")
    return name.replace("大字", "").replace("字", "").replace("　", "").replace(" ", "")


def normalize(texts):
    """`ja-address` over many texts, by the JS implementation of it.

    The keys have to be the town index's terms byte for byte, and the tokenizer
    that made those is implemented in Rust and in JS, not in Python. The JS one
    is checked against the Rust one by the conformance suite.
    """
    script = (
        f"import {{ jaAddress }} from {json.dumps(str(ROOT / 'js/dist/tokenizers.js'))};"
        "import { readFileSync } from 'node:fs';"
        "const lines = readFileSync(0, 'utf8').split('\\n');"
        "process.stdout.write(lines.map(jaAddress).join('\\n'));"
    )
    out = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        input="\n".join(texts),
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    result = out.split("\n")
    assert len(result) == len(texts)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fullset", required=True, help="mt_town_fullset_all.csv")
    parser.add_argument("--pos", required=True, help="directory of mt_town_pos_pref*.csv")
    parser.add_argument("--boundaries", required=True, help="glob of the census GeoJSON")
    parser.add_argument("--skip", default="HCODE=8154", help="as covering.py --skip")
    parser.add_argument("--out", default="links.tsv")
    args = parser.parse_args()
    skip_name, _, skip_value = args.skip.partition("=")

    # The registry: every 町字 in force, its town index key, and its own point.
    towns = {}  # (lg_code, machiaza_id) -> row
    for row in csv.DictReader(open(args.fullset, encoding="utf-8")):
        if row["ablt_date"]:
            continue
        key = "".join(row[level] for level in TOWN_LEVELS).strip()
        if key:
            towns[(row["lg_code"], row["machiaza_id"])] = (row, key)
    points = {}
    for path in sorted(glob.glob(f"{args.pos}/mt_town_pos_pref*.csv")):
        for row in csv.DictReader(open(path, encoding="utf-8")):
            if row["rep_lon"] and row["rep_lat"]:
                points[(row["lg_code"], row["machiaza_id"])] = (
                    float(row["rep_lon"]),
                    float(row["rep_lat"]),
                )
    keys = sorted({key for _, key in towns.values()})
    term_of = dict(zip(keys, normalize(keys)))
    print(f"{len(towns):,} 町字 in force, {len(keys):,} distinct keys", file=sys.stderr)

    by_name = collections.defaultdict(dict)  # municipality -> folded name -> ids
    for town_id, (row, _) in towns.items():
        chome = kanji(row["chome_number"]) + "丁目" if row["chome_number"] else ""
        names = [row["oaza_cho"] + chome]
        if row["koaza"]:
            names.append(row["oaza_cho"] + chome + row["koaza"])
        for name in names:
            by_name[row["lg_code"][:5]].setdefault(fold(name), []).append(town_id)

    # The census: every polygon, by KEY_CODE, bucketed for point tests.
    areas = []  # (key_code, rings, bbox)
    grid = collections.defaultdict(list)
    links = collections.defaultdict(set)  # key_code -> town ids
    how = {}  # key_code -> method
    for path in sorted(glob.glob(args.boundaries)):
        for feature in json.load(open(path, encoding="utf-8"))["features"]:
            props = feature.get("properties") or {}
            if str(props.get(skip_name)) == skip_value:
                continue
            rings = rings_of(feature.get("geometry") or {})
            if not rings:
                continue
            code = props["KEY_CODE"]
            xs = [x for r in rings for x, _ in r]
            ys = [y for r in rings for _, y in r]
            bbox = (min(xs), min(ys), max(xs), max(ys))
            at = len(areas)
            areas.append((code, rings, bbox))
            for gx in range(int(bbox[0] * 10), int(bbox[2] * 10) + 1):
                for gy in range(int(bbox[1] * 10), int(bbox[3] * 10) + 1):
                    grid[(gx, gy)].append(at)

            if code in how:
                continue  # a further part of an area already linked
            names = by_name.get(props["PREF"] + props["CITY"], {})
            name = props.get("S_NAME") or ""
            plain = fold(re.sub(r"[（(].*?[）)]", "", name))
            if fold(name) in names or plain in names:
                links[code].update(names.get(fold(name)) or names[plain])
                how[code] = "name"
            elif plain and (within := [n for n in names if plain.startswith(n)]):
                links[code].update(names[max(within, key=len)])
                how[code] = "within"

    linked = {town for ids in links.values() for town in ids}
    agree = disagree = 0

    def holding(lon, lat):
        return [
            areas[i][0]
            for i in grid.get((int(lon * 10), int(lat * 10)), ())
            if areas[i][2][0] <= lon <= areas[i][2][2]
            and areas[i][2][1] <= lat <= areas[i][2][3]
            and contains(areas[i][1], lon, lat)
        ]

    by_town = collections.defaultdict(set)
    for code, ids in links.items():
        for town in ids:
            by_town[town].add(code)
    # A name link checked against the registry's own point, where it has one.
    for town, codes in by_town.items():
        if town in points:
            if set(holding(*points[town])) & codes:
                agree += 1
            else:
                disagree += 1

    methods = collections.Counter()
    for town in towns:
        if town in linked:
            methods["name" if any(how[c] == "name" for c in by_town[town]) else "within"] += 1
        elif town in points and (codes := holding(*points[town])):
            for code in codes:
                links[code].add(town)
                by_town[town].add(code)
            methods["point"] += 1
    for town in towns:
        if by_town.get(town):
            continue
        oaza = (town[0], town[1][:4] + "000")
        if by_town.get(oaza):
            for code in by_town[oaza]:
                links[code].add(town)
            methods["oaza"] += 1
        else:
            methods["none"] += 1

    with open(args.out, "w", encoding="utf-8") as out:
        for code in sorted(links):
            terms = sorted({term_of[towns[t][1]] for t in links[code]})
            out.write(f"{code}\t{' '.join(terms)}\n")

    codes = {a[0] for a in areas}
    print(f"{len(codes):,} census areas, {len(links):,} with a key", file=sys.stderr)
    for method in ("name", "within", "point", "oaza", "none"):
        print(f"  町字 linked by {method:6} {methods[method]:8,}", file=sys.stderr)
    print(
        f"name links checked against the registry's point: {agree:,} agree, {disagree:,} do not",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
