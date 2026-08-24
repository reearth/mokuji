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

    // Each phase is measured rather than assumed. An index whose entries carry
    // implied postings spends no read on them at all, and a breakdown that
    // asserted one would be reporting the code's expectations instead of what
    // the file cost.
    let matches = block_on(index.prefix(prefix, limit))?;
    let dictionary = reader.reads();

    let entries: Vec<_> = matches.iter().map(|(_, entry)| *entry).collect();
    let postings = block_on(index.postings_span(&entries))?;
    let postings_reads = reader.reads() - dictionary;

    // A term in a dictionary index names one document. Collecting the documents
    // first lets the payload reads share blocks, which for keys that sort
    // together — as an address hierarchy does — is most of them.
    let docs: Vec<u64> = postings
        .iter()
        .filter_map(|p| p.docs.first().copied())
        .collect();
    let payloads = block_on(index.payload_span(&docs))?;
    let payload_reads = reader.reads() - dictionary - postings_reads;

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
        "query:    {} read(s) total - {dictionary} dictionary, {postings_reads} postings, \
         {payload_reads} payload",
        reader.reads()
    );
    eprintln!(
        "bytes:    {} of {}",
        reader.stats().bytes,
        reader.file_len()
    );
    Ok(())
}
