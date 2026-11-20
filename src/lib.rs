//! Search a Mokuji file: open it, and look up, complete or rank what a person
//! typed, reading only the ranges a query needs.
//!
//! The layer above [`read`], which reads sections and takes terms as
//! bytes already normalized. This one puts what was typed through the
//! tokenizer the file records, as a person's query has to be, and answers in
//! the same shape as the TypeScript reader (`mokuji`) and the Python
//! one, so that the three read alike:
//!
//! ```no_run
//! # async fn run() -> Result<(), mokuji::Error> {
//! use mokuji::MokujiIndex;
//!
//! let index = MokujiIndex::open_file("postal.mokuji").await?;
//! let found = index.lookup("1000005", 10).await?;
//! # Ok(()) }
//! ```
//!
//! The methods are `async` because a range may be fetched over a network. A
//! file on disk is read synchronously underneath, so a program with no
//! runtime can drive them with [`block_on`].

use std::path::Path;

// The modules follow the sections of the specification, because keeping the
// dictionary, the postings and the payload from knowing about each other is a
// requirement of the format and not only good practice.
pub mod codec;
pub mod format;
pub mod manifest;
pub mod meta;
pub mod read;
pub mod tokenize;
pub mod write;

/// The `mokuji` command: what each subcommand does, as a library, so that
/// the tests can drive it. Built with the `cli` feature.
#[cfg(feature = "cli")]
pub mod cli;

use crate::meta::Meta;
use crate::read::{Column, CountingReader, Index, TermEntry};

pub use crate::format::Error as FormatError;
pub use crate::read::block_on::block_on;
pub use crate::read::{FileReader, Payload, RangeReader, ReadStats, SliceReader};
pub use crate::tokenize::Tokenizer;

/// What can go wrong opening or asking a file.
#[derive(Debug)]
pub enum Error {
    /// The file could not be read, or is not a valid Mokuji file.
    File(FormatError),
    /// A query named an attribute column the file does not carry.
    NoColumn(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::File(e) => write!(f, "{e}"),
            Error::NoColumn(name) => {
                write!(f, "this index carries no attribute column called '{name}'")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<FormatError> for Error {
    fn from(e: FormatError) -> Self {
        Error::File(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::File(e.into())
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An open Mokuji file, asked in what a person types.
#[derive(Debug)]
pub struct MokujiIndex<R> {
    index: Index<CountingReader<R>>,
    meta: Meta,
    tokenizer: Option<Tokenizer>,
}

/// A document a lookup found, under the term that found it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The term, as the file stores it.
    pub term: String,
    /// The document's ID.
    pub doc: u64,
    /// What the file returns for it.
    pub payload: Payload,
}

/// What [`MokujiIndex::lookup`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    /// What was typed, after the file's tokenizer.
    pub normalized: String,
    /// Every document of every term the prefix matched, in term order.
    pub results: Vec<Hit>,
}

/// What [`MokujiIndex::complete`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// What was typed, after the file's tokenizer.
    pub normalized: String,
    /// Whether the completions section answered, at no read before the payload.
    pub from_table: bool,
    /// The documents, in the order the file lists them.
    pub docs: Vec<u64>,
    /// Their payloads, in the same order.
    pub payloads: Vec<Payload>,
}

/// How a search combines and ranks its terms. The defaults are `mokuji
/// search`'s, and the TypeScript reader's.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// How many results to return with their payload.
    pub limit: usize,
    /// The union of the terms rather than their intersection: what an n-gram
    /// query wants, since a misspelling shares every gram but two.
    pub any: bool,
    /// Fetch postings for only this many of the terms, the rarest, which the
    /// dictionary says without reading any postings.
    pub probe: Option<usize>,
    /// BM25's `b`, from 0 to 1. Zero for a corpus whose long documents are its
    /// good ones.
    pub length_norm: f64,
    /// An attribute column added to each score as `ln(1 + value)`, a static
    /// prior.
    pub boost: Option<String>,
    /// Require the terms at consecutive positions; the file must carry them.
    pub phrase: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            limit: 10,
            any: false,
            probe: None,
            length_norm: 0.75,
            boost: None,
            phrase: false,
        }
    }
}

/// A document a search matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// The document's ID.
    pub doc: u64,
    /// Its BM25 score, or `None` when the file carries no document lengths and
    /// the order is by document ID and means nothing.
    pub score: Option<f64>,
    /// What the file returns for it.
    pub payload: Payload,
}

/// What [`MokujiIndex::search`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// The terms the query became.
    pub terms: Vec<String>,
    /// The terms whose postings were read, with how many documents each names.
    pub probed: Vec<(String, u64)>,
    /// Terms present but not read, because `probe` left them out.
    pub skipped: usize,
    /// How many documents matched, of which `results` are the first `limit`.
    pub matched: usize,
    /// The best `limit`, best first.
    pub results: Vec<Scored>,
}

impl MokujiIndex<FileReader> {
    /// Opens the file at `path`, to be read a range at a time.
    pub async fn open_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(FileReader::open(path)?).await
    }
}

impl<R: RangeReader> MokujiIndex<R> {
    /// Opens a file through `reader`: its header and everything resident, the
    /// table of contents, in one read where the layout allows.
    pub async fn open(reader: R) -> Result<Self> {
        let index = Index::open(CountingReader::new(reader)).await?;
        let meta = Meta::parse(index.meta())?;
        let tokenizer = Tokenizer::parse(&meta.tokenizer.id);
        Ok(MokujiIndex {
            index,
            meta,
            tokenizer,
        })
    }

    /// The tokenizer the file records, by its registered identifier.
    pub fn tokenizer_id(&self) -> &str {
        &self.meta.tokenizer.id
    }

    /// What a query goes through before it is looked up: the file's tokenizer,
    /// without splitting. A tokenizer this build does not know leaves it as
    /// typed.
    pub fn normalize(&self, text: &str) -> String {
        match self.tokenizer {
            Some(t) => t.normalize_query(text),
            None => text.to_string(),
        }
    }

    /// The credit results from this file carry, if it states one.
    pub fn attribution(&self) -> Option<&str> {
        self.meta.attribution.as_deref()
    }

    /// The index's license, as an SPDX expression, if it states one.
    pub fn license(&self) -> Option<&str> {
        self.meta.license.as_deref()
    }

    /// The metadata section, decoded.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// An attribute column, by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.index.column(name)
    }

    /// The reads and bytes since open, or since [`Self::reset_stats`].
    pub fn stats(&self) -> ReadStats {
        self.index.reader().stats()
    }

    /// Starts counting from zero, so that what follows is one query's cost.
    pub fn reset_stats(&self) {
        self.index.reader().reset();
    }

    /// The file's size in bytes.
    pub fn file_len(&self) -> u64 {
        self.index.reader().file_len()
    }

    /// The reader one layer down, which takes terms as bytes.
    pub fn index(&self) -> &Index<CountingReader<R>> {
        &self.index
    }

    /// The terms beginning with what was typed, at most `limit` of them, and
    /// every document each names, with its payload.
    ///
    /// The query a hierarchical key is for: an address, a postal code, a path.
    /// A contiguous range of terms is contiguous in every section, so this
    /// costs the same few reads at any file size.
    pub async fn lookup(&self, text: &str, limit: usize) -> Result<Lookup> {
        let normalized = self.normalize(text);
        let entries = self.index.prefix(normalized.as_bytes(), limit).await?;
        let terms: Vec<TermEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
        let postings = self.index.postings_span(&terms).await?;

        // A term may name more than one document, so all of them are fetched,
        // in ascending order, so that neighbours share payload reads.
        let mut wanted: Vec<u64> = postings
            .iter()
            .flat_map(|p| p.docs.iter().copied())
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let fetched = self.index.payload_span(&wanted).await?;

        let mut results = Vec::new();
        for ((key, _), run) in entries.iter().zip(&postings) {
            let term = String::from_utf8_lossy(key).into_owned();
            for doc in &run.docs {
                let at = wanted.binary_search(doc).expect("fetched above");
                results.push(Hit {
                    term: term.clone(),
                    doc: *doc,
                    payload: fetched[at].clone(),
                });
            }
        }
        Ok(Lookup {
            normalized,
            results,
        })
    }

    /// The first `limit` documents holding a term that begins with what was
    /// typed, from the completions section when it answers.
    pub async fn complete(&self, text: &str, limit: usize) -> Result<Completion> {
        let normalized = self.normalize(text);
        let (docs, from_table) = self.index.complete(normalized.as_bytes(), limit).await?;
        let payloads = self.index.payload_span(&docs).await?;
        Ok(Completion {
            normalized,
            from_table,
            docs,
            payloads,
        })
    }

    /// The documents matching what was typed, split into terms by the file's
    /// tokenizer, ranked by BM25 when the file carries document lengths.
    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<SearchResult> {
        let terms: Vec<String> = match self.tokenizer {
            Some(t) => t.split(query),
            None => vec![query.to_string()],
        };
        let column = match &options.boost {
            Some(name) => Some(
                self.index
                    .column(name)
                    .ok_or_else(|| Error::NoColumn(name.clone()))?,
            ),
            None => None,
        };
        let ranking = rank(&self.index, &terms, column, options).await?;

        // The best `limit`, fetched in document order so that neighbours share
        // a payload read, and returned in rank order.
        let shown: Vec<(u64, Option<f64>)> =
            ranking.ranked.iter().take(options.limit).copied().collect();
        let mut docs: Vec<u64> = shown.iter().map(|(doc, _)| *doc).collect();
        docs.sort_unstable();
        let payloads = self.index.payload_span(&docs).await?;
        let results = shown
            .into_iter()
            .map(|(doc, score)| Scored {
                doc,
                score,
                payload: payloads[docs.binary_search(&doc).expect("fetched above")].clone(),
            })
            .collect();
        Ok(SearchResult {
            terms,
            matched: ranking.ranked.len(),
            probed: ranking.probed,
            skipped: ranking.skipped,
            results,
        })
    }
}

/// Every document a set of terms matches, best first, and what finding them
/// read: the half of a search before any payload, which `mokuji search` uses
/// directly so that it can say what each half cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranking {
    /// Documents with their scores, best first; `None` scores when the file
    /// carries no document lengths, in document order.
    pub ranked: Vec<(u64, Option<f64>)>,
    /// The terms whose postings were read, with their document frequencies.
    pub probed: Vec<(String, u64)>,
    /// Terms present but left unread by `probe`.
    pub skipped: usize,
}

/// Matches and ranks `terms`, already put through the tokenizer.
///
/// The ranking is a sample, not part of the format. `SPEC.md` §1 puts scoring
/// outside the specification, since which model is right is a property of the
/// corpus; what the format supplies is the postings and the statistics a
/// scorer needs. This is what using them looks like, and the TypeScript and
/// Python readers rank the same way: to about 1e-12, since JavaScript's
/// `Math.log` is not correctly rounded, so documents whose scores differ by
/// less than that may be ordered differently there.
pub async fn rank<R: RangeReader>(
    index: &Index<R>,
    terms: &[String],
    boost: Option<&Column>,
    options: &SearchOptions,
) -> Result<Ranking> {
    let SearchOptions {
        any,
        probe,
        length_norm,
        phrase,
        ..
    } = *options;
    let empty = |probed: Vec<(String, u64)>, skipped: usize| Ranking {
        ranked: Vec::new(),
        probed,
        skipped,
    };
    if phrase && !index.has_positions() {
        return Err(Error::File(FormatError::Malformed {
            detail: "this index carries no positions, so it cannot test a phrase",
        }));
    }

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
            None if !any => return Ok(empty(Vec::new(), 0)),
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
        // A probe of zero reads no postings, so nothing can be matched.
        if present.is_empty() {
            return Ok(empty(Vec::new(), skipped));
        }
    }

    let mut runs = Vec::with_capacity(present.len());
    for (_, entry) in &present {
        runs.push((entry.clone(), index.postings(entry).await?));
    }
    let probed: Vec<(String, u64)> = present
        .iter()
        .map(|(term, entry)| (term.clone(), entry.doc_freq))
        .collect();

    let mut matched: Vec<u64> = if any {
        let mut all: Vec<u64> = runs
            .iter()
            .flat_map(|(_, p)| p.docs.iter().copied())
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    } else {
        let mut matched = runs[0].1.docs.clone();
        for (_, postings) in &runs[1..] {
            matched.retain(|doc| postings.docs.binary_search(doc).is_ok());
        }
        matched
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
        return Ok(Ranking {
            ranked: matched.into_iter().map(|doc| (doc, None)).collect(),
            probed,
            skipped,
        });
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
    Ok(Ranking {
        ranked: scored,
        probed,
        skipped,
    })
}
