#!/usr/bin/env python3
"""Answer "which polygon holds this point" from an index built by covering.py.

The spatial part of the query is three lines: take the point's geohash, and look
up each prefix of it that the index actually uses. Everything after that is the
client's business — decode the geometry the lookup returned and test the point
against it.

    python3 locate.py reverse.mokuji 35.681236 139.767125

The index records which geohash precisions it holds under `geohash-precisions`,
so a reader learns how many lookups to make from the file rather than from an
agreement with whoever built it. Without that it would have to try every
precision, which is still bounded — just wastefully.
"""

import argparse
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from covering import SCALE, contains, geohash


def attrs(mokuji, path):
    """The application-defined metadata the file carries."""
    out = subprocess.run(
        [mokuji, "verify", str(path)], capture_output=True, text=True, check=True
    ).stdout
    found = {}
    for line in out.splitlines():
        if line.startswith("attr "):
            key, _, value = line[len("attr ") :].strip().partition(" = ")
            found[key] = value
    return found


def lookup(mokuji, path, term):
    """Every document holding `term`, and what the lookup cost."""
    out = subprocess.run(
        [mokuji, "search", str(path), term, "-n", "1000"],
        capture_output=True,
        text=True,
        check=True,
    )
    documents = []
    reads = 0
    # The matches go to stdout, the cost report to stderr.
    for line in out.stdout.splitlines() + out.stderr.splitlines():
        if "\t" in line:
            _, _, payload = line.partition("\t")
            documents.append(payload)
        elif line.startswith("query:"):
            reads = int(line.split()[1])
    return documents, reads


def decode(text):
    """The rings encoded by covering.py, back into coordinates."""
    rings = []
    for part in text.split(";"):
        values = [int(v) for v in part.split(",")]
        ring = []
        x = y = 0
        for i in range(0, len(values), 2):
            x += values[i]
            y += values[i + 1]
            ring.append((x / SCALE, y / SCALE))
        # covering.py drops the repeated closing vertex; the tests below walk
        # the ring as a closed loop, so put it back.
        ring.append(ring[0])
        rings.append(ring)
    return rings


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("index")
    parser.add_argument("lat", type=float)
    parser.add_argument("lon", type=float)
    parser.add_argument("--mokuji", default="mokuji")
    args = parser.parse_args()

    meta = attrs(args.mokuji, args.index)
    precisions = [int(p) for p in meta.get("geohash-precisions", "1,2,3,4,5,6,7").split(",")]

    cell = geohash(args.lon, args.lat, max(precisions))
    reads = 0
    candidates = []
    for precision in precisions:
        documents, cost = lookup(args.mokuji, args.index, cell[:precision])
        reads += cost
        candidates.extend(documents)

    answers = []
    for payload in candidates:
        label, _, geometry = payload.partition("|")
        if contains(decode(geometry), args.lon, args.lat):
            answers.append(label)

    print(f"cell      {cell}")
    print(f"lookups   {len(precisions)} ({', '.join(cell[:p] for p in precisions)})")
    print(f"candidate {len(candidates)}")
    print(f"reads     {reads}")
    for answer in answers:
        print(f"answer    {answer}")
    if not answers:
        print("answer    none: the point is outside every polygon in this index")


if __name__ == "__main__":
    main()
