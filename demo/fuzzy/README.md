# Approximate address search

The address index in [../addresses](../addresses) answers a prefix exactly, and
that is all it answers. Its tokenizer folds the spellings of a place name onto
one, so `丸ノ内` does find `丸の内` — but leave off the prefecture, or mistype a
character the folding says nothing about, and it returns nothing while being
right about it.

Approximate matching needs no new machinery. It is the same query with two
changes: **the union of the terms instead of their intersection**, and terms
that are character n-grams instead of whole addresses.

A one-character error leaves every bigram intact but two. An intersection asks
for all of them and finds nothing; a union asks for any of them and ranks by how
many were there.

```sh
# see ../addresses for how to fetch the registry
python3 ../addresses/abr.py town --fullset abr/mt_town_fullset_all.csv \
    --pos abr/pos --city-pos abr/mt_city_pos_all.csv \
    --municipalities --municipality-prior 10 --echo-address --out town.tsv
mokuji build town.tsv fuzzy.mokuji --tokenizer ja-address-bigram \
    --attribute municipality \
    $(python3 ../provenance.py args abr/mt_town_fullset_all.csv.zip \
        abr/mt_city_pos_all.csv.zip abr/pos/*.zip) \
    --source ../addresses/abr.py --license CC-BY-4.0 \
    --attribution "アドレス・ベース・レジストリ（デジタル庁）, CC BY 4.0"
```

```
$ mokuji search fuzzy.mokuji 東京都千代田区丸ノ内 --any --probe 6 --length-norm 0 -n 3
372775  44.667  東京都千代田区丸の内一丁目|139.767201,35.68156
372776  44.667  東京都千代田区丸の内三丁目|139.763476,35.676952
372777  44.667  東京都千代田区丸の内二丁目|139.763447,35.680022

probed:   都千:116 丸の:120 区丸:156 代田:1298 田区:1325 千代:1569
skipped:  3 term(s) with more postings, without reading any of them
3508 match(es), 3 shown, ranked by BM25
query:    16 read(s) - 15 dictionary and postings, 1 payload
bytes:    68431 of 17608103
```

```
$ mokuji search fuzzy.mokuji 茅ケ崎市 --any --probe 6 --length-norm 0 -n 2
463906  23.348  神奈川県茅ヶ崎市茅ヶ崎|139.408975,35.339706
463907  23.348  神奈川県茅ヶ崎市茅ヶ崎１丁目|139.40271,35.333515

probed:   茅ヶ:156 ヶ崎:2107 崎市:17806
query:    7 read(s) - 6 dictionary and postings, 1 payload
```

The second one never reaches the approximate part: `ja-address-bigram` folds
`茅ケ崎` and `茅ヶ崎` onto the same spelling, so the query is exact. The same
folding makes the *exact* index in [../addresses](../addresses) answer it too —
`mokuji prefix` on a `ja-address` file now completes `東京都千代田区丸ノ内`,
which it could not before, and the payload still shows the spelling the registry
uses.

| | |
|---|---|
| Addresses | 727,812: 725,970 町字 nationwide, and 1,842 municipalities |
| Tokenizer | `ja-address-bigram` |
| Terms | 155,254 |
| Postings | 9,096,299 |
| Index | 17.6 MB |
| A query | 5–17 range reads, 10–70 KB |

## A municipality by its name

Someone looking for a city types its name — `和光市`, not `埼玉県和光市`. The
registry's town master names every 町字 and never a municipality on its own, so
that query could only answer with 和光市's towns, in no useful order.
`--municipalities` adds each municipality as an address of its own, at its
point in `mt_city_pos_all`: 1,912 of them, wards and designated cities
included, of which 70 already had a town row spelled the same way.

That is not enough by itself. A town that repeats its municipality's name —
`広島県府中市府中町`, `北海道古宇郡泊村大字泊村` — holds the query's bigrams twice
and outranks it. So `--municipality-prior 10` writes a column, 10 for a
municipality and 0 for a town, and `--boost municipality` adds it to the score
as a static prior, `ln(1 + 10)`, about 2.4:

```
$ mokuji search fuzzy.mokuji 和光市 -n 3 --boost municipality
125271  24.719  埼玉県和光市|139.605693,35.781104
125288  20.559  埼玉県和光市広沢|139.60131,35.779974
125297  20.559  埼玉県和光市本町|139.60631,35.787518

35 match(es), 3 shown, ranked by BM25 and a static prior
query:    5 read(s) - 4 dictionary and postings, 1 payload
bytes:    15165 of 17608103

$ mokuji search fuzzy.mokuji 府中市 -n 3 --boost municipality
275611  19.409  広島県府中市|133.236323,34.568349
374178  19.409  東京都府中市|139.477663,35.668921
275637  17.656  広島県府中市府中町|133.232682,34.577183
```

Every municipality typed without its prefecture — `和光市`, `札幌市中央区`,
`境町` — and where its own record ranked:

| | First | First, or behind only others of the same name | In the top five |
|---|---|---|---|
| No prior | 65.4% | 66.0% | 92.6% |
| **Prior of 10** | **93.3%** | **95.3%** | **100%** |

Before the municipalities were added, none of them could be found at all. Nor
do they disturb a search for a town: 1,000 町字 typed without their prefecture
came back first 999 times, with the municipalities and the prior and without
either.

In a browser, `MokujiIndex.search` in [`@reearth/mokuji`](../../js) ranks
exactly as `mokuji search` does, options and all, and `../web/index.html` asks
this index whenever no address starts with what was typed: `和光市` there is 6
range requests and 19 KB, once the index is open, and opening it is 2 requests
and 621 KB, most of it the document lengths BM25 needs.

## Does it actually correct anything?

Recall is the only honest measure, so 200 real addresses per row were damaged
and looked up again, each query through the CLI against the index above:

```sh
python3 recall.py --index fuzzy.mokuji --corpus town.tsv
```

| What was done to the address | Top-1 | Top-5 | Reads |
|---|---|---|---|
| One character substituted | 80% | 89% | 19.5 |
| One character deleted | 81% | 92% | 18.5 |
| Two characters transposed | 70% | 85% | 19.8 |
| Prefecture left off | 92% | 96% | 17.2 |
| Kana variant (`の`↔`ノ`, `ヶ`↔`ケ`, `の`↔`之`) | 94% | 99% | 20.7 |
| `が`↔`ヶ` | 77% | 92% | 20.2 |

With `--boost municipality` every row is the same to the percent.

The kana row was the interesting one, because it is the error people actually
make, and before the tokenizer folded those spellings it scored **70%** — worse
than a random substitution. The reason was structural: `丸の内` → `丸ノ内`
destroys `丸の` and `の内`, which are exactly the rare bigrams `--probe` relies
on. Damage a common character and the query barely notices; damage a
distinctive one and it loses its best evidence.

So the fix went where it belonged, which was not the search. `ja-address-bigram`
folds `ノ`→`の`, `ケ`/`ヵ`→`ヶ` and `之`→`の` before tokenizing, and that row
moved to 91% / 99% with every other row unchanged and the index slightly
smaller; it is 94% / 99% on the registry measured above. See `TOKENIZERS.md` for what is folded and what was measured and left
out — `が` is the last row above, and it is already handled as well as any
substitution is.

## What this demonstrates

**The dictionary lets a reader plan its reads.** A term entry carries
`doc_freq`, so what a term's postings will cost is known before any postings are
read. `--probe 6` fetches the six rarest and reports the rest as skipped —
without having read them. For `東京都渋谷区道玄坂` that is 943 postings instead
of 47,538, **fifty times less**, with the same top three.

| Query | All terms | Six rarest | Reads | Same top 3 |
|---|---|---|---|---|
| 東京都千代田区丸ノ内 | 56,053 postings | 4,570 | 16 | yes |
| 大阪市中央区本町 | 45,483 | 26,261 | 14 | yes |
| 東京都渋谷区道玄坂 | 47,538 | **943** | 17 | yes |
| 札幌市中央区北一条西 | 44,912 | 13,886 | 15 | yes |
| 渋谷区神南 | 1,199 | 1,199 | 9 | yes |

**Skewed is what makes it work.** The median bigram names 2 addresses; the 99th
percentile names 865; `島県` names 117,249. Almost every bigram is cheap, and
the few that are not are the few that say least. A reader that can see the
distribution before paying for it gets the cheap ones and skips the rest.

**`--any` is a query, not a feature.** Nothing in the file knows it is being
searched approximately. The same index answers an intersection, and the same
BM25 that ranks a document collection ranks this. What changed is one flag in
the caller.

**Edit distance made it worse, measurably.** Reranking the top 30 by normalized
edit distance was the obvious next step, and it broke the flagship case:
`東京都千代田区丸ノ内` returned `東京都千代田区一番町` instead of
`東京都千代田区丸の内一丁目`. Edit distance rewards being the same *length*, and
the address someone wants is usually longer than what they typed — they stop
before `丁目`. BM25 over the rare bigrams already encodes what matters, so the
rerank was removed.

This is the second time a standard scorer had to be argued with rather than
adopted; the first was BM25's length normalization in [../poi](../poi), which is
why `--length-norm 0` appears above.

## Where it stops

**`--probe` picks by cost, not by usefulness.** The rarest six bigrams are a
good proxy for the most informative six, and nothing here checks that. A query
whose rare bigrams are all in one corner of the address will rank on that corner
alone.

**Nothing here is spelling correction.** There is no model of what people
mistype and no candidate generation — a query with two errors in the same short
name has little left to match on. Deletion neighbourhoods (SymSpell) would do
better at bounded edit distance, at the cost of an index several times larger.

**The read count grows with the query, not the corpus.** A ten-character address
is nine bigrams, so nine dictionary blocks. That is the `2k + m` of `SPEC.md` §4
with k set by how much the user typed — bounded, and larger than the three reads
an exact prefix costs. Approximation is not free; it is just cheap.
