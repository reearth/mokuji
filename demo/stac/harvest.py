#!/usr/bin/env python3
"""Walk a static STAC catalog and write every item it reaches to items.jsonl.

This is the walk the index exists to make unnecessary, done once, by whoever
publishes the index. It follows `child` and `item` links from the root and
counts every JSON document it had to read, which the index records as
`catalog-documents` so that a reader can say what the alternative would cost.

    python3 harvest.py --root https://maxar-opendata.s3.amazonaws.com/events/catalog.json \
        --out items.jsonl

A partial run is resumed: items already in the output are not fetched again.
"""

import argparse
import concurrent.futures as cf
import json
import os
import posixpath
import sys
import time
import urllib.parse
import urllib.request


def get(url):
    for attempt in range(4):
        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                return json.load(response)
        except Exception as err:  # noqa: BLE001 — retried, then re-raised
            last = err
            time.sleep(1 + attempt)
    raise last


def resolve(base, href):
    parts = urllib.parse.urlsplit(base)
    path = posixpath.normpath(posixpath.join(posixpath.dirname(parts.path), href))
    return urllib.parse.urlunsplit((parts.scheme, parts.netloc, path, "", ""))


def links(doc, base, rel):
    return [resolve(base, link["href"]) for link in doc.get("links", []) if link.get("rel") == rel]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--workers", type=int, default=128)
    args = parser.parse_args()

    # Catalogs and collections, level by level. Each is one read, and each level
    # can only be fetched once the one above it has been.
    documents = 0
    # The title an item is searched by is its outermost collection's: in Maxar's
    # catalog that is the event ("Japan Earthquake Jan 2024"), and the
    # collections nested under it are named after acquisition IDs.
    items = []  # (item url, the title of the outermost collection)
    level = [(args.root, None)]
    with cf.ThreadPoolExecutor(args.workers) as pool:
        while level:
            fetched = list(pool.map(lambda x: (x, get(x[0])), level))
            documents += len(fetched)
            level = []
            for (url, title), doc in fetched:
                if title is None and doc.get("type") == "Collection":
                    title = doc.get("title") or doc.get("id")
                level += [(child, title) for child in links(doc, url, "child")]
                items += [(item, title) for item in links(doc, url, "item")]
    documents += len(items)
    print(f"{documents:,} documents to read, {len(items):,} of them items", file=sys.stderr)

    done = set()
    if os.path.exists(args.out):
        with open(args.out, encoding="utf-8") as f:
            done = {json.loads(line)["url"] for line in f if line.strip()}
    todo = [x for x in items if x[0] not in done]

    def one(x):
        url, title = x
        item = get(url)
        p = item.get("properties", {})
        return {
            "id": item["id"],
            "url": url,
            "title": title,
            "bbox": item["bbox"],
            "datetime": p.get("datetime"),
            "platform": p.get("platform"),
            "gsd": p.get("gsd"),
            "clouds": p.get("tile:clouds_percent", p.get("eo:cloud_cover")),
        }

    with open(args.out, "a", encoding="utf-8") as out, cf.ThreadPoolExecutor(args.workers) as pool:
        for n, record in enumerate(pool.map(one, todo), 1):
            out.write(json.dumps(record, ensure_ascii=False) + "\n")
            if n % 5000 == 0:
                print(f"{n:,} of {len(todo):,}", file=sys.stderr)

    with open(args.out + ".documents", "w") as f:
        f.write(f"{documents}\n")


if __name__ == "__main__":
    main()
