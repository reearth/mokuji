#!/usr/bin/env python3
"""How often does a damaged address still find the address it came from?

Addresses are drawn from the corpus the index was built from, damaged one way at
a time, and looked up again. Every query goes through the real CLI against the
real index, so the numbers are the ones a reader would get, not the ones a
re-implementation would.

    python3 recall.py --index fuzzy.mokuji --corpus town.tsv
    python3 recall.py --index fuzzy.mokuji --corpus town.tsv -- --boost municipality

Arguments after `--` are passed to every `mokuji search`, after the ones used
for an approximate query: `--any --probe 6 --length-norm 0 -n 5`.
"""

import argparse
import random
import subprocess

# Characters common in place names, for a substitution that looks like a typo
# rather than noise.
COMMON = "一二三五上下中北南東西本町村田川山口島原谷野大小長高"


def substitute(rng, a):
    i = rng.randrange(2, len(a))
    return a[:i] + rng.choice(COMMON) + a[i + 1 :]


def delete(rng, a):
    i = rng.randrange(2, len(a))
    return a[:i] + a[i + 1 :]


def transpose(rng, a):
    i = rng.randrange(1, len(a) - 1)
    return a[:i] + a[i + 1] + a[i] + a[i + 2 :]


def drop_prefecture(rng, a):
    for suffix in ("県", "都", "府", "道"):
        j = a.find(suffix)
        if 1 < j < 5:
            return a[j + 1 :]
    return None


def swap(pairs):
    def variant(rng, a):
        for x, y in pairs:
            if x in a[2:]:
                return a[:2] + a[2:].replace(x, y, 1)
        return None

    return variant


KINDS = {
    "One character substituted": substitute,
    "One character deleted": delete,
    "Two characters transposed": transpose,
    "Prefecture left off": drop_prefecture,
    "Kana variant (の↔ノ, ヶ↔ケ, の↔之)": swap(
        (("の", "ノ"), ("ノ", "の"), ("ヶ", "ケ"), ("ケ", "ヶ"), ("の", "之"), ("之", "の"))
    ),
    "が↔ヶ": swap((("が", "ヶ"), ("ヶ", "が"))),
}


def search(args, query):
    out = subprocess.run(
        [args.mokuji, "search", args.index, query, "--any", "--probe", "6",
         "--length-norm", "0", "-n", "5", *args.extra],
        capture_output=True,
        text=True,
        check=True,
    )
    found = [line.split("\t")[2].split("|")[0] for line in out.stdout.splitlines() if line.count("\t") == 2]
    reads = next(
        (int(line.split()[1]) for line in out.stderr.splitlines() if line.startswith("query:")), 0
    )
    return found, reads


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--index", required=True)
    parser.add_argument("--corpus", required=True, help="the TSV the index was built from")
    parser.add_argument("--mokuji", default="mokuji")
    parser.add_argument("-n", type=int, default=200, help="addresses per kind of damage")
    parser.add_argument("--seed", type=int, default=3)
    parser.add_argument("extra", nargs="*", help="further `mokuji search` arguments, after --")
    args = parser.parse_args()

    addresses = [line.split("\t", 1)[0] for line in open(args.corpus, encoding="utf-8")]
    rng = random.Random(args.seed)
    print(f"| What was done to the address | Top-1 | Top-5 | Reads |")
    print(f"|---|---|---|---|")
    for name, damage in KINDS.items():
        top1 = top5 = tried = reads = 0
        while tried < args.n:
            address = rng.choice(addresses)
            if len(address) < 6:
                continue
            query = damage(rng, address)
            if not query or query == address or len(query) < 4:
                continue
            tried += 1
            found, cost = search(args, query)
            reads += cost
            top1 += found[:1] == [address]
            top5 += address in found
        print(f"| {name} | {100 * top1 / tried:.0f}% | {100 * top5 / tried:.0f}% | {reads / tried:.1f} |", flush=True)


if __name__ == "__main__":
    main()
