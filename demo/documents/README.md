# Document search

The other shape. `../addresses` indexes a dictionary — one term names one
record, a query is a prefix, and the postings section is empty. This indexes a
collection: thousands of terms per record, a ranked intersection for a query,
and postings that dominate the file. Same layout, same writer, same reader.

Measured on the PLATEAU specifications, which are public. One document is one
section of a specification, with its own text and not its subsections' —
`corpus.py` says why that takes the JSON beside the Markdown:

```sh
git clone https://github.com/eukarya-inc/plateau-spec
python3 corpus.py --docs plateau-spec/docs --out documents.tsv
mokuji build documents.tsv documents.mokuji --tokenizer bigram
mokuji search documents.mokuji 建築物 -n 3
```

```
149	8.561	procedure/tocC_03_01_01|C.3.1.1 LOD0
144	8.537	procedure/tocC_02_01|C.2.1 建築物モデル
211	8.403	procedure/tocC_11_01_02_02|C.11.1.2.2 作業手順

294 match(es), 3 shown, ranked by BM25
open:     1 read(s)
query:    6 read(s) - 4 dictionary and postings, 2 payload
bytes:    17164 of 785085
```

| | |
|---|---|
| Corpus | 2,521 sections, 2.2 million characters |
| Terms | 23,448 |
| Index | 785 KB |
| Build | 0.3 s |

## Sections, against the address index

```
                    documents          addresses
term_index              168 B            11.1 KB
term_blocks           148.9 KB  (19%)   863.2 KB  (12%)
postings              579.5 KB  (74%)          0        implied
payload                51.5 KB  (6.6%)    5.99 MB  (84%)
doc_lengths             3.7 KB                   absent
                       -------           -------
                        785 KB            7.1 MB
```

14,209 of the 23,448 terms, 61%, are rare enough that their postings are
written in their dictionary entries rather than as runs (`SPEC.md` §5.3.4).
That took the postings section from 673 KB to 580 KB and the file from 839 KB
to 785. It costs the query above 3.4 KB, because the blocks it reads are
larger, and no reads.

Inverted, and neither shape required the format to choose. What a term is, what
payload means, whether a term's postings are worth storing, whether anything is
ranked — every one of those is a place the specification declined to decide.

## Why bigrams

`--tokenizer bigram` needs no dictionary, which is the point: a query has to be
analyzed the same way the terms were, so an analyzer that needs tens of
megabytes of dictionary needs them **at query time as well**, in whatever is
doing the querying. A page has no server to ask.

It costs precision. `京都` matches inside `東京都`, and the dictionary is larger
than one of words would be. `TOKENIZERS.md` records what was measured when the
alternatives were tried, including why ICU word segmentation is not registered
even though it is available in every browser and in Cloudflare Workers.

## Phrases

`--positions` stores where every term occurs, and `search --phrase` requires
them to be consecutive:

```
$ mokuji build documents.tsv documents.mokuji --tokenizer bigram --positions
$ mokuji search documents.mokuji 建築物のLOD -n 1
107 match(es)
$ mokuji search documents.mokuji 建築物のLOD -n 1 --phrase
12 match(es) as a phrase
```

Ninety-five of the 107 matches were terms co-occurring rather than a phrase — which
for a bigram index is most of what makes it imprecise, since the terms are
fragments and the question is nearly always whether they were adjacent.

| | without | with positions |
|---|---|---|
| Index | 785 KB | 2.5 MB |
| positions section | — | 1.6 MB (67%) |
| An ordinary query | 6 reads, 17.2 KB | **6 reads**, 20.7 KB |
| A phrase query | not possible | 13 reads |

**Positions cost bytes, not reads.** A query that asks no phrase never touches
the section; the read count is identical and the extra bytes are the two varints
each dictionary entry now carries to locate a run it may never fetch.

That is the case where being read in ranges changes what is worth storing. An
index downloaded before it is used pays for positions on every query whether or
not it asks a question that needs them — which is why formats meant to be
downloaded make positions optional and their users leave them off.

## What is missing here

**Ranking is the caller's.** `mokuji search` carries a sample BM25 built on the
document lengths the file supplies (`SPEC.md` §5.7); the format supplies the
statistics and has no opinion about the model.

**No highlighting.** Achievable with positions, or without them — put the document text in the
payload, fetch the matches, and re-run the tokenizer over what comes back — but
not built here.
