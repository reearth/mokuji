# Place-name search

The third shape. `../addresses` indexes a dictionary — one exact key per record.
`../documents` indexes a collection — thousands of terms per record and no
coordinates. A gazetteer is neither: **several names for one record, all of
which should find it, and a position to return**.

```sh
curl -O https://download.geonames.org/export/dump/JP.zip && unzip JP.zip
python3 geonames.py --input JP.txt --out poi.tsv
mokuji build poi.tsv poi.mokuji --tokenizer bigram \
  --attribute lon:5 --attribute lat:5 --attribute population --attribute names
mokuji search poi.mokuji 富士山 -n 3 --boost names --length-norm 0
```

```
13337	17.521	Fujiyama
18038	17.036	Kofuji Yama
47092	16.902	Fuji Yama

9 match(es), 3 shown, ranked by BM25 and a static prior
query:    9 read(s), 11 KB of 4,918,611
```

| | |
|---|---|
| Source | GeoNames Japan, 103,757 rows, CC BY 4.0 |
| Places | 98,187, of which 84% carry a Japanese name |
| Index | 4.9 MB |

## Where the coordinates went

A place needs a name to show **and** a position to return, and a document has
exactly one payload. So the name is the payload and the coordinates are two
attribute columns (`SPEC.md` §5.8).

That is not a workaround; it is the better arrangement. Attribute columns are
resident, so a reader has every coordinate in the file before it fetches
anything — which is what filtering by map viewport would need. It costs 752 KB
held for 98,187 places, about eight bytes each, and that is the trade: numbers
you want in bulk go in columns, and the one thing you want per result goes in
payload.

```
term_blocks     628.6 KB  (13%)
postings          2.7 MB  (56%)
payload         694.9 KB  (14%)   display names
doc_attributes  752.3 KB  (15%)   lon, lat, population, names — resident
doc_lengths      76.3 KB
                 ------
                  4.9 MB
```

## What the ranking taught

Straight BM25 gets this corpus **wrong**, and instructively so:

```
$ mokuji search poi.mokuji 富士山 -n 3
67906	15.625	Fujisan Genshirin      ← a forest
47092	15.178	Fuji Yama              ← a hill
74822	15.178	Fuji Yama              ← another hill
```

Mount Fuji is in the index. It ranks ninth of nine.

BM25 normalizes by document length, and a gazetteer record's length is how many
names it has — which is the best fame signal the data offers. Japan carries 223
names, Tokyo 119, Mount Fuji 12, a nameless hill 6. **The normalization is
penalizing precisely the thing that indicates importance.**

Turning it off and adding the name count as a prior puts Mount Fuji first, and
`那覇空港` returns the airport rather than the station of the same name.

None of that is the format's business, and that is the point. The file supplied
the document lengths (§5.7) and the columns (§5.8); every decision that made the
ranking right — drop the length normalization, add this prior, weight it in log
space — was corpus-specific tuning, which is exactly the reason `SPEC.md` §1
keeps scoring outside.

## Attribution

GeoNames is published under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
No data is committed here; the commands above fetch it.
