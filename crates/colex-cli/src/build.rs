//! `colex build` - turn a corpus into an index, recording where it came from.

use std::path::Path;

use colex_meta::{cbor_map, ContentHash, Generator, Meta, Source, Tokenizer, Value};
use colex_write::{Builder, Options};
use sha2::{Digest, Sha256};

use crate::corpus::{self, TOKENIZER_ID};
use crate::info::Error;

/// Builds `input` into `output`.
pub fn run(input: &Path, output: &Path, options: Options, generation: u64) -> Result<(), Error> {
    let raw = std::fs::read(input)?;
    let text = std::str::from_utf8(&raw).map_err(|_| Error::Corpus("input is not UTF-8".into()))?;
    let corpus = corpus::parse(text).map_err(Error::Corpus)?;

    // Hash the source bytes as they were read, not the parsed corpus. The claim
    // being recorded is "this index was derived from that file", and it is only
    // checkable against the file itself.
    let digest = Sha256::digest(&raw);

    let mut builder = Builder::new(options)?;
    for payload in &corpus.payloads {
        builder.add_document(payload.clone());
    }
    for (term, docs) in &corpus.postings {
        for (&doc, &freq) in docs {
            builder.add_posting(term, doc, freq)?;
        }
    }

    let meta = Meta {
        generation,
        sources: vec![Source {
            content_hash: ContentHash::sha2_256(&digest),
            uri: Some(input.display().to_string()),
        }],
        generator: Generator {
            name: "colex".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            // Everything the writer branches on. SPEC.md §3.4 requires these to
            // be sufficient, with the source hash, to rebuild the file.
            params: cbor_map([
                (
                    "terms_per_block",
                    Value::Uint(options.terms_per_block as u64),
                ),
                (
                    "docs_per_payload_block",
                    Value::Uint(u64::from(options.docs_per_payload_block)),
                ),
                ("has_freqs", Value::Uint(u64::from(options.has_freqs))),
            ]),
        },
        tokenizer: Tokenizer {
            id: TOKENIZER_ID.into(),
            params: cbor_map([]),
        },
        attrs: None,
    };

    let bytes = builder.finish(&meta.encode())?;
    std::fs::write(output, &bytes)?;

    println!("{} bytes", bytes.len());
    println!(
        "{} documents, {} terms",
        corpus.payloads.len(),
        corpus.postings.len()
    );
    println!("derived from {}", ContentHash::sha2_256(&digest));
    Ok(())
}
