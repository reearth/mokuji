# Approximate address search

The address index in [../addresses](../addresses) answers a prefix exactly, and
that is all it answers. Type `丸ノ内` for `丸の内`, or leave off the prefecture,
and it returns nothing — while being right about it.

Approximate matching needs no new machinery. It is the same query with two
changes: **the union of the terms instead of their intersection**, and terms
that are character n-grams instead of whole addresses.

A one-character error leaves every bigram intact but two. An intersection asks
for all of them and finds nothing; a union asks for any of them and ranks by how
many were there.

```sh
# see ../addresses for how to fetch the registry
python3 ../addresses/abr.py town --fullset abr/mt_town_fullset_all.csv \
    --pos abr/pos/ --out town.tsv --echo-address
mokuji build town.tsv fuzzy.mokuji --tokenizer bigram \
    --source abr/mt_town_fullset_all.csv
```

```
$ mokuji search fuzzy.mokuji 東京都千代田区丸ノ内 --any --probe 6 --length-norm 0 -n 3
371608  35.966  東京都千代田区丸の内一丁目|139.767201,35.68156
371609  35.966  東京都千代田区丸の内三丁目|139.763476,35.676952
371610  35.966  東京都千代田区丸の内二丁目|139.763447,35.680022

probed:   丸ノ:33 都千:115 区丸:156 代田:1295 田区:1319 千代:1565
skipped:  3 term(s) with more postings, without reading any of them
3420 match(es), 3 shown, ranked by BM25
query:    16 read(s) - 15 dictionary and postings, 1 payload
bytes:    49261 of 17482113
```

```
$ mokuji search fuzzy.mokuji 渋谷区神南 --any --probe 6 --length-norm 0 -n 2
374703  32.885  東京都渋谷区神南１丁目|139.700363,35.663127
374704  32.885  東京都渋谷区神南２丁目|139.696982,35.666451

probed:   神南:37 渋谷:161 区神:394 谷区:607
query:    9 read(s) - 8 dictionary and postings, 1 payload
```

| | |
|---|---|
| Addresses | 725,855 町字, nationwide |
| Tokenizer | `bigram` |
| Terms | 157,285 |
| Postings | 9,223,043 |
| Index | 17.5 MB |
| A query | 9–17 range reads, 20–50 KB |

## Does it actually correct anything?

Recall is the only honest measure, so 200 real addresses per row were damaged
and looked up again, each query through the CLI against the real index:

| What was done to the address | Top-1 | Top-5 | Reads |
|---|---|---|---|
| One character substituted | 81% | 92% | 19.8 |
| One character deleted | 80% | 91% | 18.8 |
| Two characters transposed | 70% | 82% | 20.0 |
| Prefecture left off | 94% | 100% | 17.0 |
| Kana variant (`の`↔`ノ`, `ヶ`↔`ケ`) | 70% | 83% | 21.4 |

The last row is the interesting one, because it is the error people actually
make. It scores **worse than a random substitution**, and the reason is
structural: `丸の内` → `丸ノ内` destroys `丸の` and `の内`, which are exactly the
rare bigrams the probe was relying on. Damage a common character and the query
barely notices; damage a distinctive one and it loses its best evidence.

The fix is not in the search. Folding the variants before tokenizing — the same
corpus, the same index size, one preprocessing step — moves that row to **92% /
99%** and leaves every other row unchanged. That is a tokenizer's job, and the
registry in `TOKENIZERS.md` has no entry that does it yet.

## What this demonstrates

**The dictionary lets a reader plan its reads.** A term entry carries
`doc_freq`, so what a term's postings will cost is known before any postings are
read. `--probe 6` fetches the six rarest and reports the rest as skipped —
without having read them. For `東京都渋谷区道玄坂` that is 943 postings instead
of 47,538, **fifty times less**, with the same top three.

| Query | All terms | Six rarest | Same top 3 |
|---|---|---|---|
| 東京都千代田区丸ノ内 | 54,153 postings | 4,483 | yes |
| 大阪市中央区本町 | 45,483 | 26,261 | yes |
| 東京都渋谷区道玄坂 | 47,538 | 943 | yes |
| 札幌市中央区北一条西 | 45,037 | 14,011 | yes |

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
