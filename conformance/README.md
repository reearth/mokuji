# Conformance suite

The suite is part of the specification, not part of the implementation.

Each case in `cases.toml` names a fixture, a query, the expected result, **and
the expected number of range reads**. The read count is checked the same way
the result is, because the I/O bound in `SPEC.md` §4 is the property the format
exists to provide, and a bound that is not tested is a bound that drifts.

## The fixtures

| | shape |
|---|---|
| `tiny` | A document collection: several terms per document, postings stored, payload opaque, every section raw |
| `dict` | A dictionary: one term per record, postings implied throughout, payload as delta-coded integers, every section raw |
| `dict-deflate` | `dict` again, compressed |
| `tiny-extra` | `tiny` again, with two sections this version does not define — one in the specification's range, one in the application's |
| `names-delta` | A segment added to `names` later, for the collection below |
| `names` | A gazetteer for completion: each name one term under `name-list`, and a completions section with an entry for every prefix at least two terms begin with — including the byte prefixes that stop inside a character |

```
fixtures/build_fixtures.py   builds tiny, tiny-extra, dict, names and names-delta,
                             independently of the Rust writer, and collection.json
fixtures/collection.json     a manifest (MANIFEST.md): names, then names-delta,
                             with one document of names deleted; names-delta
                             states a region, Japan, and names states none
fixtures/*.corpus.txt        the sources they are derived from
fixtures/*.mokuji             the fixtures
fixtures/*.hex               annotated hex dumps
cases.toml                   query, expected result, expected read count; [[case]]
                             for term queries, [[completion]] for prefixes,
                             [[collection]] for prefixes across a collection
```

`tiny` and `dict` are stored raw so that they can be read against the
specification by hand — and so that two writers can be compared byte for byte,
which compression makes impossible (`SPEC.md` §3.4). `dict-deflate` exists so
that decompression is exercised on the read side, where interoperability
actually matters; it is written by the Rust writer (`mokuji::write`) and kept current by a test.

## Who answers to these cases

| implementation | how |
|---|---|
| `mokuji::read` | `cargo test --test read_conformance` |
| `mokuji::manifest` | `cargo test --test manifest_conformance` — the collection cases |
| `mokuji` (`MokujiIndex`) | `cargo test --test search` — the same questions as the TypeScript reader's tests |
| `build_fixtures.py` | checks every case before writing a fixture |
| `js/` (`mokuji` on npm) | `cd js && npm test` |
| `python/` (`mokuji`) | `python3 -m unittest discover -s python/tests` — every case, read counts included, and a comparison with the TypeScript reader |

## Why the fixtures are not built by the writer

If the Rust writer emitted them, a mistake in the writer would be mirrored by the
reader developed against it, and the two would agree with each other rather than
with the specification. `build_fixtures.py` is a second implementation, in
another language, written from the prose alone. Where the two disagree, one of
them is wrong about the specification — which is the whole point of having one.

## Regenerating

```sh
python3 fixtures/build_fixtures.py           # rebuild
python3 fixtures/build_fixtures.py --check   # verify, as CI does
MOKUJI_REGENERATE=1 cargo test                # rewrite dict-deflate.mokuji
```
