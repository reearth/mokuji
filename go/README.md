# mokuji (Go)

Read [Mokuji](../README.md) files a range at a time, from a path, a URL or any
source of bytes. The standard library and `golang.org/x/text`, for the
Unicode normalization and case mapping the tokenizers need.

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

The scores agree with the other readers' to about 1e-12, not to the last
bit, since logarithms differ in the last place between platforms; two
documents whose scores differ by less than that may come out in either order
between them.

`index.Stats()` is the range requests and bytes queries have cost since
opening or `ResetStats`, and `index.OnRequest` sees every request.
`index.Attribution` and `index.License` are the credit the file states.
Queries may run from several goroutines at once.

### Reading a file on hand

`OpenFile` reads with positional reads: a file of gigabytes is searched
without being loaded, and costs what a query touches.

```go
postal, err := mokuji.OpenFile(ctx, "postal.mokuji")
defer func() { _ = postal.Close() }()
postal.Lookup(ctx, "1000005", 10)
```

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

The tests run the conformance cases from `../conformance/`, and, when
`python3` is present, ask the Python reader the same questions and compare
the answers, scores and costs included, and the tokenizers' output on a few
hundred strings.

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
