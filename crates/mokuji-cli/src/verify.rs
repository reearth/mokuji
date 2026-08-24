//! `mokuji verify` - walk a whole file and check what it claims about itself.
//!
//! A query touches a handful of blocks and trusts the rest. Publishing a file
//! is a claim about all of it, so this reads every block, decodes every
//! postings run, and fetches every payload. It is the one operation in the tool
//! that is deliberately not bounded.

use std::path::Path;

use mokuji_meta::{ContentHash, Meta};
use mokuji_read::{block_on::block_on, term, CountingReader, Index, RangeReader, SliceReader};
use sha2::{Digest, Sha256};

use crate::info::Error;

/// Verifies `path`, optionally against the source it claims to derive from.
pub fn run(path: &Path, source: Option<&Path>) -> Result<(), Error> {
    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open(&reader))?;
    let meta = Meta::parse(index.meta())?;

    println!("generation      {}", meta.generation);
    println!(
        "generator       {} {}",
        meta.generator.name, meta.generator.version
    );
    println!("tokenizer       {}", meta.tokenizer.id);
    for entry in &meta.sources {
        println!("derived from    {}", entry.content_hash);
        if let Some(uri) = &entry.uri {
            println!("                {uri}");
        }
    }

    let stats = block_on(walk(&index))?;
    println!();
    println!("terms           {}", stats.terms);
    println!("term blocks     {}", stats.blocks);
    println!("postings        {}", stats.postings);
    println!("documents       {}", index.header().num_docs);
    println!(
        "tokens          {} indexed, {} declared in the header",
        stats.tokens,
        index.header().sum_doclen
    );
    if stats.tokens != index.header().sum_doclen {
        // Not an error. The header's total is the document length used for
        // scoring, which may legitimately count tokens the tokenizer chose not
        // to index. Worth reporting, not worth rejecting.
        println!("                (these differ; the tokenizer may drop terms it does not index)");
    }
    println!("bytes read      {}", reader.stats().bytes);

    match source {
        Some(source) => check_source(&meta, source)?,
        None => {
            println!();
            println!(
                "provenance not checked: pass --source to hash the file it claims to derive from"
            );
        }
    }

    println!();
    println!("ok");
    Ok(())
}

#[derive(Default)]
struct Stats {
    terms: u64,
    blocks: u32,
    postings: u64,
    tokens: u64,
}

/// Reads every block and checks the structure holds together.
async fn walk<R: RangeReader>(index: &Index<R>) -> Result<Stats, Error> {
    let term_index = index.term_index()?;
    let boundaries = term_index.keys()?;
    let mut stats = Stats {
        blocks: term_index.block_count(),
        ..Stats::default()
    };

    let mut previous_term: Option<Vec<u8>> = None;
    let mut expected_term_id = 0u64;
    for ordinal in 0..term_index.block_count() {
        let block = index.read_term_block(ordinal).await?;
        let entries = term::decode_block(&block, index.has_positions())?;
        if entries.is_empty() {
            return Err(Error::Invalid(format!("term block {ordinal} is empty")));
        }

        // The index promises that block `i` begins with boundary key `i`. A
        // query relies on that without being able to check it.
        if entries[0].0 != boundaries[ordinal as usize] {
            return Err(Error::Invalid(format!(
                "term block {ordinal} does not begin with the key the index gives for it"
            )));
        }

        for (term, entry) in entries {
            if let Some(previous) = &previous_term {
                if &term <= previous {
                    return Err(Error::Invalid(
                        "terms are not strictly increasing across blocks".into(),
                    ));
                }
            }
            // Dense and sequential in term order: the monotonic ID rule
            // (SPEC.md §3.2) made checkable.
            if entry.term_id != expected_term_id {
                return Err(Error::Invalid(format!(
                    "term ids are not dense: expected {expected_term_id}, found {}",
                    entry.term_id
                )));
            }

            let postings = index.postings(&entry).await?;
            stats.postings += postings.len() as u64;
            stats.tokens += postings.freqs.iter().sum::<u64>();
            stats.terms += 1;
            expected_term_id += 1;
            previous_term = Some(term);
        }
    }

    for doc in 0..index.header().num_docs {
        index.read_payload(doc).await?;
    }
    Ok(stats)
}

fn check_source(meta: &Meta, source: &Path) -> Result<(), Error> {
    let digest = ContentHash::sha2_256(&Sha256::digest(std::fs::read(source)?));
    println!();
    println!("source          {}", source.display());
    println!("hashes to       {digest}");

    match meta.sources.iter().find(|s| s.content_hash == digest) {
        Some(_) => {
            println!("                matches the recorded provenance");
            Ok(())
        }
        None => Err(Error::Invalid(format!(
            "the source hashes to {digest}, which is not among the sources this index records"
        ))),
    }
}
