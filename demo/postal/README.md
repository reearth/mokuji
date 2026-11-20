# Postal code lookup, both directions

A postal code lookup runs both ways — code to address, address to code — and is
usually two indexes, or one index and a reverse table.

It does not have to be. A term is a sorted byte string, and a postal code and a
Japanese address occupy **disjoint, contiguous ranges** of that ordering:
`1000001` sorts among the ASCII digits, `東京都…` among the CJK. Neither
pollutes the other's prefix scans. So each record carries both keys, and one
file answers in both directions.

Built from Japan Post's postal code table, every code in the country:

```sh
python3 ../provenance.py fetch \
  https://www.post.japanpost.jp/service/search/zipcode/download/utf/zip/utf_ken_all.zip \
  utf_ken_all.zip
unzip utf_ken_all.zip
python3 corpus.py --ken-all utf_ken_all.csv --out postal.tsv
mokuji build postal.tsv postal.mokuji --tokenizer space-split --no-doc-lengths \
  $(python3 ../provenance.py args utf_ken_all.zip) --source corpus.py \
  --attribution "郵便番号データ（日本郵便株式会社）"
```

```
$ mokuji prefix postal.mokuji 1000001
1000001	1000001|東京都千代田区千代田
query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    1474 of 2206580

$ mokuji prefix postal.mokuji 北海道札幌市中央区大通西
北海道札幌市中央区大通西	0600042|北海道札幌市中央区大通西（１〜１９丁目）
北海道札幌市中央区大通西	0640820|北海道札幌市中央区大通西（２０〜２８丁目）
query:    3 read(s) total - 1 dictionary, 0 postings, 2 payload
bytes:    5613 of 2206580
```

Prefixes work in both spaces too — `100` lists codes, `東京都千代田区` lists
addresses — because each is a prefix of a term and the dictionary is sorted.

| | |
|---|---|
| Records | 124,526 code-and-address pairs, from the table of 2026-10-30 |
| Distinct codes | 120,721 |
| Terms | 239,888 — both key spaces in one dictionary |
| Index | 2.2 MB |
| Build | 0.4 s |
| Either direction | 2 range reads, or 1 with `--resident`, plus one per further payload block |

## What the table says, and what is indexed

Each row is a code, a prefecture, a municipality and a town (`utf-readme.html`
describes the columns). Three town values are not towns, and are indexed as the
municipality alone:

| Town value | Rows | Means |
|---|---|---|
| 以下に掲載がない場合 | 1,870 | the code for an address in the municipality that no other row names |
| …の次に番地がくる場合 | 17 | the lot number follows the municipality directly |
| …一円 | 23 | a municipality with no town names |

6,479 towns carry a parenthesized note — （１〜１９丁目）, （次のビルを除く）,
（１階） — saying which part of the town the code is for. The note is shown, in
the payload, and left out of the key: `大通西` is one term naming both its
codes, and the payload says which is which. Telling them apart needs the chome
or the floor, which is past what a key lookup can do.

## The other source

The Address Base Registry's town master has a `post_code` column, so the same
corpus can be built from the table `../addresses` already fetches:

```sh
python3 corpus.py --fullset abr/mt_town_fullset_all.csv --out postal-abr.tsv
```

It is a subset: 93,205 records and 82,688 codes, from the one row in eight of
the registry that carries a code, in a 1.7 MB index. Japan Post's table is the
one to publish.

## Terms

Japan Post's readme for the table says 「郵便番号データに限っては日本郵便株式会社は著作権を主張しません。自由に配布していただいて結構です。」
— it claims no copyright in the postal code data and allows it to be
distributed freely. The build still records where the data came from and when,
as `--attribution` and the source's `modified` date.

## What this demonstrates

**Mixed key spaces cost nothing.** Nothing in the format knows that some terms
are numbers and some are addresses. They are bytes in a sorted list, and the
sorting keeps each kind together, which is all a prefix scan needs.

**Two keys a record, and still almost no postings.** A dictionary where each
term names its own record gets its postings for free (`SPEC.md` §5.3.4); here
each record has two terms, so most terms cannot. They name one or two records
each, and those are written in the dictionary entry rather than as runs: the
postings section is 16 KB of the 2.2 MB, and a lookup reads none. Before
postings could be written there, the registry's version of this index was
2.7 MB, 1.1 MB of it postings, and every lookup cost a read more.

**`--no-doc-lengths` exists because of this demo.** `build` stores per-document
lengths whenever the tokenizer splits, which is a good guess and a wrong one
here: every record is exactly two terms, so there is nothing to normalize and
the section was 24 KB of constants.
