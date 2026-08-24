# Measurement tools

Neither of these is part of the format or the reference implementation. They
exist to answer questions about it with numbers, and are kept so the numbers can
be re-taken rather than quoted.

Both read a corpus produced by `colex dump`, so they measure the index as it
actually is rather than a corpus as it was before indexing:

```sh
cargo run --release -q -p colex-cli -- dump tokyo.colex > tokyo-dump.tsv
```

## `fst-comparison`

Builds a finite state transducer over the same terms and reports its size
against the blocked front-coded dictionary this workspace writes.

```sh
cd fst-comparison && cargo run --release -- ../../tokyo-dump.tsv
```

Excluded from the workspace, because it pulls in a dependency the format does
not use — which is the point: it measures the road not taken.

## `payload-encodings.py`

Encodes the payload column several ways — as published, rounded, fixed point,
delta — and reports what each would cost per address after per-block deflate.

```sh
python3 payload-encodings.py tokyo-dump.tsv
```
