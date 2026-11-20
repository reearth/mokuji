# The landing page

`index.html`, `style.css` and `app.js`, served as they are. `mokuji` is a link
to `../js/dist`; the Pages workflow copies it in as files. Locally:

```sh
cd site && python3 -m http.server
```

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
  and one is shown. Japanese visitors see Japan's addresses and postal codes
  first; others, places worldwide.
- **Light and dark, phone and desktop,** each checked by looking before a
  change is called done.
- Hover and open transitions are 0.2–0.3 s.

## Shared with other code

How `app.js` asks the Japanese files mirrors reearth-places `src/jp.js` and
`src/reverse.js`. If those change how a file is read, change this too.
