//! Browser bindings for reading COLex over HTTP range requests.
//!
//! This crate supplies a [`RangeReader`](colex_read::RangeReader) backed by
//! `fetch` with a `Range` header, plus the resident-set cache that keeps the
//! header, section table, metadata, and term index in memory across queries.
//!
//! It carries more weight than a binding layer usually does. The claim the
//! format makes is that a query costs a bounded number of requests, and the
//! browser network panel is where that claim is checked by someone who has not
//! read the specification. Two things therefore need proving early, because
//! both fail at the transport rather than in the format: that `Range` survives
//! the CDN in front of the bucket, and that CORS exposes `Content-Range` to
//! the page.

// Bindings land with the reader; this crate currently exists to fix the
// dependency edges and the contract above.
