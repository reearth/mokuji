# Measurement tools

None of these is part of the format or the reference implementation. They
exist to answer questions about it with numbers, and are kept so the numbers can
be re-taken rather than quoted.

The first two read a corpus produced by `mokuji dump`, so they measure the index
as it actually is rather than a corpus as it was before indexing:

```sh
cargo run --release -q -- dump tokyo.mokuji > tokyo-dump.tsv
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

### On a dictionary

The corpus above is a collection: a place has many terms and a term many places.
An address register is a dictionary, where each term names its own record and
the dictionary states its postings instead of storing them (`SPEC.md` §5.3.4).
That only holds while documents are in term order, so any `--order-by` gives it
up. Measured with the `mokuji` command alone, on 1,744,962 Tokyo addresses
(`ja-address`, `--payload ints:6`), in three orders: key order; by the number of
addresses in the municipality, a stand-in for importance that keeps each
municipality's addresses together; and at random, the worst case:

| | key order | by municipality | random |
|---|---|---|---|
| Term blocks | 862 KB | 863 KB | 1,165 KB |
| Postings | **0** | **12,149 KB** | 12,149 KB |
| Payload | 5,993 KB | 5,993 KB | 10,364 KB |
| Completing a prefix, 20 results | 2–3 reads, 0.7–1.1 KB | 3–4 reads, 1.0 KB | 22 reads, 8.5 KB |

(Attribute columns are left out; the random column alone is 10 MB.)

Keeping addresses together keeps payload exactly as small, so locality is not
what an order costs a dictionary. The postings are: 1.7 million runs of one
document each, about seven bytes apiece, stored because a term's document is no
longer its own ID — nearly twice the rest of the file, and one more
read per query. A random order loses locality as well and spends a payload read
on every result.

So a dictionary should stay in key order. Where an address needs a rank, it
belongs in a column or in a table beside the dictionary, not in the numbering.

### A table for the prefixes that are expensive

The prefix table above says a short prefix cannot be answered from the
dictionary within a small bound. It can be answered ahead of time instead. The
`completion` binary in the same crate reads an ordered file and builds, for
every prefix that is expensive, the ten most important documents holding a term
that begins with it — the ten smallest IDs among the heads of those terms' runs,
since the file is numbered by importance.

A prefix is expensive if it matches at least 1,024 terms, which would span more
than a block or two of the dictionary, or if those terms' postings together
exceed 16 KB, which a reader would fetch to find the heads among them. Every
other prefix is already cheap.

```sh
cd importance-order && cargo run --release --bin completion -- \
  ../ordered.mokuji --chars 12 --max-span 16384
```

On the same 12.9 million places:

| Characters | expensive prefixes | table, deflated | cheap prefixes, worst |
|---|---|---|---|
| 1 | 320 | 3.8 KB | 743 terms, 16.3 KB |
| 2 | 888 | 9.4 KB | 986 terms, 16.4 KB |
| 3 | 1,424 | 22.3 KB | 1,020 terms, 16.3 KB |
| 4 | 750 | 14.2 KB | 1,023 terms, 16.3 KB |
| 5 | 202 | 4.6 KB | 985 terms, 16.3 KB |
| 6–11 | 177 | 4.8 KB | 944 terms, 16.4 KB |
| 12 | 0 | — | 132 terms, 14.4 KB |
| **All** | **3,761** | **59 KB** | |

The table runs out at twelve characters, and it has to stay out: a longer
prefix matches a subset of what a shorter one does, so once no prefix of some
length is expensive, none longer is. **59 KB decides every prefix of every
length.** Held resident, it answers an expensive prefix with no read before the
payload, and leaves every other prefix at most two or three dictionary blocks
and 16 KB of postings.

It stores document IDs, not a ranking, so the format needs nothing new to carry
it beyond a section to put it in; whether that section belongs in the
specification is the question this measurement is for.

### Completing more than one word

The table decides a prefix of one term. What someone types into a search box is
often a finished word and an unfinished one — `black se` — and the `multiword`
binary measures that, two ways, on files numbered by importance:

- **words**, the `bigram` file above. Finished words are exact terms; the
  unfinished one is a prefix whose runs are united, then intersected with them.
- **names**, a second file where every name is one term under `name-list`, so
  what was typed is one prefix of a name. `geonames.py --whole-names` writes
  that corpus, names separated by U+001F, and the file carries its own
  completions section (`SPEC.md` §5.3.5).

```sh
python3 ../demo/poi/geonames.py --input allCountries.txt --out names.tsv --whole-names
mokuji build names.tsv names.mokuji --tokenizer name-list \
  --attribute lon:5 --attribute lat:5 --attribute population --attribute names \
  --order-by names --completions 1024,16384,10
cd importance-order && cargo run --release --bin multiword -- \
  ../ordered.mokuji ../names.mokuji typed.txt
```

The queries are the first 1,000 famous places typed partway: the first word cut
after one to three letters (`lon`), the second word cut the same way
(`black se`), and a Japanese name cut after two or three characters (`東京タ`).
Reads and bytes before the payload, with the table held resident. For words
the table is simulated, since that file carries none; for names it is the
section in the file, read through `Index::complete`:

| Typed | words | names + table |
|---|---|---|
| First word, 710 | p50 0, p99 3 reads; p99 22 KB | p50 0, p99 3 reads; p99 18 KB |
| Second word, 1,089 | p50 **29**, p99 **529** reads; p99 **13 MB** | p50 2, p99 3 reads; p99 19 KB |
| Japanese, 977 | 4 reads; p99 13 KB | 2–3 reads; p99 13 KB |

Words fail on the second word, and the table cannot rescue them. The table holds
the best ten for `se` alone, and the best ten of an intersection are not among
the best ten of either side, so the reader unites the runs of every term
beginning `se` — hundreds of dictionary blocks and megabytes of postings — to
intersect them with `black`.

Names turn the same query into one prefix, and the table then covers it: no
typed query cost more than 3 reads or 24 KB before its payload, and in all 2,776
the section gave the same ten documents as walking the dictionary without it.

The section itself holds 11,243 prefixes, 165 KB deflated — more than the 59 KB
of the word table, because names share long prefixes. The longest entries are
Thai, 48 to 50 bytes: `สถานีอนามัยตำบล` (subdistrict health station) and
`โรงเรียนบ้านหนอง` (Ban Nong school) begin thousands of names each. Coverage is
over bytes, not characters
(`SPEC.md` §5.3.5), and 1,549 of the entries, 14%, end inside a character.

The whole-name file is 416 MB against 368 MB for words, and it is a second file rather than a
replacement. It completes a name from its start only; `sea` does not find
`Black Sea`, which is why the two agree on 35–55% of their answers. A search
that finds a word anywhere still needs the word file; completion is better
served by the name file.

## `segments`

What a collection (`MANIFEST.md`) costs as deltas and deletes accumulate. Over
the name file above as the base, it lays the first `--deltas` of a directory of
delta files, deletes `--delete-rate` of the base's documents — uniformly, or
with `--important` among the first 100,000 — and completes the partly typed
names two ways: in collection order, and every segment's first ten, which is
what ranking across segments needs.

```sh
cd importance-order && cargo run --release --bin segments -- \
  ../names.mokuji ../deltas typed.txt --deltas 4 --delete-rate 0.001 --important
```

The figures are in `MANIFEST.md` §4 and §5. Two of them decide how to run a
collection: each delta of 1,000 places adds one read at open and one to a query
that asks every segment, while round trips stay at three; and deletes that fall
on important places stop mattering once completion entries list more than a
page shows — at twenty documents to an entry, 12,854 of them left no prefix
reading its dictionary.

The same tool measures shards: point it at one continent's file as the base
with no deltas, or at one continent with the other six as its "deltas", which
is what a query about no place pays. The figures are in `MANIFEST.md` §7.1.

