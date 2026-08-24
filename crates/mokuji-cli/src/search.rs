//! `mokuji search` - intersect terms, rank them, and report what it cost.
//!
//! The ranking here is a sample, not part of the format. `SPEC.md` §1 puts
//! scoring outside the specification on the grounds that which model is right
//! is a property of the corpus; what the format supplies is the postings and
//! the statistics a scorer needs. This is what using them looks like.

use std::path::Path;

use mokuji_meta::Meta;
use mokuji_read::{block_on::block_on, CountingReader, Index, RangeReader, SliceReader, TermEntry};

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
    let (ranked, probed, skipped) = block_on(rank(
        &index,
        &terms,
        column,
        length_norm,
        phrase,
        combine,
        probe,
    ))?;
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
    Ok(())
}

/// The documents the query matches, best first when the file allows it.
///
/// Returns the ranking, the terms whose postings were actually fetched with
/// their document frequencies, and how many were left unfetched.
///
/// Returns `None` for the score when the index carries no document lengths, in
/// which case the order is by document ID and means nothing.
#[allow(clippy::type_complexity)]
async fn rank<R: RangeReader>(
    index: &Index<R>,
    terms: &[String],
    boost: Option<&mokuji_read::Column>,
    length_norm: f64,
    phrase: bool,
    combine: Combine,
    probe: Option<usize>,
) -> Result<(Vec<(u64, Option<f64>)>, Vec<(String, u64)>, usize), Error> {
    let empty = |probed: Vec<(String, u64)>, skipped: usize| (Vec::new(), probed, skipped);

    // One read per dictionary block the terms fall in, and the entries carry
    // `doc_freq` — so what each term's postings will cost is known before any
    // of them are read.
    let keys: Vec<&[u8]> = terms.iter().map(|t| t.as_bytes()).collect();
    let found = index.lookup_many(&keys).await?;

    let mut present: Vec<(String, TermEntry)> = Vec::new();
    for (term, entry) in terms.iter().zip(found) {
        match entry {
            Some(entry) => present.push((term.clone(), entry)),
            // A term nobody wrote down cannot be in an intersection, and there
            // is nothing to skip past in a union.
            None if combine == Combine::All => return Ok(empty(Vec::new(), 0)),
            None => {}
        }
    }
    if present.is_empty() {
        return Ok(empty(Vec::new(), 0));
    }

    // Rarest first: a term matching few documents says the most about which
    // ones, and costs the least to say it.
    let mut skipped = 0;
    if let Some(probe) = probe {
        present.sort_by_key(|(_, entry)| entry.doc_freq);
        skipped = present.len().saturating_sub(probe);
        present.truncate(probe);
    }

    let mut runs = Vec::with_capacity(present.len());
    for (_, entry) in &present {
        runs.push((*entry, index.postings(entry).await?));
    }
    let probed: Vec<(String, u64)> = present
        .iter()
        .map(|(term, entry)| (term.clone(), entry.doc_freq))
        .collect();

    let mut matched: Vec<u64> = match combine {
        Combine::All => {
            let mut matched = runs[0].1.docs.clone();
            for (_, postings) in &runs[1..] {
                matched.retain(|doc| postings.docs.binary_search(doc).is_ok());
            }
            matched
        }
        Combine::Any => {
            let mut all: Vec<u64> = runs
                .iter()
                .flat_map(|(_, p)| p.docs.iter().copied())
                .collect();
            all.sort_unstable();
            all.dedup();
            all
        }
    };
    if matched.is_empty() {
        return Ok(empty(probed, skipped));
    }

    if phrase {
        // One further read per term, and only here. The terms have to occur at
        // consecutive positions, in the order they were typed.
        let mut places = Vec::with_capacity(runs.len());
        for (entry, postings) in &runs {
            places.push(index.positions(entry, postings).await?);
        }
        matched.retain(|doc| {
            let at = |i: usize| -> &[u64] {
                let run = &runs[i].1;
                let index = run.docs.binary_search(doc).expect("in the intersection");
                places[i][index].as_slice()
            };
            at(0).iter().any(|&start| {
                (1..runs.len()).all(|i| at(i).binary_search(&(start + i as u64)).is_ok())
            })
        });
        if matched.is_empty() {
            return Ok(empty(probed, skipped));
        }
    }

    let Some(lengths) = index.doc_lengths() else {
        return Ok((
            matched.into_iter().map(|doc| (doc, None)).collect(),
            probed,
            skipped,
        ));
    };
    let header = index.header();
    let n = header.num_docs as f64;
    let avgdl = header.avg_doclen().unwrap_or(1.0).max(1.0);

    // Robertson and Sparck Jones' usual constant. Tuning, which is
    // corpus-dependent, which is why none of this is in the format.
    const K1: f64 = 1.2;
    let b = length_norm;

    let mut scored: Vec<(u64, Option<f64>)> = matched
        .into_iter()
        .map(|doc| {
            let dl = lengths.get(doc as usize).copied().unwrap_or(0) as f64;
            // A union scores over the terms the document actually has, which is
            // what makes matching more of them rank higher.
            let score: f64 = runs
                .iter()
                .filter_map(|(entry, postings)| {
                    let at = postings.docs.binary_search(&doc).ok()?;
                    let df = entry.doc_freq as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let tf = postings.freqs[at] as f64;
                    Some(idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - b + b * dl / avgdl)))
                })
                .sum();
            // A static prior, added in log space. Which prior, and how much of
            // it, is tuning — which is why the format supplies the column and
            // says nothing about what to do with it.
            let prior = boost.map_or(0.0, |column| {
                let value = column.values.get(doc as usize).copied().unwrap_or(0);
                (1.0 + value.max(0) as f64).ln()
            });
            (doc, Some(score + prior))
        })
        .collect();

    // Ties broken by document ID, so that a run is reproducible.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    Ok((scored, probed, skipped))
}
