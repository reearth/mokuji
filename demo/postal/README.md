# Postal code lookup, both directions

A postal code lookup runs both ways — code to address, address to code — and is
usually two indexes, or one index and a reverse table.

It does not have to be. A term is a sorted byte string, and a postal code and a
Japanese address occupy **disjoint, contiguous ranges** of that ordering:
`1000001` sorts among the ASCII digits, `東京都…` among the CJK. Neither
pollutes the other's prefix scans. So each record carries both keys, and one
file answers in both directions.

```sh
# see ../addresses for how to fetch the registry
python3 corpus.py --fullset abr/mt_town_fullset_all.csv --out postal.tsv
mokuji build postal.tsv postal.mokuji --tokenizer space-split --no-doc-lengths
```

```
$ mokuji prefix postal.mokuji 1000001
1000001	1000001|東京都千代田区千代田
query:    3 read(s), 1.9 KB of 2,651,081

$ mokuji prefix postal.mokuji 東京都千代田区千代田
東京都千代田区千代田	1000001|東京都千代田区千代田
query:    3 read(s), 6.2 KB of 2,651,081
```

Prefixes work in both spaces too — `100` lists codes, `東京都千代田区` lists
addresses — because each is a prefix of a term and the dictionary is sorted.

| | |
|---|---|
| Records | 93,207 code-and-address pairs |
| Distinct codes | 82,690 |
| Terms | 175,752 — both key spaces in one dictionary |
| Index | 2.7 MB |
| Either direction | 3 range reads, or 2 with `--resident` |

## Where it comes from

The Address Base Registry's town master carries a `post_code` column, so no
second source is needed. It is not the whole postal file: 93,207 of 727,418
registry rows carry a code. A complete index would come from Japan Post's own
release, in exactly this shape.

## What this demonstrates

**Mixed key spaces cost nothing.** Nothing in the format knows that some terms
are numbers and some are addresses. They are bytes in a sorted list, and the
sorting keeps each kind together, which is all a prefix scan needs.

**One record, two keys, means stored postings.** A dictionary where each term
names its own record gets its postings for free (`SPEC.md` §5.3.4); here each
record has two terms, so the postings are written down — 1.1 MB of the 2.7 MB.
That is the price of the second direction, and it is cheaper than a second file.

**`--no-doc-lengths` exists because of this demo.** `build` stores per-document
lengths whenever the tokenizer splits, which is a good guess and a wrong one
here: every record is exactly two terms, so there is nothing to normalize and
the section was 24 KB of constants.
