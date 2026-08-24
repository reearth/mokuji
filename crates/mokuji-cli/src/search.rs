//! `mokuji search` - intersect terms, rank them, and report what it cost.
//!
//! The ranking here is a sample, not part of the format. `SPEC.md` §1 puts
//! scoring outside the specification on the grounds that which model is right
//! is a property of the corpus; what the format supplies is the postings and
//! the statistics a scorer needs. This is what using them looks like.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{block_on::block_on, CountingReader, Index, RangeReader, SliceReader};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// Intersects `terms` against the index at `path`, reporting what it cost.
pub fn run(path: &Path, terms: &[&str], limit: usize) -> Result<(), Error> {
    let reader = CountingReader::new(SliceReader::new(std::fs::read(path)?));
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();

    // Terms have to go through whatever the index's terms went through. Put
    // each through the tokenizer rather than merely rewriting it: an analyzer
    // that splits turns one word the user typed into several that must all be
    // present.
    let recorded = Meta::parse(index.meta())?.tokenizer.id;
    let terms: Vec<String> = match Tokenizer::parse(&recorded) {
        Some(tokenizer) => terms.iter().flat_map(|t| tokenizer.split(t)).collect(),
        None => {
            eprintln!(
                "warning: this index was built with the '{recorded}' tokenizer, which this \
                 build does not implement; terms are used as typed"
            );
            terms.iter().map(|t| t.to_string()).collect()
        }
    };
    if terms.is_empty() {
        return Err(Error::NoTerms);
    }
    reader.reset();

    let ranked = block_on(rank(&index, &terms))?;
    let after_postings = reader.reads();

    let shown: Vec<u64> = ranked.iter().take(limit).map(|(doc, _)| *doc).collect();
    let payloads = block_on(index.payload_span(&shown))?;

    for ((doc, score), payload) in ranked.iter().zip(&payloads) {
        match score {
            Some(score) => println!("{doc}\t{score:.3}\t{payload}"),
            None => println!("{doc}\t{payload}"),
        }
    }

    eprintln!();
    eprintln!(
        "{} match(es), {} shown{}",
        ranked.len(),
        shown.len(),
        if index.doc_lengths().is_some() {
            ", ranked by BM25"
        } else {
            ", unranked: this index carries no document lengths"
        }
    );
    eprintln!("open:     {open_reads} read(s)");
    eprintln!(
        "query:    {} read(s) - {after_postings} dictionary and postings, {} payload",
        reader.reads(),
        reader.reads() - after_postings
    );
    eprintln!(
        "bytes:    {} of {}",
        reader.stats().bytes,
        reader.file_len()
    );
    Ok(())
}

/// The documents holding every term, best first when the file allows it.
///
/// Returns `None` for the score when the index carries no document lengths, in
/// which case the order is by document ID and means nothing.
async fn rank<R: RangeReader>(
    index: &Index<R>,
    terms: &[String],
) -> Result<Vec<(u64, Option<f64>)>, Error> {
    let mut runs = Vec::with_capacity(terms.len());
    for term in terms {
        let Some(entry) = index.lookup(term.as_bytes()).await? else {
            return Ok(Vec::new());
        };
        runs.push((entry, index.postings(&entry).await?));
    }

    let mut matched: Vec<u64> = runs[0].1.docs.clone();
    for (_, postings) in &runs[1..] {
        matched.retain(|doc| postings.docs.binary_search(doc).is_ok());
    }
    if matched.is_empty() {
        return Ok(Vec::new());
    }

    let Some(lengths) = index.doc_lengths() else {
        return Ok(matched.into_iter().map(|doc| (doc, None)).collect());
    };
    let header = index.header();
    let n = header.num_docs as f64;
    let avgdl = header.avg_doclen().unwrap_or(1.0).max(1.0);

    // Robertson and Sparck Jones' usual constants. They are tuning, which is
    // corpus-dependent, which is why none of this is in the format.
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    let mut scored: Vec<(u64, Option<f64>)> = matched
        .into_iter()
        .map(|doc| {
            let dl = lengths.get(doc as usize).copied().unwrap_or(0) as f64;
            let score: f64 = runs
                .iter()
                .map(|(entry, postings)| {
                    let df = entry.doc_freq as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let at = postings
                        .docs
                        .binary_search(&doc)
                        .expect("in the intersection");
                    let tf = postings.freqs[at] as f64;
                    idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * dl / avgdl))
                })
                .sum();
            (doc, Some(score))
        })
        .collect();

    // Ties broken by document ID, so that a run is reproducible.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    Ok(scored)
}
