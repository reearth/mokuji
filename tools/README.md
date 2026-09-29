# Measurement tools

None of these is part of the format or the reference implementation. They
exist to answer questions about it with numbers, and are kept so the numbers can
be re-taken rather than quoted.

The first two read a corpus produced by `mokuji dump`, so they measure the index
as it actually is rather than a corpus as it was before indexing:

```sh
cargo run --release -q -p mokuji-cli -- dump tokyo.mokuji > tokyo-dump.tsv
```

## `fst-comparison`

Builds a finite state transducer over the same terms and reports its size
against the blocked front-coded dictionary this workspace writes.

```sh
cd fst-comparison && cargo run --release -- ../../tokyo-dump.tsv
```

Excluded from the workspace, because it pulls in a dependency the format does
not use — which is the point: it measures the road not taken.

## `payload-encodings.py`

Encodes the payload column several ways — as published, rounded, fixed point,
delta — and reports what each would cost per address after per-block deflate.

```sh
python3 payload-encodings.py tokyo-dump.tsv
```

## `importance-order`

What `mokuji build --order-by` buys a reader and costs the file. It takes two
builds of one corpus — in corpus order, and numbered by a prior column — and a
file of queries, and answers each query's top `k` three ways: every run read
whole in either file, and on the ordered file only the first `--head` bytes of
each run, falling back to the rest when the heads cannot prove the answer.

```sh
python3 ../demo/poi/geonames.py --input allCountries.txt --out world.tsv
mokuji build world.tsv corpus.mokuji --tokenizer bigram \
  --attribute lon:5 --attribute lat:5 --attribute population --attribute names
mokuji build world.tsv ordered.mokuji --tokenizer bigram \
  --attribute lon:5 --attribute lat:5 --attribute population --attribute names \
  --order-by names
cd importance-order && cargo run --release -- \
  ../corpus.mokuji ../ordered.mokuji names queries.txt --head 1024
```

Measured on GeoNames `allCountries` (12,854,245 places, 5,834,149 terms), top
10, ranked by name count alone:

| | corpus order | by importance |
|---|---|---|
| File | 380.0 MB | 368.2 MB |
| Postings | 163.9 MB | 158.1 MB |
| Payload | 90.7 MB | 91.6 MB |

Numbering by importance makes the file **smaller**, not larger: important places
share their words, so their gaps shrink. This corpus arrives in GeoNames ID
order, which carries no locality for payload to lose; an address register in
key order does, and is not measured here.

Postings bytes per query, and range reads:

| Queries | whole runs | heads | reads, whole → heads | heads that fell back |
|---|---|---|---|---|
| 300 single words (`creek`, `church`) | 58,238 B | **1,010 B** | 11.6 → 8.8 | 0 |
| 1,000 famous places (`Black Sea`) | 104,173 B | 92,645 B | 10.4 → 12.6 | 53% |
| 300 common names (`San Antonio`) | 245,537 B | 200,591 B | 14.3 → 16.2 | 70% |

A single term is answered by the head of its run and nothing else — fifty-seven
times fewer bytes, and fewer payload reads, since the answer sits in the first
payload blocks. An intersection is not: the heads of two runs share too few
documents to prove a top ten, so most queries read the rest anyway and pay a
read per term for having tried. Numbering helps a one-word query and does
nothing for a multi-word one.

How many dictionary blocks a word prefix spans, for the first word of the famous
places, at 1024 terms a block:

| Characters | p50 | p90 | p99 | max |
|---|---|---|---|---|
| 1 | 114 | 375 | 526 | 526 |
| 2 | 12 | 52 | 116 | 156 |
| 3 | 3 | 9 | 24 | 38 |
| 4 | 1 | 2 | 6 | 11 |

`SPEC.md` §4.1 bounds a prefix by the blocks it spans, and at planet scale that
bound is not a small number until the third or fourth character.
