//! `mokuji complete` - what someone has typed so far, and the documents to
//! show for it.
//!
//! `prefix` lists the terms a prefix matches, in dictionary order. This lists
//! documents instead, first those the file ranks first: from the completions
//! section when the prefix has an entry there, which costs no read before the
//! payload, and otherwise from every term the prefix matches.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{block_on::block_on, CountingReader, FileReader, Index, RangeReader};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// Completes `typed` against the index at `path`, reporting what it cost.
pub fn run(path: &Path, typed: &str, limit: usize) -> Result<(), Error> {
    let reader = CountingReader::new(FileReader::open(path)?);
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();

    let meta = Meta::parse(index.meta())?;
    let recorded = meta.tokenizer.id.clone();
    let normalized = match Tokenizer::parse(&recorded) {
        Some(tokenizer) => tokenizer.normalize_query(typed),
        None => typed.to_string(),
    };
    reader.reset();

    let (docs, from_table) = block_on(index.complete(normalized.as_bytes(), limit))?;
    let before_payload = reader.reads();
    let payloads = block_on(index.payload_span(&docs))?;

    for (doc, payload) in docs.iter().zip(&payloads) {
        println!("{doc}\t{payload}");
    }

    eprintln!();
    let source = if from_table {
        "the completions section"
    } else {
        match index.completions() {
            Some(c) if c.covers() => "the dictionary; the prefix has no entry, so it is cheap",
            Some(_) => "the dictionary; the prefix has no entry",
            None => "the dictionary; the file has no completions",
        }
    };
    eprintln!("{} document(s), from {source}", docs.len());
    eprintln!("open:     {open_reads} read(s)");
    eprintln!(
        "query:    {} read(s) total - {before_payload} before the payload, {} payload",
        reader.reads(),
        reader.reads() - before_payload
    );
    eprintln!(
        "bytes:    {} of {}",
        reader.stats().bytes,
        reader.file_len()
    );
    // Results derived from a licensed database carry its credit with them;
    // the file says what that is, so the tool says it too.
    if let Some(attribution) = &meta.attribution {
        eprintln!("data:     {attribution}");
    }
    Ok(())
}
