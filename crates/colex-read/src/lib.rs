//! Bounded range-read access to a COLex file.
//!
//! The central abstraction is [`RangeReader`]: a source that can return an
//! arbitrary byte range. A local file, an object in a bucket, and an HTTP
//! endpoint that honours `Range` all satisfy it, which is what lets the same
//! reader run in a browser and on a server.
//!
//! [`CountingReader`] is the reason this crate exists in the shape it does.
//! The I/O bound in `SPEC.md` §4 is the substance of the format, so it is
//! tested the same way correctness is: every conformance case asserts the
//! number of range reads performed, not only the result returned.

pub mod block_on;
mod counting;
mod index;
pub mod postings;
mod reader;
mod slice;
pub mod term;

pub use counting::{CountingReader, ReadStats};
pub use index::{Index, Match, OpenOptions, Payload};
pub use postings::Postings;
pub use reader::RangeReader;
pub use slice::SliceReader;
pub use term::{TermEntry, TermIndex};
