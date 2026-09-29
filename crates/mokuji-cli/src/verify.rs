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
    if let Some(mokuji_meta::Value::Map(entries)) = &meta.attrs {
        for (key, value) in entries {
            let key = match key {
                mokuji_meta::Value::Text(text) => text.clone(),
                other => format!("{other:?}"),
            };
            match value {
                mokuji_meta::Value::Text(text) => println!("attr            {key} = {text}"),
                other => println!("attr            {key} = {other:?}"),
            }
        }
    }
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
    if let Some(completions) = index.completions() {
        let checked = block_on(check_completions(&index, &stats.first_bytes))?;
        println!(
            "completions     {} entries, covering {} terms or {} bytes, {} documents each",
            completions.len(),
            completions.min_terms,
            completions.max_span,
            completions.docs_per_entry
        );
        println!("                {checked} prefixes checked for coverage");
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

struct Stats {
    terms: u64,
    blocks: u32,
    postings: u64,
    tokens: u64,
    /// Terms and postings bytes by first byte: the one-byte prefixes, which
    /// coverage starts from.
    first_bytes: Vec<(u64, u64)>,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            terms: 0,
            blocks: 0,
            postings: 0,
            tokens: 0,
            first_bytes: vec![(0, 0); 256],
        }
    }
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

            if let Some(&first) = term.first() {
                let slot = &mut stats.first_bytes[usize::from(first)];
                slot.0 += 1;
                slot.1 += entry.postings_len;
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

/// Checks what `SPEC.md` §5.3.5 requires of a writer and a reader cannot see:
/// that each entry lists documents its prefix matches, as many as it should,
/// and that every expensive prefix has one. Returns how many prefixes were
/// tested for coverage.
///
/// Coverage is tested without enumerating every prefix. Term counts and
/// postings sizes only shrink as a prefix grows, so the shortest expensive
/// prefix missing an entry, if there is one, is either one byte long or one
/// byte longer than a prefix that has an entry — and those are what is tried.
async fn check_completions<R: RangeReader>(
    index: &Index<R>,
    first_bytes: &[(u64, u64)],
) -> Result<u64, Error> {
    let completions = index.completions().expect("checked by the caller");
    let expensive = |count: u64, span: u64| {
        (completions.min_terms != 0 && count >= completions.min_terms)
            || (completions.max_span != 0 && span >= completions.max_span)
    };
    let missing = |prefix: &[u8]| {
        Error::Invalid(format!(
            "prefix {:?} is expensive and has no completion entry",
            String::from_utf8_lossy(prefix)
        ))
    };

    let mut checked = 0u64;
    if completions.covers() {
        for (byte, &(count, span)) in first_bytes.iter().enumerate() {
            checked += u64::from(count > 0);
            if expensive(count, span) && completions.get(&[byte as u8]).is_none() {
                return Err(missing(&[byte as u8]));
            }
        }
    }

    for (prefix, listed) in completions.entries() {
        let matched = index.prefix(prefix, usize::MAX).await?;
        let entries: Vec<_> = matched.iter().map(|(_, e)| *e).collect();
        let mut docs: Vec<u64> = index
            .postings_span(&entries)
            .await?
            .into_iter()
            .flat_map(|p| p.docs)
            .collect();
        docs.sort_unstable();
        docs.dedup();

        let what = || String::from_utf8_lossy(prefix).into_owned();
        if let Some(stray) = listed.iter().find(|d| docs.binary_search(d).is_err()) {
            return Err(Error::Invalid(format!(
                "the completion for {:?} lists document {stray}, which holds no term beginning with it",
                what()
            )));
        }
        let due = (docs.len() as u64).min(completions.docs_per_entry);
        if listed.len() as u64 != due {
            return Err(Error::Invalid(format!(
                "the completion for {:?} lists {} documents where it should list {due}",
                what(),
                listed.len()
            )));
        }

        if completions.covers() {
            let mut children: std::collections::BTreeMap<u8, (u64, u64)> = Default::default();
            for (term, entry) in &matched {
                if let Some(&next) = term.get(prefix.len()) {
                    let child = children.entry(next).or_default();
                    child.0 += 1;
                    child.1 += entry.postings_len;
                }
            }
            for (next, (count, span)) in children {
                checked += 1;
                let mut child = prefix.to_vec();
                child.push(next);
                if expensive(count, span) && completions.get(&child).is_none() {
                    return Err(missing(&child));
                }
            }
        }
    }
    Ok(checked)
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
