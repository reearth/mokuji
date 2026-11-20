// Package mokuji reads Mokuji files a range at a time, from a path, a URL or
// any source of bytes, and writes them.
//
// Mokuji is a single-file layout for an inverted index, designed to be
// published as a static object and queried over HTTP range requests (see
// SPEC.md and MANIFEST.md in the repository). This package is a reader of it,
// beside the Rust crate and the TypeScript and Python readers, whose API it
// follows with Go names. It answers to the same conformance cases, read
// counts included.
//
// Open a file with OpenFile, OpenURL, or Open over any Source, and query it
// with Lookup, Complete or Search. A query goes through the tokenizer the
// file records (TOKENIZERS.md) before it is looked up. Every query is
// accompanied by Index.Stats, the range requests and bytes it cost.
//
// A collection, a manifest over several files, is opened with
// OpenCollection.
//
// Builder writes a file, from documents and a sorted stream of postings,
// with metadata composed as a Metadata.
package mokuji
