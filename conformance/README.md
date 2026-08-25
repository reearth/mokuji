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

```
fixtures/build_fixtures.py   builds tiny, tiny-extra and dict, independently of mokuji-write
fixtures/*.corpus.txt        the sources they are derived from
fixtures/*.mokuji             the fixtures
fixtures/*.hex               annotated hex dumps
cases.toml                   query, expected result, expected read count
```

`tiny` and `dict` are stored raw so that they can be read against the
specification by hand — and so that two writers can be compared byte for byte,
which compression makes impossible (`SPEC.md` §3.4). `dict-deflate` exists so
that decompression is exercised on the read side, where interoperability
actually matters; it is written by `mokuji-write` and kept current by a test.

## Who answers to these cases

| implementation | how |
|---|---|
| `mokuji-read` | `cargo test -p mokuji-read --test conformance` |
| `build_fixtures.py` | checks every case before writing a fixture |
| `demo/web/mokuji.js` | `node --test 'demo/web/*.test.js'` |

## Why the fixtures are not built by the writer

If `mokuji-write` emitted them, a mistake in the writer would be mirrored by the
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
