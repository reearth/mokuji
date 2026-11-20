# Fuzzing

A Mokuji file is untrusted input. The whole proposition is that a stranger's file
is fetched from object storage and read in a page, so every offset, length, and
count in it is attacker-controlled, and `SPEC.md` §8 requires a reader to fail
cleanly rather than crash on any of it.

```sh
cargo +nightly fuzz run read
cargo +nightly fuzz run meta
```

Nightly is required by `cargo-fuzz`, so these do not run in CI.
`tests/read_hostile.rs` does, and covers the same ground
exhaustively but shallowly: every byte of a fixture flipped, zeroed, maxed, and
truncated at every length.

## What it has found

Two defects, both on their first run, both in code that had passed every other
test:

- A subtraction that underflowed when an offset table pointed a block before the
  section holding it.
- Allocations sized from counts declared in the file. A four-byte block count
  could ask for gigabytes before a single byte of the blocks arrived. Reserving
  a bounded amount and letting the vector grow costs nothing and removes the
  whole class.

The second is the more instructive: `SPEC.md` §8 already said not to do it, in
writing, and the code did it anyway in four places.
