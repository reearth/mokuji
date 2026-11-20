//! `mokuji prefix` - complete a prefix, and report what it cost.
//!
//! This is the query a hierarchical key exists for. An address is looked up
//! whole or by prefix, never by intersecting its parts, so this rather than
//! `search` is the shape forward geocoding takes.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{
    block_on::block_on, CountingReader, Index, OpenOptions, RangeReader, SliceReader,
};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// Completes `query` against the index at `path`, reporting what it cost.
pub fn run(path: &Path, query: &str, limit: usize, resident: bool) -> Result<(), Error> {
    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open_with(
        &reader,
        OpenOptions {
            resident_dictionary: resident,
        },
    ))?;
    let open_reads = reader.reads();

    // The query has to go through whatever the terms went through, or it will
    // not match them. The index records which tokenizer that was, so a reader
    // does not have to be told.
    let meta = Meta::parse(index.meta())?;
    let recorded = meta.tokenizer.id.clone();
    let normalized = match Tokenizer::parse(&recorded) {
        Some(tokenizer) => tokenizer.normalize_query(query),
        None => {
            eprintln!(
                "warning: this index was built with the '{recorded}' tokenizer, which this \
                 build does not implement; the query is used as typed"
            );
            query.to_string()
        }
    };
    if normalized != query {
        eprintln!("query normalized to '{normalized}' by the '{recorded}' tokenizer");
    }
    let prefix = normalized.as_bytes();
    reader.reset();

    // Each phase is measured rather than assumed. An index whose entries carry
    // implied postings spends no read on them at all, and a breakdown that
    // asserted one would be reporting the code's expectations instead of what
    // the file cost.
    let matches = block_on(index.prefix(prefix, limit))?;
    let dictionary = reader.reads();

    let entries: Vec<_> = matches.iter().map(|(_, entry)| entry.clone()).collect();
    let postings = block_on(index.postings_span(&entries))?;
    let postings_reads = reader.reads() - dictionary;

    // A term may name several records — a postal code covers many addresses —
    // so every document is a result, not just the first. Collected in document
    // order so the payload reads share blocks.
    let mut hits: Vec<(usize, u64)> = Vec::new();
    for (i, run) in postings.iter().enumerate() {
        for &doc in &run.docs {
            hits.push((i, doc));
        }
    }
    hits.truncate(limit);
    hits.sort_by_key(|(_, doc)| *doc);

    let docs: Vec<u64> = hits.iter().map(|(_, doc)| *doc).collect();
    let payloads = block_on(index.payload_span(&docs))?;
    let payload_reads = reader.reads() - dictionary - postings_reads;

    for ((i, _), payload) in hits.iter().zip(&payloads) {
        println!("{}\t{}", String::from_utf8_lossy(&matches[*i].0), payload);
    }

    eprintln!();
    eprintln!("{} term(s), {} record(s) shown", matches.len(), hits.len());
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
    // Results derived from a licensed database carry its credit with them;
    // the file says what that is, so the tool says it too.
    if let Some(attribution) = &meta.attribution {
        eprintln!("data:     {attribution}");
    }
    Ok(())
}
