#!/usr/bin/env python3
"""Turn the Address Base Registry into a postal-code corpus, both directions.

A postal code lookup runs both ways — code to address, address to code — and
they are usually two indexes, or one index and a reverse table. They do not have
to be. A term is a sorted byte string, and a postal code and a Japanese address
occupy disjoint, contiguous ranges of that ordering: `1000005` sorts among the
ASCII digits, `東京都…` among the CJK. Neither pollutes the other's prefixes.

So each record carries both keys, and one file answers in both directions.

Output is one record per line: the two keys separated by a space, then the
payload holding both halves.

    postal-code address<TAB>postal-code|address

Built from `mt_town_fullset_all.csv`, the same registry table the address demo
uses — see ../addresses for how to fetch it.

    python3 corpus.py --fullset abr/mt_town_fullset_all.csv --out postal.tsv
    mokuji build postal.tsv postal.mokuji --tokenizer space-split
    mokuji prefix postal.mokuji 1000005      # code to address
    mokuji prefix postal.mokuji 東京都千代田区丸の内   # address to code
"""

import argparse
import csv
import pathlib
import sys

LEVELS = ["pref", "county", "city", "ward", "oaza_cho", "chome", "koaza"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fullset", default="mt_town_fullset_all.csv")
    parser.add_argument("--out", default="postal.tsv")
    args = parser.parse_args()

    out = pathlib.Path(args.out)
    rows = kept = 0
    seen = set()
    records = []
    with open(args.fullset, encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            rows += 1
            code = row.get("post_code", "").strip()
            address = "".join(row[level] for level in LEVELS).strip()
            if not code or not address:
                continue
            # The registry carries alias rows; one code and one address name one
            # record however many times they are written down.
            if (code, address) in seen:
                continue
            seen.add((code, address))
            records.append((code, address))
            kept += 1

    # Sorted so that the corpus is stable and the two key spaces stay tidy.
    records.sort()
    with out.open("w", encoding="utf-8") as sink:
        for code, address in records:
            sink.write(f"{code} {address}\t{code}|{address}\n")

    codes = len({c for c, _ in records})
    print(
        f"{rows:,} rows -> {kept:,} records, {codes:,} distinct codes "
        f"-> {out} ({out.stat().st_size:,} bytes)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
