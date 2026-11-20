//! Integer and string codecs used by the Mokuji format.
//!
//! These are the primitives the section encoders are built from: variable
//! length integers here, and, as the reader takes shape, delta plus
//! bit-packing for postings and front-coding for term blocks.
//!
//! Everything in this crate operates on byte slices already in memory. It has
//! no notion of sections, offsets, or I/O.

pub mod bitpack;
pub mod block;
pub mod front_coded;
pub mod varint;
