#!/usr/bin/env python3
"""Turn a postal code table into a corpus that answers both directions.

A postal code lookup runs both ways — code to address, address to code — and
they are usually two indexes, or one index and a reverse table. They do not have
to be. A term is a sorted byte string, and a postal code and a Japanese address
occupy disjoint, contiguous ranges of that ordering: `1000005` sorts among the
ASCII digits, `東京都…` among the CJK. Neither pollutes the other's prefixes.

So each record carries both keys, and one file answers in both directions.

Output is one record per line: the two keys separated by a space, then the
payload holding both halves.

    postal-code address<TAB>postal-code|address

Built from either of two tables:

  --ken-all   Japan Post's own file, `utf_ken_all.csv`: every postal code
  --fullset   the Address Base Registry's town master, whose `post_code` column
              carries a code for about one row in eight (see ../addresses)

    python3 corpus.py --ken-all utf_ken_all.csv --out postal.tsv
    mokuji build postal.tsv postal.mokuji --tokenizer space-split
    mokuji prefix postal.mokuji 1000005      # code to address
    mokuji prefix postal.mokuji 東京都千代田区丸の内   # address to code
"""

import argparse
import csv
import pathlib
import re
import sys

LEVELS = ["pref", "county", "city", "ward", "oaza_cho", "chome", "koaza"]


def registry(path):
    """(code, address, address as shown) from the registry's town master."""
    with open(path, encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            code = row.get("post_code", "").strip()
            address = "".join(row[level] for level in LEVELS).strip()
            yield code, address, address


def ken_all(path):
    """(code, address, address as shown) from Japan Post's `utf_ken_all.csv`.

    Columns 3, 7, 8 and 9 are the code, the prefecture, the municipality and the
    town (`utf-readme.html`). Three town values are not towns: 以下に掲載がない場合,
    the code for an address in the municipality that no other row names;
    …の次に番地がくる場合, where the lot number follows the municipality; and
    …一円, a municipality with no town names. Each is indexed as the
    municipality alone.

    A parenthesized note — （１〜１９丁目）, （次のビルを除く）, （１階） — narrows
    which part of a town the code is for. It stays in what is shown and is left
    out of the key, so that 大通西 finds both of its codes.
    """
    with open(path, encoding="utf-8") as handle:
        for row in csv.reader(handle):
            code, pref, city, town = row[2], row[6], row[7], row[8]
            if (
                town == "以下に掲載がない場合"
                or town.endswith("の次に番地がくる場合")
                or town == f"{city}一円"
            ):
                town = ""
            shown = pref + city + town
            key = pref + city + re.sub(r"（.*?）", "", town)
            yield code, key, shown


def main():
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--ken-all", help="Japan Post's utf_ken_all.csv")
    source.add_argument("--fullset", help="the registry's mt_town_fullset_all.csv")
    parser.add_argument("--out", default="postal.tsv")
    args = parser.parse_args()

    out = pathlib.Path(args.out)
    rows = kept = 0
    seen = set()
    records = []
    table = ken_all(args.ken_all) if args.ken_all else registry(args.fullset)
    for code, key, shown in table:
        rows += 1
        if not code or not key:
            continue
        # The registry carries alias rows, and Japan Post a row per part of a
        # town; one code and one address shown are one record however many
        # times they are written down.
        if (code, shown) in seen:
            continue
        seen.add((code, shown))
        records.append((code, key, shown))
        kept += 1

    # Sorted so that the corpus is stable and the two key spaces stay tidy.
    records.sort()
    with out.open("w", encoding="utf-8") as sink:
        for code, key, shown in records:
            sink.write(f"{code} {key}\t{code}|{shown}\n")

    codes = len({c for c, _, _ in records})
    print(
        f"{rows:,} rows -> {kept:,} records, {codes:,} distinct codes "
        f"-> {out} ({out.stat().st_size:,} bytes)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
