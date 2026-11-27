# mokuji (Go)

Read [Mokuji](../README.md) files a range at a time, from a path, a URL or any
source of bytes, and write them. The standard library and `golang.org/x/text`,
for the Unicode normalization and case mapping the tokenizers need.

It is a reader of [SPEC.md](../SPEC.md) beside the Rust crate and the
[TypeScript](../js) and [Python](../python) readers, whose API it follows with
Go names. It answers to the same conformance cases, read counts included.

## Use

```go
import mokuji "github.com/reearth/mokuji/go"

// One file. A URL is read with Range requests.
index, err := mokuji.OpenURL(ctx, "https://example.com/places.mokuji")
done, err := index.Complete(ctx, "東京タ", 10) // done.Docs, done.Payloads, done.FromTable

// A collection: a manifest over several files (MANIFEST.md), restricted to a
// place so that only the files there are opened.
collection, err := mokuji.OpenCollection(ctx, "https://example.com/world.json",
	&mokuji.CollectionOptions{Countries: []string{"JP"}})
found, err := collection.Complete(ctx, "tokyo", 10)
```

The query goes through the tokenizer the file records (`TOKENIZERS.md`)
before it is looked up, so `一丁目` finds `1丁目` in a `ja-address` index.

`Search` ranks by BM25, the same ranking as `mokuji search` with the same
options, and splits the query the way the file's tokenizer does: into
bigrams, for a `ja-address-bigram` index.

```go
fuzzy, err := mokuji.OpenURL(ctx, "https://example.com/abr-fuzzy.mokuji")
hits, err := fuzzy.Search(ctx, "和光市", mokuji.Boost("municipality"))
// hits.Results[0].Payload == "埼玉県和光市|139.605693,35.781104"

// Typed wrong: any of its bigrams, the six rarest, without length normalization.
fuzzy.Search(ctx, "千代田区丸乃内", mokuji.Any(), mokuji.Probe(6), mokuji.LengthNorm(0))
```

`Filter` keeps only the documents a function accepts, before ranking and
before any payload is read. Asking for more results and discarding some costs
the payload of every one discarded, and the payload is most of what a query
reads. Numbering the corpus by kind turns a filter by kind into an ID range:

```go
// Documents 0 to 1,199 are one kind, the rest another.
spec.Search(ctx, "LOD2", mokuji.Filter(func(doc int) bool { return doc < 1200 }))
```

A file held in memory (`BytesSource`) pays for a payload block in
decompression rather than in requests, so it is served better by smaller
blocks than the default 64 documents (`DocsPerPayloadBlock`). On the 2,521
sections of the PLATEAU specifications, with each section's text in its
payload, 16 made the file 8% larger and a filtered search about twice as
fast.

`Phrase` requires the terms at consecutive positions, in the order typed,
from a file that carries positions. It is tested after `Filter`: each term's
positions are read whole, one request per term, once any document survives
the match and the filter, and nothing is read when none does. `Rank` is the
half of a search before any
payload: the documents a set of terms matches, best first, for a caller that
decides itself which payloads to fetch.

```go
tiny.Search(ctx, "tokyo chuo", mokuji.Phrase())          // not "chuo tokyo"
ranking, err := tiny.Rank(ctx, tiny.Split("tokyo chuo")) // ranking.Ranked, no payload read
```

The scores agree with the other readers' to about 1e-12, not to the last
bit, since logarithms differ in the last place between platforms; two
documents whose scores differ by less than that may come out in either order
between them.

`index.Stats()` is the range requests and bytes queries have cost since
opening or `ResetStats`, and `index.OnRequest` sees every request.
`index.Attribution` and `index.License` are the credit the file states, and
`index.Generator` the writer and every build parameter it records, including
any a pipeline added to check when it opens the file. `index.Generation`,
`index.TokenizerParams` and `index.Attrs` are the rest of the metadata a
reader acts on, and `index.Meta` all of it, decoded; `DecodeCBOR` decodes
CBOR the same way. `Normalize(id, text)` is what a query goes through for a
tokenizer, by its identifier, without a file.
Queries may run from several goroutines at once.

A refusal is a `*FormatError` for a file or a manifest that cannot be read, a
`*NoColumnError` for a boost column the file lacks, and a `*BuildError` for
what a `Builder` will not write; `Locate` returns `ErrNotCoverings` and
`ErrRings`.

### A collection

`Complete` and `Candidates` are a collection's completion: the first
documents in collection order, or every segment's own first for a caller
that ranks across them. `Search` is the live documents holding every term,
in collection order, unranked, since scores from different segments are not
comparable. `LiveIn` is one segment's first live documents for a prefix
already normalized.

```go
found, err := collection.Search(ctx, "Tokyo", 10) // found.Hits, found.Payloads, found.Matched
```

`ParseManifest` reads a manifest with the checks a collection opens with,
`Within` restricts it to some countries, and `JSON` writes it as the Rust
crate does, byte for byte, so that one collection is written down one way.

### An address typed past its key

`LookupPrefix` gives the terms a prefix begins, and, when none does, the
longest term one of the readings it is given begins with, marked
`MatchWithin`: the town an address typed down to its lot number is in, when
the file keys towns. Every prefix of the readings is looked up at once.

```go
q := index.Normalize("東京都千代田区丸の内1丁目9番1号")
hits, err := mokuji.LookupPrefix(ctx, index, q, 10, []string{q})
// [{Term: "東京都千代田区丸の内1丁目", Doc, Payload, Match: "within"}]
```

### Which polygon holds a point

`Locate` answers from a file of geohash coverings, as `demo/reverse` builds:
each polygon a document keyed by the cells that cover it, its label and rings
as payload, and the file's `attrs` saying how (`geohash-precisions`,
`geometry=delta-int-1e…`, and `payload-fields` for a label cut at U+001F).
It looks up the point's cell at every precision at once and tests the
candidates' rings even-odd.

```go
found, err := mokuji.Locate(ctx, index, 35.6812, 139.7671, nil)
// found.Inside: [{Doc, Label, Fields: {"KEY_CODE": …}, Rings: [[[lon, lat], …]]}]
```

`Geohash`, `DecodeRings`, `Contains`, `PrecisionsOf`, `ScaleOf` and
`FieldsOf` are the parts it is made of.

### Reading a file on hand

`OpenFile` reads with positional reads: a file of gigabytes is searched
without being loaded, and costs what a query touches.

```go
postal, err := mokuji.OpenFile(ctx, "postal.mokuji")
defer func() { _ = postal.Close() }()
postal.Lookup(ctx, "1000005", 10)
```

`OpenWith` with `OpenOptions{ResidentDictionary: true}` holds the whole term
dictionary: one more request at open and its stored size in memory, and then
a term lookup costs no request at all.

### Reading from something else

`Open` takes any `Source`: an object store client, a cache in front of one.
`ReaderAtSource` adapts an `io.ReaderAt`, and `BytesSource` a file in memory.
A source that implements `Sizer` lets the reader check offsets against the
file's length.

```go
type bucket struct{ client *s3.Client }

func (b bucket) ReadRange(ctx context.Context, offset, length int64) ([]byte, error) {
	out, err := b.client.GetObject(ctx, &s3.GetObjectInput{
		Bucket: aws.String("b"), Key: aws.String("places.mokuji"),
		Range:  aws.String(fmt.Sprintf("bytes=%d-%d", offset, offset+length-1)),
	})
	if err != nil {
		return nil, err
	}
	defer func() { _ = out.Body.Close() }()
	return io.ReadAll(out.Body)
}

index, err := mokuji.Open(ctx, bucket{client})
```

For another HTTP client, `Open` an `&mokuji.HTTPSource{URL: url, Client: client}`.

## Write

`Builder` is the Rust crate's `mokuji::write`. Documents are added first, in
ID order; postings then, terms in byte order and each term's documents
ascending. The writer checks the order and owns no sort, so that a corpus
larger than memory is sorted outside it. `Split` turns a document's text into
terms as a tokenizer from `TOKENIZERS.md` does.

```go
options := mokuji.DefaultWriteOptions()
builder, err := mokuji.NewBuilder(options)
doc, err := builder.AddDocument([]byte("35.671,139.772"))
// ... every document, then every posting, in order
err = builder.AddPosting([]byte("chuo"), doc, 1)

meta, err := (&mokuji.Metadata{
	Generation:       1,
	Sources:          []mokuji.Provenance{{ContentHash: "sha2-256:…", URI: "places.tsv"}},
	GeneratorName:    mokuji.GeneratorName,
	GeneratorVersion: mokuji.Version,
	GeneratorParams:  options.Params(),
	TokenizerID:      "space-split",
}).Encode()
file, err := builder.Finish(meta)
```

The metadata is the caller's to compose: it names the source the index was
derived from, which the writer never sees (SPEC.md §6). `ExampleBuilder` in
`example_test.go` is a whole build.

Stored raw (`Compress: false`), a file is the Rust writer's byte for byte, and
the tests rebuild the conformance fixtures to show it. Compressed, it is not:
Go's deflate is not the Rust crate's, and no specification of deflate fixes
which of the valid encodings a compressor emits. Such a file says the same
thing, every reader reads it, and it is reproducible by this writer, which is
what SPEC.md §3.4 asks; `generator.name` is `mokuji-go` so that a rebuild is
checked against the writer that made it. Go's deflate may change between Go
releases, so a compressed rebuild is checked with the same Go too.

## Install

```sh
go get github.com/reearth/mokuji/go
```

The module is versioned by `go/v*` tags. The format and this API may change
before 1.0, so pin the version.

## Develop

From this directory:

```sh
go test ./...
```

The tests run the conformance cases from `../conformance/`, rebuild the raw
fixtures byte for byte, and, when `python3` is present, ask the Python reader
the same questions and compare the answers, scores and costs included, the
tokenizers' output on a few hundred strings, and its reading of the fixtures
as this writer compresses them.

## Where it differs from the TypeScript reader

The same places as the Python reader:

- A term that sorts before the first block is absent without a read
  (SPEC.md §5.3.2), as it is in the Rust reader; so is a prefix or a range
  that nothing in the dictionary can begin with or fall in.
- It checks what SPEC.md says a reader must: varints of at most ten bytes, a
  strictly increasing term index, offsets inside their sections, a postings
  run's count against its entry's `doc_freq`, and `HAS_POSITIONS` only with
  `HAS_FREQS`.
- The metadata decoder reads every CBOR type, so an unknown key holding a
  boolean, a negative number or bytes does not stop a file opening.
- `ja-address` strips `White_Space`, as the Rust tokenizer does; JavaScript's
  `trim` differs on U+0085 and U+FEFF.
- Requests are made one after another, not at once.
- A manifest is parsed with the checks the Rust crate makes, with three
  differences Go's JSON decoder brings: a key given twice is taken at its last
  value rather than refused, a number above 2^63 − 1 is refused though it fits
  the format's unsigned integers, and a lone surrogate escape in a string is
  read as U+FFFD rather than refused.
