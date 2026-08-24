//! Deterministic writer for the Mokuji format.
//!
//! The writer is the last part of the reference implementation to be built.
//! Writing it first tends to produce a reader that can only read what that
//! writer happens to emit, which quietly makes the implementation the
//! specification. The reader was developed against a fixture built from the
//! prose instead, and this writer is required to reproduce it byte for byte.
//!
//! Two properties are required by `SPEC.md` §3.4 and §7:
//!
//! - Given identical input and identical parameters, output is byte-identical.
//!   Provenance only means something if the derivation can be re-run and
//!   compared.
//! - Input is a sorted stream. Sorting stays with the caller, so that indexing
//!   a corpus larger than memory does not require the writer to own an
//!   external sort.

mod builder;

pub use builder::{Builder, Column, Options, PayloadEncoding};
