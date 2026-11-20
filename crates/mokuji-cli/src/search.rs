//! `mokuji search` - intersect terms, rank them, and report what it cost.
//!
//! The ranking here is a sample, not part of the format. `SPEC.md` §1 puts
//! scoring outside the specification on the grounds that which model is right
//! is a property of the corpus; what the format supplies is the postings and
//! the statistics a scorer needs. This is what using them looks like.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{block_on::block_on, CountingReader, FileReader, Index, RangeReader};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// How the terms of a query are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// Every term must be present. What a search for words means.
    All,
    /// Any term will do, and matching more of them scores higher.
    ///
    /// What a search over n-grams means: a query with one character wrong
    /// shares every n-gram but two with the term it was meant to be, and
    /// requiring all of them would find nothing.
    Any,
}

/// Everything about a query except the terms and the file.
pub struct Query<'a> {
    /// How many results to show.
    pub limit: usize,
    /// An attribute column to add as a static prior.
    pub boost: Option<&'a str>,
    /// BM25's `b`. Zero for a corpus whose long documents are its good ones.
    pub length_norm: f64,
    /// Require the terms to occur at consecutive positions.
    pub phrase: bool,
    /// Whether every term must be present.
    pub combine: Combine,
    /// Fetch postings for at most this many terms, rarest first.
    pub probe: Option<usize>,
}

impl Default for Query<'_> {
    fn default() -> Self {
        Query {
            limit: 10,
            boost: None,
            length_norm: 0.75,
            phrase: false,
            combine: Combine::All,
            probe: None,
        }
    }
}

/// Runs `terms` against the index at `path`, reporting what it cost.
pub fn run(path: &Path, terms: &[&str], query: &Query<'_>) -> Result<(), Error> {
    let Query {
        limit,
        boost,
        length_norm,
        phrase,
        combine,
        probe,
    } = *query;
    let reader = CountingReader::new(FileReader::open(path)?);
    let index = block_on(Index::open(&reader))?;
    let open_reads = reader.reads();

    // Terms have to go through whatever the index's terms went through. Put
    // each through the tokenizer rather than merely rewriting it: an analyzer
    // that splits turns one word the user typed into several that must all be
    // present.
    let meta = Meta::parse(index.meta())?;
    let recorded = meta.tokenizer.id.clone();
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

    let column = match boost {
        Some(name) => Some(index.column(name).ok_or_else(|| {
            Error::Invalid(format!(
                "this index carries no attribute column called '{name}'"
            ))
        })?),
        None => None,
    };
    if phrase && !index.has_positions() {
        return Err(Error::Invalid(
            "this index carries no positions, so it cannot test a phrase".into(),
        ));
    }
    let options = mokuji::SearchOptions {
        limit,
        any: combine == Combine::Any,
        probe,
        length_norm,
        boost: boost.map(str::to_string),
        phrase,
    };
    let mokuji::Ranking {
        ranked,
        probed,
        skipped,
    } = block_on(mokuji::rank(&index, &terms, column, &options))?;
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
    if !probed.is_empty() && (combine == Combine::Any || probe.is_some()) {
        eprintln!(
            "probed:   {}",
            probed
                .iter()
                .map(|(term, df)| format!("{term}:{df}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if skipped > 0 {
            // The point of the dictionary carrying doc_freq: a reader knows
            // what a term's postings will cost before it pays for them.
            eprintln!(
                "skipped:  {skipped} term(s) with more postings, without reading any of them"
            );
        }
    }
    eprintln!(
        "{} match(es){}, {} shown{}",
        ranked.len(),
        if phrase { " as a phrase" } else { "" },
        shown.len(),
        if index.doc_lengths().is_some() {
            match boost {
                Some(_) => ", ranked by BM25 and a static prior",
                None => ", ranked by BM25",
            }
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
    // Results derived from a licensed database carry its credit with them;
    // the file says what that is, so the tool says it too.
    if let Some(attribution) = &meta.attribution {
        eprintln!("data:     {attribution}");
    }
    Ok(())
}
