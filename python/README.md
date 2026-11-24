# mokuji (Python)

Read [Mokuji](../README.md) files a range at a time, from a path, a URL or any
source of bytes. Pure standard library: `zlib` for the required raw deflate,
`urllib` for range requests, nothing to install beside it.

It is the fourth implementation of [SPEC.md](../SPEC.md), after the Rust
crates, the Python fixture builder and the [TypeScript reader](../js), whose
API it follows with Python names. It answers to the same conformance cases,
read counts included.

> Installed from git rather than PyPI while the format is a draft (below).

## Use

```python
from mokuji import MokujiIndex, MokujiCollection

# One file. A URL makes the reader fetch ranges itself.
index = MokujiIndex.open("https://example.com/places.mokuji")
done = index.complete("東京タ", 10)  # .docs, .payloads, .from_table

# A collection: a manifest over several files (MANIFEST.md), restricted to a
# place so that only the files there are opened.
collection = MokujiCollection.open("https://example.com/world.json", countries=["JP"])
found = collection.complete("tokyo", 10)
```

The query goes through the tokenizer the file records (`TOKENIZERS.md`)
before it is looked up, so `一丁目` finds `1丁目` in a `ja-address` index.

`search` ranks by BM25, the same ranking as `mokuji search` with the same
options, and splits the query the way the file's tokenizer does — into bigrams,
for a `ja-address-bigram` index:

```python
fuzzy = MokujiIndex.open("https://example.com/abr-fuzzy.mokuji")
hits = fuzzy.search("和光市", boost="municipality")
# hits.results[0].payload == "埼玉県和光市|139.605693,35.781104"

# Typed wrong: any of its bigrams, the six rarest, without length normalization.
fuzzy.search("千代田区丸乃内", any=True, probe=6, length_norm=0)
```

`filter` keeps only the documents a function accepts, by document ID, before
ranking and before any payload is read, so that discarding a document costs no
payload. Numbered by kind, a corpus is filtered by kind with an ID range:
`filter=lambda doc: doc < 1200`.

The scores agree with `mokuji search`'s and the TypeScript reader's to about
1e-12, not to the last bit. JavaScript's `Math.log` is not correctly rounded,
and Python's is, so two documents whose scores differ by less than that may
come out in either order between the two.

Every result is accompanied by `index.stats` — the range requests and bytes it
cost since opening or `reset_stats()` — and `index.attribution` /
`index.license`, the credit the file states. `index.generator` is the writer
and every build parameter it records, including any a pipeline added to check
when it opens the file.

### Reading a file on hand

Give `open` a path, and it reads with positional reads: a file of gigabytes is
searched without being loaded, and costs what a query touches.

```python
with MokujiIndex.open("postal.mokuji") as postal:
    postal.lookup("1000005", 10)
```

### Reading from something else

Give `open` any object with `read(offset, length)` returning the bytes — an
object store client, a cache in front of one. A `size` attribute, if it has
one, lets the reader check offsets against the file's length.

```python
class Bucket:
    def read(self, offset, length):
        return s3.get_object(Bucket="b", Key="places.mokuji",
                             Range=f"bytes={offset}-{offset + length - 1}")["Body"].read()

index = MokujiIndex.open(Bucket())
```

## Write

`Builder` is the Rust crate's `mokuji::write`. Documents are added first, in
ID order; postings then, terms in byte order and each term's documents
ascending. The writer checks the order and owns no sort. `split` turns a
document's text into terms as a tokenizer from `TOKENIZERS.md` does.

```python
from mokuji import Builder, Metadata, Provenance, WriteOptions, split

options = WriteOptions()
builder = Builder(options)
doc = builder.add_document("35.671,139.772")
# ... every document, then every posting, in order
builder.add_posting("chuo", doc)

meta = Metadata(
    sources=[Provenance("sha2-256:…", "places.tsv", None, None)],
    generator_params=options.params(),
    tokenizer_id="space-split",
)
data = builder.finish(meta.encode())
```

The metadata is the caller's to compose: it names the source the index was
derived from, which the writer never sees (SPEC.md §6).

Stored raw (`compress=False`), a file is the Rust writer's byte for byte, and
the tests rebuild the conformance fixtures to show it. Compressed with zlib,
it is not, since no specification of deflate fixes which of the valid
encodings a compressor emits; it says the same thing, every reader reads it,
and this writer reproduces it, which is what SPEC.md §3.4 asks. Its
`generator.name` is `mokuji-python`, so that a rebuild is checked against the
writer that made it.

## Install

```sh
pip install mokuji
```

The format and this API may change before 1.0, so pin the version.

## Develop

From the repository root:

```sh
python3 -m unittest discover -s python/tests
```

The tests run the conformance cases from `conformance/`, and, when `node` and
a built `js/dist` are present, ask the TypeScript reader the same questions
and compare the answers.

## Where it differs from the TypeScript reader

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
