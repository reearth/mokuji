//! `colex prefix` - complete a prefix, and report what it cost.
//!
//! This is the query a hierarchical key exists for. An address is looked up
//! whole or by prefix, never by intersecting its parts, so this rather than
//! `search` is the shape forward geocoding takes.

use std::path::Path;

use colex_read::{block_on::block_on, CountingReader, Index, RangeReader, SliceReader};

use crate::info::Error;

pub fn run(path: &Path, prefix: &[u8], limit: usize) -> Result<(), Error> {
    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();
    reader.reset();

    let matches = block_on(index.prefix(prefix, limit))?;
    let dictionary_reads = reader.reads();

    let entries: Vec<_> = matches.iter().map(|(_, entry)| *entry).collect();
    let postings = block_on(index.postings_span(&entries))?;

    // A term in a dictionary index names one document. Collecting the documents
    // first lets the payload reads share blocks, which for keys that sort
    // together — as an address hierarchy does — is most of them.
    let docs: Vec<u64> = postings
        .iter()
        .filter_map(|p| p.docs.first().copied())
        .collect();
    let payloads = block_on(index.payload_span(&docs))?;

    for ((term, _), payload) in matches.iter().zip(&payloads) {
        let term = String::from_utf8_lossy(term);
        match std::str::from_utf8(payload) {
            Ok(text) => println!("{term}\t{text}"),
            Err(_) => println!("{term}\t<{} bytes>", payload.len()),
        }
    }

    eprintln!();
    eprintln!("{} match(es)", matches.len());
    eprintln!("open:     {open_reads} read(s)");
    eprintln!(
        "query:    {} read(s) total - {dictionary_reads} dictionary, {} postings, {} payload",
        reader.reads(),
        if entries.is_empty() { 0 } else { 1 },
        reader.reads() - dictionary_reads - usize::from(!entries.is_empty()) as u32,
    );
    eprintln!(
        "bytes:    {} of {}",
        reader.stats().bytes,
        reader.file_len()
    );
    Ok(())
}
