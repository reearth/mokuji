//! `colex search` - intersect terms and report both results and cost.

use std::path::Path;

use colex_read::{block_on::block_on, CountingReader, Index, SliceReader};

use crate::info::Error;

pub fn run(path: &Path, terms: &[&str]) -> Result<(), Error> {
    if terms.is_empty() {
        return Err(Error::NoTerms);
    }

    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();
    // The resident set is charged to opening the file, not to the query
    // (SPEC.md §4). A long-lived reader pays it once.
    reader.reset();

    let query: Vec<&[u8]> = terms.iter().map(|t| t.as_bytes()).collect();
    let matches = block_on(index.search(&query))?;

    for m in &matches {
        match std::str::from_utf8(&m.payload) {
            Ok(text) => println!("{}\t{text}", m.doc),
            // Payload is opaque to the format, so it is not necessarily text.
            Err(_) => println!("{}\t<{} bytes>", m.doc, m.payload.len()),
        }
    }

    let bound = 2 * terms.len() + matches.len();
    eprintln!();
    eprintln!("{} match(es)", matches.len());
    eprintln!("open: {open_reads} read(s)");
    eprintln!("query: {} read(s), bound {bound}", reader.reads());
    Ok(())
}
