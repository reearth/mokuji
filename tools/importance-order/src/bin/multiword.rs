//! Completing what someone is typing, two ways, on files numbered by importance.
//!
//! - `words`: the gazetteer as `bigram` indexes it. The words already typed are
//!   exact terms; the last, unfinished one is a prefix whose terms' runs are
//!   united; the two are intersected.
//! - `names`: every name one term, so what has been typed is a single prefix of
//!   a name, spaces and all.
//!
//! Each is measured without and with the table `completion` builds, held
//! resident: a prefix in the table is answered by it, at no read, when it is the
//! whole question. In `words` it is the whole question only when nothing else
//! was typed; an unfinished word after a finished one still has to be united
//! and intersected. Whether a prefix is in the table is decided here from an
//! uncounted copy of the file, the same test `completion` applies.
//!
//! The queries file holds `class<TAB>typed` lines.
//!
//!     multiword words.mokuji names.mokuji typed.txt [-k 10] [--min-terms 1024] [--max-span 16384]

use std::collections::BTreeMap;

use importance_order::FileReader;
use mokuji_cli::corpus::Tokenizer;
use mokuji_format::Result;
use mokuji_read::block_on::block_on;
use mokuji_read::{CountingReader, Index, TermEntry};

type Counted = CountingReader<FileReader>;

struct Files<'a> {
    counted: &'a Counted,
    index: Index<&'a Counted>,
    oracle: Index<FileReader>,
}

impl Files<'_> {
    fn stats(&self) -> (u32, u64) {
        let s = self.counted.stats();
        (s.reads, s.bytes)
    }
}

fn open<'a>(path: &str, counted: &'a Counted) -> Files<'a> {
    Files {
        counted,
        index: block_on(Index::open(counted)).unwrap(),
        oracle: block_on(Index::open(FileReader::open(path))).unwrap(),
    }
}

fn union(lists: impl IntoIterator<Item = Vec<u64>>) -> Vec<u64> {
    let mut all: Vec<u64> = lists.into_iter().flatten().collect();
    all.sort_unstable();
    all.dedup();
    all
}

fn intersect(mut lists: Vec<Vec<u64>>) -> Vec<u64> {
    lists.sort_by_key(Vec::len);
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .copied()
        .filter(|d| rest.iter().all(|l| l.binary_search(d).is_ok()))
        .collect()
}

/// A prefix's documents, and whether the table would hold it.
fn prefix_docs<R: mokuji_read::RangeReader>(
    index: &Index<R>,
    prefix: &[u8],
    min_terms: u64,
    max_span: u64,
) -> Result<(Vec<u64>, bool)> {
    let entries: Vec<TermEntry> = block_on(index.prefix(prefix, usize::MAX))?
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let span: u64 = entries.iter().map(|e| e.postings_len).sum();
    let expensive = entries.len() as u64 >= min_terms || span >= max_span;
    let docs = if entries.is_empty() {
        Vec::new()
    } else {
        union(
            block_on(index.postings_span(&entries))?
                .into_iter()
                .map(|p| p.docs),
        )
    };
    Ok((docs, expensive))
}

/// The top `k` for what has been typed, and whether the table answered it.
fn complete<R: mokuji_read::RangeReader>(
    index: &Index<R>,
    exact: &[Vec<u8>],
    prefix: Option<&[u8]>,
    k: usize,
    limits: (u64, u64),
    table: bool,
) -> Result<(Vec<u64>, bool)> {
    let mut lists = Vec::new();
    if !exact.is_empty() {
        let terms: Vec<&[u8]> = exact.iter().map(Vec::as_slice).collect();
        let entries = block_on(index.lookup_many(&terms))?;
        let Some(entries) = entries.into_iter().collect::<Option<Vec<_>>>() else {
            return Ok((Vec::new(), false));
        };
        for entry in &entries {
            lists.push(block_on(index.postings(entry))?.docs);
        }
    }
    if let Some(prefix) = prefix {
        if table && exact.is_empty() {
            // The caller has already established from the oracle that the
            // table holds this prefix; answering from it reads nothing.
            return Ok((Vec::new(), true));
        }
        lists.push(prefix_docs(index, prefix, limits.0, limits.1)?.0);
    }
    let mut top = intersect(lists);
    top.truncate(k);
    Ok((top, false))
}

#[derive(Default)]
struct Tally {
    reads: Vec<u64>,
    bytes: Vec<u64>,
    payload_reads: u64,
    table: u64,
    empty: u64,
}

impl Tally {
    fn row(&mut self, label: &str) {
        let n = self.reads.len().max(1) as f64;
        self.reads.sort_unstable();
        self.bytes.sort_unstable();
        let pct = |v: &[u64], p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
        println!(
            "{label:<20} {:>5} {:>5} {:>6} {:>9} {:>10} {:>11} {:>7.2} {:>6} {:>6}",
            pct(&self.reads, 0.5),
            pct(&self.reads, 0.99),
            self.reads.last().unwrap(),
            pct(&self.bytes, 0.5),
            pct(&self.bytes, 0.99),
            self.bytes.last().unwrap(),
            self.payload_reads as f64 / n,
            self.table,
            self.empty,
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: multiword <words.mokuji> <names.mokuji> <typed.txt>");
        std::process::exit(2);
    }
    let (mut k, mut min_terms, mut max_span) = (10usize, 1024u64, 16384u64);
    let mut rest = &args[4..];
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "-k" => k = value.parse().unwrap(),
            "--min-terms" => min_terms = value.parse().unwrap(),
            "--max-span" => max_span = value.parse().unwrap(),
            other => panic!("unknown option {other}"),
        }
        rest = tail;
    }
    let limits = (min_terms, max_span);

    let words_reader = CountingReader::new(FileReader::open(&args[1]));
    let names_reader = CountingReader::new(FileReader::open(&args[2]));
    let words = open(&args[1], &words_reader);
    let names = open(&args[2], &names_reader);

    let typed: Vec<(String, String)> = std::fs::read_to_string(&args[3])
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(c, q)| (c.to_string(), q.to_string()))
        .collect();

    // class -> strategy -> tally
    let mut tallies: BTreeMap<String, BTreeMap<&str, Tally>> = BTreeMap::new();
    let mut overlap: BTreeMap<String, (u64, u64)> = BTreeMap::new();

    for (class, query) in &typed {
        // Words: every token exact but the last, which is unfinished when the
        // query stops inside it — an ASCII word, or a single character.
        let mut tokens: Vec<Vec<u8>> = Tokenizer::Bigram
            .split(query)
            .into_iter()
            .map(String::into_bytes)
            .collect();
        let unfinished = query
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            || tokens
                .last()
                .is_some_and(|t| std::str::from_utf8(t).unwrap().chars().count() == 1);
        let partial = if unfinished { tokens.pop() } else { None };
        tokens.sort();
        tokens.dedup();

        // Names: the whole of what was typed, as the converter wrote a name.
        let key = query.to_lowercase().replace(' ', "\u{a0}").into_bytes();

        let word_expensive = partial.as_ref().is_some_and(|p| {
            prefix_docs(&words.oracle, p, min_terms, max_span)
                .unwrap()
                .1
        });
        let (name_truth, name_expensive) =
            prefix_docs(&names.oracle, &key, min_terms, max_span).unwrap();

        let mut results: Vec<Vec<u64>> = Vec::new();
        for (label, files, exact, prefix, expensive) in [
            ("words", &words, &tokens[..], partial.as_deref(), false),
            (
                "words + table",
                &words,
                &tokens[..],
                partial.as_deref(),
                word_expensive,
            ),
            ("names", &names, &[][..], Some(key.as_slice()), false),
            (
                "names + table",
                &names,
                &[][..],
                Some(key.as_slice()),
                name_expensive,
            ),
        ] {
            let before = files.stats();
            let (mut top, from_table) =
                complete(&files.index, exact, prefix, k, limits, expensive).unwrap();
            if from_table {
                top = if label.starts_with("names") {
                    name_truth.iter().copied().take(k).collect()
                } else {
                    let p = partial.as_deref().unwrap();
                    prefix_docs(&words.oracle, p, min_terms, max_span)
                        .unwrap()
                        .0
                        .into_iter()
                        .take(k)
                        .collect()
                };
            }
            let looked = files.stats();
            block_on(files.index.payload_span(&top)).unwrap();
            let done = files.stats();

            let tally = tallies
                .entry(class.clone())
                .or_default()
                .entry(label)
                .or_default();
            tally.reads.push(u64::from(looked.0 - before.0));
            tally.bytes.push(looked.1 - before.1);
            tally.payload_reads += u64::from(done.0 - looked.0);
            tally.table += u64::from(from_table);
            tally.empty += u64::from(top.is_empty());
            results.push(top);
        }
        // Both files number the same places the same way, so their answers
        // can be compared by ID.
        let shared = results[0].iter().filter(|d| results[2].contains(d)).count() as u64;
        let o = overlap.entry(class.clone()).or_default();
        o.0 += shared;
        o.1 += results[0].len().max(results[2].len()) as u64;
    }

    println!(
        "top {k}; a prefix is in the table at {min_terms} terms or {max_span} bytes of postings\n"
    );
    for (class, strategies) in &mut tallies {
        let n = strategies.values().next().map_or(0, |t| t.reads.len());
        println!("## {class}, {n} queries\n");
        println!(
            "{:<20} {:>5} {:>5} {:>6} {:>9} {:>10} {:>11} {:>7} {:>6} {:>6}",
            "", "rd50", "rd99", "rdmax", "B p50", "B p99", "B max", "pl rd", "table", "empty"
        );
        for (label, tally) in strategies.iter_mut() {
            tally.row(label);
        }
        let (shared, of) = overlap[class];
        println!(
            "\nwords and names agree on {:.0}% of their results\n",
            100.0 * shared as f64 / of.max(1) as f64
        );
    }
}
