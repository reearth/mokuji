//! What numbering documents by importance buys a reader, and what it costs.
//!
//! Takes two builds of one corpus — in corpus order, and with `--order-by` —
//! and a file of queries, one per line. Reports each file's sections, then what
//! answering every query for its top `k` costs in each:
//!
//! - `full`: read every term's whole run, intersect, rank by the prior column.
//!   What a reader has to do when document order says nothing about rank.
//! - `head`: on the ordered file only. Read the first `--head` bytes of each
//!   run, which hold its most important documents, and fall back to the rest
//!   of a run only when the heads cannot prove the top `k`.
//!
//! Then how many dictionary blocks a short prefix spans, which is what
//! completing a word costs before a single match is ranked.
//!
//!     importance-order corpus.mokuji ordered.mokuji <prior> queries.txt [-k 10] [--head 1024]

use std::collections::BTreeMap;

use importance_order::{decode_head, FileReader};
use mokuji::cli::corpus::Tokenizer;
use mokuji::format::{Flags, Result};
use mokuji::read::block_on::block_on;
use mokuji::read::{postings, CountingReader, Index, RangeReader, TermEntry, TermIndex};

type Reader = CountingReader<FileReader>;

/// What answering the query set cost, summed.
#[derive(Default)]
struct Cost {
    queries: u64,
    answered: u64,
    reads: u64,
    dictionary_bytes: u64,
    postings_reads: u64,
    postings_bytes: Vec<u64>,
    payload_reads: u64,
    payload_bytes: u64,
    fallbacks: u64,
}

/// Reader counts at the four points a query passes: before it, after the
/// dictionary, after the postings, after the payload.
type Marks = [(u32, u64); 4];

impl Cost {
    fn add(&mut self, [start, looked_up, ranked, done]: Marks, found: bool) {
        self.queries += 1;
        self.answered += u64::from(found);
        self.reads += u64::from(done.0 - start.0);
        self.dictionary_bytes += looked_up.1 - start.1;
        self.postings_reads += u64::from(ranked.0 - looked_up.0);
        self.postings_bytes.push(ranked.1 - looked_up.1);
        self.payload_reads += u64::from(done.0 - ranked.0);
        self.payload_bytes += done.1 - ranked.1;
    }

    fn print(&mut self, label: &str) {
        let q = self.queries.max(1) as f64;
        let postings = &mut self.postings_bytes;
        postings.sort_unstable();
        let pct = |p: f64| {
            let at = ((postings.len() as f64 - 1.0) * p).round() as usize;
            postings.get(at).copied().unwrap_or(0)
        };
        println!(
            "{label:<18} {:>6.2} {:>9.0} {:>7.2} {:>9.0} {:>8} {:>9} {:>7.2} {:>8.0} {:>8}",
            self.reads as f64 / q,
            self.dictionary_bytes as f64 / q,
            self.postings_reads as f64 / q,
            postings.iter().sum::<u64>() as f64 / q,
            pct(0.5),
            pct(0.99),
            self.payload_reads as f64 / q,
            self.payload_bytes as f64 / q,
            self.fallbacks,
        );
    }
}

fn stats(reader: &Reader) -> (u32, u64) {
    let s = reader.stats();
    (s.reads, s.bytes)
}

fn intersect(lists: &[Vec<u64>]) -> Vec<u64> {
    let mut lists: Vec<&Vec<u64>> = lists.iter().collect();
    lists.sort_by_key(|l| l.len());
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .copied()
        .filter(|d| rest.iter().all(|l| l.binary_search(d).is_ok()))
        .collect()
}

/// Terms of a query, deduplicated, as the index was built.
fn terms_of(query: &str) -> Vec<Vec<u8>> {
    let mut terms: Vec<Vec<u8>> = Tokenizer::Bigram
        .split(query)
        .into_iter()
        .map(String::into_bytes)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Every run read whole, then ranked by the prior. Returns the top `k` in
/// ascending document order.
fn full(
    index: &Index<&Reader>,
    reader: &Reader,
    prior: &[i64],
    terms: &[&[u8]],
    k: usize,
    looked_up: &mut (u32, u64),
) -> Result<Vec<u64>> {
    let entries = block_on(index.lookup_many(terms))?;
    *looked_up = stats(reader);
    let Some(entries) = entries.into_iter().collect::<Option<Vec<TermEntry>>>() else {
        return Ok(Vec::new());
    };
    let mut lists = Vec::with_capacity(entries.len());
    for entry in &entries {
        lists.push(block_on(index.postings(entry))?.docs);
    }
    let mut matched = intersect(&lists);
    // Highest prior first; ties by document ID, which is corpus order in both
    // files, so the two strategies are answering the same question.
    matched.sort_by_key(|&d| (std::cmp::Reverse(prior[d as usize]), d));
    matched.truncate(k);
    matched.sort_unstable();
    Ok(matched)
}

/// The heads of the runs, and the rest only when the heads cannot settle it.
fn head(
    index: &Index<&Reader>,
    reader: &Reader,
    terms: &[&[u8]],
    k: usize,
    head_bytes: u64,
    fallbacks: &mut u64,
    looked_up: &mut (u32, u64),
) -> Result<Vec<u64>> {
    let entries = block_on(index.lookup_many(terms))?;
    *looked_up = stats(reader);
    let Some(entries) = entries.into_iter().collect::<Option<Vec<TermEntry>>>() else {
        return Ok(Vec::new());
    };
    let mut lists = Vec::with_capacity(entries.len());
    let mut heads = Vec::with_capacity(entries.len());
    let mut complete = Vec::with_capacity(entries.len());
    for entry in &entries {
        if entry.postings_len == 0 {
            lists.push(block_on(index.postings(entry))?.docs);
            heads.push(Vec::new());
            complete.push(true);
            continue;
        }
        let len = entry.postings_len.min(head_bytes);
        let bytes = block_on(reader.read_range(entry.postings_offset, len as u32))?;
        let (docs, whole) = decode_head(&bytes, entry.postings_len)?;
        lists.push(docs);
        heads.push(bytes);
        complete.push(whole);
    }

    // A document at or below every incomplete list's last decoded ID is known
    // to be in or out of each list, since each is complete up to its own last.
    let bound = lists
        .iter()
        .zip(&complete)
        .filter(|(_, &whole)| !whole)
        .map(|(docs, _)| docs.last().copied().unwrap_or(0))
        .min();
    let matched = intersect(&lists);
    let settled: Vec<u64> = match bound {
        None => matched,
        Some(bound) => matched.into_iter().filter(|&d| d <= bound).collect(),
    };
    if bound.is_none() || settled.len() >= k {
        return Ok(settled.into_iter().take(k).collect());
    }

    // Not enough: fetch the rest of each incomplete run, one read each, and
    // decode it joined to the head already in hand.
    *fallbacks += 1;
    let has_freqs = index.header().flags.contains(Flags::HAS_FREQS);
    for (i, entry) in entries.iter().enumerate() {
        if complete[i] {
            continue;
        }
        let have = heads[i].len() as u64;
        let rest = block_on(reader.read_range(
            entry.postings_offset + have,
            (entry.postings_len - have) as u32,
        ))?;
        let mut run = std::mem::take(&mut heads[i]);
        run.extend_from_slice(&rest);
        lists[i] = postings::decode(&run, has_freqs, index.header().num_docs)?.docs;
    }
    Ok(intersect(&lists).into_iter().take(k).collect())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: importance-order <corpus.mokuji> <ordered.mokuji> <prior> <queries.txt> [-k n] [--head bytes]");
        std::process::exit(2);
    }
    let mut k = 10usize;
    let mut head_bytes = 1024u64;
    let mut rest = &args[5..];
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "-k" => k = value.parse().unwrap(),
            "--head" => head_bytes = value.parse().unwrap(),
            other => panic!("unknown option {other}"),
        }
        rest = tail;
    }
    let queries: Vec<String> = std::fs::read_to_string(&args[4])
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    let plain_reader = CountingReader::new(FileReader::open(&args[1]));
    let ordered_reader = CountingReader::new(FileReader::open(&args[2]));
    let plain = block_on(Index::open(&plain_reader)).unwrap();
    let ordered = block_on(Index::open(&ordered_reader)).unwrap();
    let prior_of = |index: &Index<&Reader>| {
        index
            .column(&args[3])
            .unwrap_or_else(|| panic!("no column '{}'", args[3]))
            .values
            .clone()
    };
    let plain_prior = prior_of(&plain);
    let ordered_prior = prior_of(&ordered);

    println!("## sections\n");
    println!(
        "{:<18} {:>14} {:>14} {:>8}",
        "section", "corpus order", "by importance", "ratio"
    );
    let mut rows: BTreeMap<u16, (String, u64, u64)> = BTreeMap::new();
    for (i, index) in [&plain, &ordered].into_iter().enumerate() {
        for s in index.sections() {
            let row = rows
                .entry(s.id.0)
                .or_insert_with(|| (s.id.name().unwrap_or("application").to_string(), 0, 0));
            if i == 0 {
                row.1 = s.length;
            } else {
                row.2 = s.length;
            }
        }
    }
    let (mut a, mut b) = (0, 0);
    for (name, x, y) in rows.values() {
        a += x;
        b += y;
        let ratio = if *x == 0 { 0.0 } else { *y as f64 / *x as f64 };
        println!("{name:<18} {x:>14} {y:>14} {ratio:>8.3}");
    }
    println!(
        "{:<18} {:>14} {:>14} {:>8.3}\n",
        "file",
        plain_reader.file_len(),
        ordered_reader.file_len(),
        b as f64 / a as f64
    );

    println!(
        "## top {k} of {} queries, {head_bytes}-byte heads\n",
        queries.len()
    );
    println!(
        "{:<18} {:>6} {:>9} {:>7} {:>9} {:>8} {:>9} {:>7} {:>8} {:>8}",
        "", "reads", "dict B", "pst rd", "pst B", "pst p50", "pst p99", "pl rd", "pl B", "fallback"
    );
    let mut costs = [Cost::default(), Cost::default(), Cost::default()];
    let mut disagreements = 0;
    for query in &queries {
        let owned = terms_of(query);
        let terms: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
        if terms.is_empty() {
            continue;
        }

        let mut m: Marks = [stats(&plain_reader); 4];
        let top = full(&plain, &plain_reader, &plain_prior, &terms, k, &mut m[1]).unwrap();
        m[2] = stats(&plain_reader);
        let plain_payload = block_on(plain.payload_span(&top)).unwrap();
        m[3] = stats(&plain_reader);
        costs[0].add(m, !top.is_empty());

        let mut m: Marks = [stats(&ordered_reader); 4];
        let top_full = full(
            &ordered,
            &ordered_reader,
            &ordered_prior,
            &terms,
            k,
            &mut m[1],
        )
        .unwrap();
        m[2] = stats(&ordered_reader);
        let _ = block_on(ordered.payload_span(&top_full)).unwrap();
        m[3] = stats(&ordered_reader);
        costs[1].add(m, !top_full.is_empty());

        let mut m: Marks = [stats(&ordered_reader); 4];
        let mut fallbacks = 0;
        let top_head = head(
            &ordered,
            &ordered_reader,
            &terms,
            k,
            head_bytes,
            &mut fallbacks,
            &mut m[1],
        )
        .unwrap();
        m[2] = stats(&ordered_reader);
        let ordered_payload = block_on(ordered.payload_span(&top_head)).unwrap();
        m[3] = stats(&ordered_reader);
        costs[2].add(m, !top_head.is_empty());
        costs[2].fallbacks += fallbacks;

        // Both files should name the same places, in the same rank order.
        let names = |p: &[mokuji::read::Payload]| {
            let mut v: Vec<String> = p.iter().map(|x| x.to_string()).collect();
            v.sort();
            v
        };
        if top_full != top_head || names(&plain_payload) != names(&ordered_payload) {
            disagreements += 1;
        }
    }
    costs[0].print("corpus order, full");
    costs[1].print("importance, full");
    costs[2].print("importance, head");
    println!(
        "\nanswered {} of {}; results differing between strategies: {disagreements}\n",
        costs[0].answered, costs[0].queries
    );

    println!("## dictionary blocks a prefix spans\n");
    let index: TermIndex<'_> = plain.term_index().unwrap();
    let keys = index.keys().unwrap();
    println!("{} blocks in all\n", keys.len());
    println!(
        "{:<8} {:>8} {:>8} {:>8} {:>8}",
        "chars", "p50", "p90", "p99", "max"
    );
    for chars in 1..=4 {
        let mut spans: Vec<usize> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for query in &queries {
            let Some(word) = Tokenizer::Bigram.split(query).into_iter().next() else {
                continue;
            };
            let prefix: String = word.chars().take(chars).collect();
            if prefix.chars().count() < chars || !seen.insert(prefix.clone()) {
                continue;
            }
            let p = prefix.as_bytes();
            spans.push(1 + keys.iter().filter(|key| key.starts_with(p)).count());
        }
        spans.sort_unstable();
        let pct = |q: f64| spans[((spans.len() as f64 - 1.0) * q).round() as usize];
        if spans.is_empty() {
            continue;
        }
        println!(
            "{chars:<8} {:>8} {:>8} {:>8} {:>8}",
            pct(0.5),
            pct(0.9),
            pct(0.99),
            spans.last().unwrap()
        );
    }
}
