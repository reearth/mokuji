# Searching a static STAC catalog

A static STAC catalog is a tree of JSON files on object storage: a root catalog,
collections under it, items under those. It is the cloud-optimized half of STAC —
publish it and walk away — and it answers exactly one kind of question, the one
you ask by following links.

*Which images cover this place, in this week?* is not that kind. A static
catalog answers it by being read whole. That is what a STAC API server is for.

This demo puts one Mokuji file beside the catalog and answers it with a handful of
range requests instead — no server, and the catalog left exactly as it was.

## The catalog

[Maxar Open Data](https://maxar-opendata.s3.amazonaws.com/events/catalog.json):
imagery released after natural disasters, published as static STAC under
CC BY-NC 4.0. At the time of writing, 55 events, 1,109 acquisition collections
and 40,027 items.

```sh
python3 harvest.py --root https://maxar-opendata.s3.amazonaws.com/events/catalog.json \
    --out items.jsonl
python3 catalog.py --items items.jsonl --out stac.tsv \
    --base https://maxar-opendata.s3.amazonaws.com/events/
mokuji build stac.tsv stac.mokuji \
    --attribute clouds:1 --attribute gsd:2 \
    --source items.jsonl \
    --meta geohash-precisions=1,2,3,4,5 \
    --meta base-url=https://maxar-opendata.s3.amazonaws.com/events/ \
    --meta catalog-documents=$(cat items.jsonl.documents)
```

`harvest.py` is the walk: every catalog, every collection, every item, one JSON
document each. Someone has to do it once. The point is that it is the publisher,
once, and not every reader, every time.

## What an item becomes

One item is one document, under three kinds of term:

| Prefix | Term | From |
|---|---|---|
| `w:` | a word of the collection title, or the platform | `Japan Earthquake Jan 2024` → `w:japan w:earthquake …` |
| `g1:`–`g5:` | every geohash cell of that precision the bbox touches | the same covering as [../reverse](../reverse) |
| `d:` | the acquisition date | `d:2024-01-02` |

A query is the intersection of whichever kinds it gives. The prefixes keep each
kind in its own range of the dictionary, the way postal codes and addresses keep
apart in [../postal](../postal).

A period is not a list of dates. A date written `YYYY-MM-DD` sorts in the order
of time, so *from 1 to 31 January* is the run of terms from `d:2024-01-01` to
`d:2024-01-31` — one walk along the dictionary and one postings read, however
long the period. That is the range query the format's ordered dictionary gives
for free.

Cloud cover and ground sample distance are attribute columns. They are resident,
so a filter on them costs no request.

The payload is what a result needs to be drawn and followed: the item's URL, its
datetime, its bbox and its collection's title.

## In the browser

```sh
cd ../web
python3 serve.py --index <any>.mokuji --stac ../stac/stac.mokuji
```

Open <http://localhost:8787/stac.html>, move the map, and search.

As in [../reverse](../reverse), the index narrows and the page decides: a cell
that touches the view does not mean the item inside it does, so the page tests
the bbox that came back. Orange is what the index returned; blue is what
survived.

## What this is not

It is not a STAC API. There is no CQL2, no arbitrary geometry beyond what a
covering approximates, no sorting beyond document order, and no paging token.
It covers the part of Item Search that is `bbox`, `datetime` and a word or two,
which is most of what a person looking at a map asks.

It is also not [stac-geoparquet](https://github.com/stac-utils/stac-geoparquet),
which puts items in columns for bulk access and analysis. The two sit side by
side: that one for reading many items' properties at once, this one for finding
a few items by word, place and time with a stated number of reads.
