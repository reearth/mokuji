//! A Mokuji collection: immutable segments, ordered by a manifest, with deletes.
//!
//! A Mokuji file cannot change (`SPEC.md` §3.3). A collection is how an index
//! that has to change is built out of files that cannot: a base, the segments
//! added since, and the documents deleted from any of them, listed in a small
//! manifest that is the one thing ever rewritten. See `MANIFEST.md`.
//!
//! A delete names a document by the ID it has in its own segment. Segments are
//! immutable, so that ID never changes, and neither a key column nor a
//! reference from one segment into another is needed to say which document is
//! gone. An update is a delete in an older segment and an add in a newer one.

mod collection;
mod document;

pub use collection::{Collection, Completion, Hit};
pub use document::{Manifest, Region, Segment, VERSION};

/// What can go wrong reading a manifest or opening a collection.
#[derive(Debug)]
pub enum Error {
    /// The manifest is not JSON, or not the shape `MANIFEST.md` gives.
    Manifest(String),
    /// A segment does not agree with what the manifest says of it, or with the
    /// segments beside it.
    Segment {
        /// The segment's path, as the manifest gives it.
        path: String,
        /// What is wrong.
        detail: String,
    },
    /// A segment file is malformed.
    Format(crate::format::Error),
    /// A question named a segment past the last one.
    NoSegment(usize),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Manifest(detail) => write!(f, "manifest: {detail}"),
            Error::Segment { path, detail } => write!(f, "segment {path}: {detail}"),
            Error::Format(err) => write!(f, "{err}"),
            Error::NoSegment(n) => write!(f, "the collection has no segment {n}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::format::Error> for Error {
    fn from(err: crate::format::Error) -> Self {
        Error::Format(err)
    }
}

/// Shorthand for this crate's results.
pub type Result<T> = std::result::Result<T, Error>;
