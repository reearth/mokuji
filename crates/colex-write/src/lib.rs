//! Deterministic writer for the COLex format.
//!
//! The writer is deliberately the last part of the reference implementation to
//! be built. Writing it first tends to produce a reader that can only read
//! what that writer happens to emit, which makes the implementation the
//! specification. The reader is developed against hand-built fixtures instead,
//! and the writer is then required to reproduce them.
//!
//! Two properties are required of it by `SPEC.md` §3.4 and §7:
//!
//! - Given identical input and identical parameters, it produces byte-
//!   identical output. Provenance only means something if the derivation can
//!   be re-run and compared.
//! - Its input is a sorted stream. Sorting is the caller's responsibility, so
//!   that indexing a corpus larger than memory does not require the writer to
//!   own an external sort.

// The builder lands with the term dictionary and postings encoders; this crate
// currently exists to fix the dependency edges and the contract above.
