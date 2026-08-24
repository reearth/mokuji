//! On-disk layout for the COLex format.
//!
//! This crate is the single place where the byte layout described in `SPEC.md`
//! is written down. It has no dependencies and performs no I/O: it defines the
//! constants, the fixed-size structures, and their encoding, so that every
//! other crate in the workspace agrees on what a COLex file is.
//!
//! Anything that requires fetching bytes belongs in `colex-read`; anything that
//! requires producing them belongs in `colex-write`.

mod codec;
mod error;
mod header;
mod section;

pub use codec::{PostingsCodec, SectionCodec};
pub use error::{Error, Result};
pub use header::{Flags, Header, HEADER_LEN, MAGIC, SPEC_MAJOR, SPEC_MINOR};
pub use section::parse_section_table;
pub use section::{SectionEntry, SectionId, SECTION_ENTRY_LEN};
