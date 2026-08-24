//! `mokuji search` - intersect terms and report both results and cost.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{block_on::block_on, CountingReader, Index, SliceReader};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// Intersects `terms` against the index at `path`, reporting what it cost.
pub fn run(path: &Path, terms: &[&str]) -> Result<(), Error> {
    if terms.is_empty() {
        return Err(Error::NoTerms);
    }

    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();

    // Terms have to go through whatever the index's terms went through.
    let recorded = Meta::parse(index.meta())?.tokenizer.id;
    let terms: Vec<String> = match Tokenizer::parse(&recorded) {
        Some(tokenizer) => terms.iter().map(|t| tokenizer.normalize_query(t)).collect(),
        None => {
            eprintln!(
                "warning: this index was built with the '{recorded}' tokenizer, which this \
                 build does not implement; terms are used as typed"
            );
            terms.iter().map(|t| t.to_string()).collect()
        }
    };
    // The resident set is charged to opening the file, not to the query
    // (SPEC.md §4). A long-lived reader pays it once.
    reader.reset();

    let query: Vec<&[u8]> = terms.iter().map(|t| t.as_bytes()).collect();
    let matches = block_on(index.search(&query))?;

    for m in &matches {
        println!("{}\t{}", m.doc, crate::payload::render(&m.payload));
    }

    let bound = 2 * terms.len() + matches.len();
    eprintln!();
    eprintln!("{} match(es)", matches.len());
    eprintln!("open: {open_reads} read(s)");
    eprintln!("query: {} read(s), bound {bound}", reader.reads());
    Ok(())
}
