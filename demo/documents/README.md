# Document search

The other shape. `../addresses` indexes a dictionary — one term names one
record, a query is a prefix, and the postings section is empty. This indexes a
collection: thousands of terms per record, a ranked intersection for a query,
and postings that dominate the file. Same layout, same writer, same reader.

Measured on the PLATEAU specifications, which are Markdown and public:

```sh
git clone https://github.com/eukarya-inc/plateau-spec
python3 corpus.py --docs plateau-spec/docs --out documents.tsv
mokuji build documents.tsv documents.mokuji --tokenizer bigram
mokuji search documents.mokuji 建築物 -n 3
```

```
365	7.087	procedure/tocC_11_01_02_02.md|C.11.1.2.2 作業手順
363	7.082	procedure/tocC_11_01_02.md|C.11.1.2 LOD1
299	7.081	procedure/tocC_02_01.md|C.2.1 建築物モデル

448 match(es), 3 shown, ranked by BM25
open:     1 read(s)
query:    6 read(s) - 4 dictionary and postings, 2 payload
bytes:    16488 of 2727976
```

| | |
|---|---|
| Corpus | 2,523 documents, 15.2 million characters, 25 MB |
| Terms | 77,240 bigrams |
| Index | 2.7 MB |
| Build | 2.1 s |

## Sections, against the address index

```
                    documents          addresses
term_index             1.3 KB            11.1 KB
term_blocks            1.10 MB  (40%)   863.2 KB  (12%)
postings               1.58 MB  (58%)          0        implied
payload                 43.5 KB (1.6%)    5.99 MB  (84%)
doc_lengths              4.3 KB                  absent
                       -------           -------
                        2.7 MB            7.1 MB
```

54,566 of the 77,240 bigrams, 71%, are rare enough that their postings are
written in their dictionary entries rather than as runs (`SPEC.md` §5.3.4).
That took the postings section from 1.96 MB to 1.58 MB and the file from 3.1 MB
to 2.7. It costs the query above a kilobyte, because the blocks it reads are
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
130 match(es)
$ mokuji search documents.mokuji 建築物のLOD -n 1 --phrase
42 match(es) as a phrase
```

Two thirds of the matches were terms co-occurring rather than a phrase — which
for a bigram index is most of what makes it imprecise, since the terms are
fragments and the question is nearly always whether they were adjacent.

| | without | with positions |
|---|---|---|
| Index | 2.7 MB | 11.5 MB |
| positions section | — | 8.7 MB (76%) |
| An ordinary query | 6 reads, 16.5 KB | **6 reads**, 22.6 KB |
| A phrase query | not possible | 19 reads |

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
