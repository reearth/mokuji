# The landing page

`index.html`, `style.css`, `app.js` and `figures.js`. `build.py` writes the
published site: the page at `/` in English, and at `/ja/` in Japanese.
`mokuji` is a link to `../js/dist`, copied in as files. Locally:

```sh
python3 site/build.py _site && python3 -m http.server -d _site
```

The images in `img/` are rendered from `brand/` by `brand/render.sh`, which
needs agent-browser; run it after changing `img/favicon.svg` or
`brand/og.html`, and commit the PNGs.

## What it is for

Someone who builds search, maps or AI tools leaves wanting to try Mokuji. The
claim is that search needs no server; the page makes it by being static and
searching anyway.

## Rules

- **The search is real.** Every query on the page reads `.mokuji` files from
  Re:Earth Places (`places.reearth.land`) with range requests, and the page
  shows what it cost: the reads, the bytes, the time, and the requests
  themselves. Nothing on the page is a recording or a mock.
- **A file is drawn as a book's fore-edge.** Fine vertical lines are its
  pages, to scale; vermilion is what a query read; grey is what opening it read
  (the table of contents, kept). That is the only visual metaphor. Do not add a
  second.
- **The figures in the examples are drawn from the mechanism** they show
  (`figures.js`): the geohash cells are computed, the bigrams cut, the lists
  sorted. Only the point clouds and postings are seeded noise. A figure shows
  what a term is in that kind of search; it does not decorate.
- **One accent.** Vermilion (`--shu`) marks what was read, the selected tab,
  and the chapter numbers. Nothing else.
- **Numbers are measured.** The costs in the list of examples are the ones the
  repository's READMEs report; change them there first. No cost comparisons
  that were not measured.
- **Both languages are in the page,** in `lang="en"` and `lang="ja"` pairs,
  and the address decides which is shown: `/` English, `/ja/` Japanese. `/`
  sends on to `/ja/` a browser that prefers Japanese, unless its visitor chose
  English there; a link to `/ja/` is always Japanese. What a crawler reads
  without running the page (title, description, link preview) is replaced
  for `/ja/` by `build.py`. Japanese visitors see Japan's addresses and postal
  codes first; others, places worldwide.
- **Light and dark, phone and desktop,** each checked by looking before a
  change is called done.
- Hover and open transitions are 0.2–0.3 s.

## Shared with other code

How `app.js` asks the Japanese files mirrors reearth-places `src/jp.js` and
`src/reverse.js`. If those change how a file is read, change this too.
