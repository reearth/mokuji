# Conformance suite

The suite is part of the specification, not part of the implementation.

Each case names a fixture, a query, the expected result, **and the expected
number of range reads**. The read count is checked the same way the result is,
because the I/O bound in `SPEC.md` §4 is the property the format exists to
provide, and a bound that is not tested is a bound that drifts.

```
fixtures/   byte-fixed .colex files, small enough to build by hand
cases.toml  query, expected result, expected read count
```

Fixtures are built by hand rather than emitted by `colex-write`, so that a
writer bug cannot make the reader agree with it.
