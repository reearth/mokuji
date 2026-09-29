#!/usr/bin/env python3
"""Turn the GeoNames dump for a country into a corpus `mokuji build` can index.

Place-name geocoding — a POI index — is the third shape. An address dictionary
has one exact key per record; a document collection has thousands of terms per
record and no coordinates. A gazetteer has *several* names for one record, all
of which should find it, and a position to return.

    https://download.geonames.org/export/dump/

Output is one place per line, tab separated:

    names<TAB>display name<TAB>longitude<TAB>latitude<TAB>population<TAB>names

The names field holds every name the record has, so all of them are searchable;
the tokenizer decides how. Coordinates go in attribute columns rather than in
payload because a document has one payload and a place needs both a name to
show and a position to return — and numbers a reader can have in bulk, before
fetching anything, are what filtering by position would want anyway.

    python3 geonames.py --input JP.txt --out poi.tsv
    mokuji build poi.tsv poi.mokuji --tokenizer bigram \
      --attribute lon:5 --attribute lat:5 \
      --attribute population --attribute names
    mokuji search poi.mokuji 富士山 -n 5 --boost names

The last column is how many names the record has, which turns out to be a
usable prior for how well known a place is: Japan carries 223, Tokyo 119, Mount
Fuji 12, and a nameless hill 6. Population would be the obvious signal and is
useless here — every mountain has none.
"""

import argparse
import csv
import pathlib
import sys

# The columns of the GeoNames export, which has no header row.
GEONAMEID, NAME, ASCIINAME, ALTERNATES, LAT, LON, FCLASS, FCODE = range(8)
POPULATION = 14


def names_of(row):
    """Every name the record has, deduplicated, in a stable order."""
    seen, out = set(), []
    for name in [row[NAME], row[ASCIINAME], *row[ALTERNATES].split(",")]:
        name = name.strip()
        if name and name not in seen:
            seen.add(name)
            out.append(name)
    return out


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, help="a GeoNames country dump, e.g. JP.txt")
    parser.add_argument("--out", default="poi.tsv")
    parser.add_argument(
        "--classes",
        default="PSTHLV",
        help="GeoNames feature classes to keep; A (administrative) is dropped "
        "by default because an address index covers it better",
    )
    parser.add_argument(
        "--whole-names",
        action="store_true",
        help="separate the names by U+001F rather than spaces, for the "
        "name-list tokenizer, so that a name is completed as a whole rather "
        "than word by word",
    )
    args = parser.parse_args()

    out = pathlib.Path(args.out)
    kept = skipped = 0
    with open(args.input, encoding="utf-8") as handle, out.open("w", encoding="utf-8") as sink:
        for row in csv.reader(handle, delimiter="\t", quoting=csv.QUOTE_NONE):
            if len(row) <= POPULATION or row[FCLASS] not in args.classes:
                skipped += 1
                continue
            names = names_of(row)
            if not names or not row[LAT] or not row[LON]:
                skipped += 1
                continue
            # Tabs and newlines would break the corpus; a name containing one is
            # a data problem rather than something to guess at.
            if args.whole_names:
                text = "\x1f".join(names).replace("\t", " ")
            else:
                text = " ".join(names).replace("\t", " ")
            sink.write(
                f"{text}\t{names[0]}\t{row[LON]}\t{row[LAT]}\t"
                f"{row[POPULATION] or 0}\t{len(names)}\n"
            )
            kept += 1

    print(
        f"{kept:,} places kept, {skipped:,} skipped -> {out} ({out.stat().st_size:,} bytes)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
