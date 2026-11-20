//! The Mokuji metadata section (`SPEC.md` §6).
//!
//! Metadata is what makes an index a *derivative* rather than a blob: it names
//! the source it came from, by content hash, and the parameters it was built
//! with. Together those are required to be sufficient to rebuild the file
//! byte-for-byte (`SPEC.md` §3.4), which is what makes the claim checkable
//! instead of merely asserted.
//!
//! The encoding is canonical CBOR, written here rather than taken from a
//! library. Determinism is the point: a general CBOR encoder is free to order
//! map keys however it likes, and two encoders that disagree would produce two
//! valid files for one input — exactly what §3.4 rules out. The subset needed
//! is small enough that owning it costs less than constraining someone else's.

mod cbor;
mod section;

pub use cbor::{decode, encode, map as cbor_map, Value};
pub use section::{is_rfc3339, ContentHash, Generator, Meta, Source, Tokenizer};
