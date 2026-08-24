//! `colex build` - turn a corpus into an index, recording where it came from.

use std::path::Path;

use colex_meta::{
    cbor_map, ContentHash, Generator, Meta, Source, Tokenizer as MetaTokenizer, Value,
};
use colex_write::{Builder, Options};
use sha2::{Digest, Sha256};

use crate::corpus::{self, Tokenizer};
use crate::info::Error;

/// Builds `input` into `output`.
///
/// `origins` are further files to record as sources. An index is often derived
/// from something the builder never sees — here, a corpus converted from a
/// published register — and recording only the file it read would make the
/// provenance true but useless: it would name an intermediate nobody keeps.
pub fn run(
    input: &Path,
    output: &Path,
    origins: &[&Path],
    options: Options,
    tokenizer: Tokenizer,
    generation: u64,
) -> Result<(), Error> {
    let raw = std::fs::read(input)?;
    let text = std::str::from_utf8(&raw).map_err(|_| Error::Corpus("input is not UTF-8".into()))?;
    let corpus = corpus::parse(text, tokenizer).map_err(Error::Corpus)?;

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

    let mut sources = vec![Source {
        content_hash: ContentHash::sha2_256(&digest),
        uri: Some(input.display().to_string()),
    }];
    for origin in origins {
        sources.push(Source {
            content_hash: ContentHash::sha2_256(&hash_file(origin)?),
            uri: Some(origin.display().to_string()),
        });
    }

    let meta = Meta {
        generation,
        sources,
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
        tokenizer: MetaTokenizer {
            id: tokenizer.id().into(),
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
    for source in &meta.sources {
        println!("derived from {}", source.content_hash);
        if let Some(uri) = &source.uri {
            println!("             {uri}");
        }
    }
    Ok(())
}

/// Hashes a file without holding it in memory, since a register is large.
fn hash_file(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize().to_vec());
        }
        hasher.update(&buffer[..read]);
    }
}
