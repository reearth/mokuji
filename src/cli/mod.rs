//! The `mokuji` command, as a library.
//!
//! The commands live here rather than in the binary so that they can be tested
//! against the conformance fixtures — in particular so that a fixture built
//! from prose by another implementation can be reproduced through the same
//! tokenizer and payload encoding the command line uses.

pub mod build;
pub mod collection;
pub mod compact;
pub mod complete;
pub mod corpus;
pub mod dump;
pub mod info;
pub mod payload;
pub mod prefix;
pub mod search;
pub mod verify;
