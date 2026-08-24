# Conformance suite

The suite is part of the specification, not part of the implementation.

Each case in `cases.toml` names a fixture, a query, the expected result, **and
the expected number of range reads**. The read count is checked the same way
the result is, because the I/O bound in `SPEC.md` §4 is the property the format
exists to provide, and a bound that is not tested is a bound that drifts.

```
fixtures/build_tiny.py    builds the fixture, independently of colex-write
fixtures/tiny.corpus.txt  the source the fixture is derived from
fixtures/tiny.colex       the fixture
fixtures/tiny.hex         its annotated hex dump
cases.toml                query, expected result, expected read count
```

## Why the fixture is not built by the writer

If `colex-write` emitted the fixtures, a mistake in the writer would be
mirrored by the reader developed against it, and the two would agree with each
other rather than with the specification. `build_tiny.py` is a second
implementation, in another language, written from the prose alone. Where the
two disagree, one of them is wrong about the specification — which is the whole
point of having a specification.

It doubles as the first evidence that `SPEC.md` is implementable by someone who
has only the document.

## Regenerating

```sh
python3 fixtures/build_tiny.py           # rebuild
python3 fixtures/build_tiny.py --check   # verify, as CI does
```

The builder checks every case in `cases.toml` before writing, so the fixture
and the expectations cannot drift apart unnoticed.
