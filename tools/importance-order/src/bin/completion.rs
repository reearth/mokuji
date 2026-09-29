//! What a table of the best completions for every short prefix would cost.
//!
//! A prefix of one or two characters spans hundreds of dictionary blocks at
//! planet scale, so it cannot be answered from the dictionary within a small
//! bound. The alternative is to answer it ahead of time: for every prefix up to
//! `--chars` characters long, the `k` most important documents holding a term
//! that begins with it.
//!
//! Only an expensive prefix is kept: one matching at least `--min-terms` terms,
//! which would span more than a block or two of the dictionary, or one whose
//! terms' postings together exceed `--max-span` bytes, which a reader would
//! have to fetch to find the heads among them. Any other prefix is answered
//! from the dictionary and the postings within a small bound already, and an
//! entry for it would be bytes spent on a question that was never expensive.
//! What those left out cost is reported too.
//!
//! Reads a file built with `--order-by`, where the most important documents of
//! a term are the first of its run, so a term contributes the head of its run
//! and nothing more. Builds the table, encodes it the way a dictionary block is
//! encoded, and reports its size and what one lookup in it reads.
//!
//!     completion ordered.mokuji [--chars 4] [-k 10] [--min-terms 1024] [--max-span 65536] [--entries-per-block 256]

use std::collections::HashMap;

use importance_order::{decode_head, FileReader};
use mokuji_codec::{block, front_coded, varint};
use mokuji_read::block_on::block_on;
use mokuji_read::{term, Index, RangeReader};

/// Keeps the `k` smallest document IDs offered to it.
fn offer(best: &mut Vec<u64>, docs: &[u64], k: usize) {
    for &doc in docs {
        if best.len() == k && doc >= *best.last().unwrap() {
            // Docs arrive ascending within one run; nothing later can enter.
            break;
        }
        if let Err(at) = best.binary_search(&doc) {
            best.insert(at, doc);
            best.truncate(k);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: completion <ordered.mokuji> [--chars n] [-k n]");
    let mut chars = 4usize;
    let mut min_terms = 1024u64;
    let mut max_span = 65536u64;
    let mut k = 10usize;
    let mut per_block = 256usize;
    let mut rest = &args[2..];
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "--chars" => chars = value.parse().unwrap(),
            "-k" => k = value.parse().unwrap(),
            "--min-terms" => min_terms = value.parse().unwrap(),
            "--max-span" => max_span = value.parse().unwrap(),
            "--entries-per-block" => per_block = value.parse().unwrap(),
            other => panic!("unknown option {other}"),
        }
        rest = tail;
    }

    let reader = FileReader::open(path);
    let index = block_on(Index::open(&reader)).unwrap();
    let blocks = index.term_index().unwrap().block_count();

    // One map per prefix length, from prefix to its best documents.
    let mut tables: Vec<HashMap<String, (u64, u64, Vec<u64>)>> = vec![HashMap::new(); chars];
    let mut terms = 0u64;
    for ordinal in 0..blocks {
        let bytes = block_on(index.read_term_block(ordinal)).unwrap();
        for (key, entry) in term::decode_block(&bytes, index.has_positions()).unwrap() {
            terms += 1;
            let Ok(text) = std::str::from_utf8(&key) else {
                continue;
            };
            let docs = if entry.postings_len == 0 {
                vec![entry.term_id]
            } else {
                // Enough for ten documents at any width a gap can have.
                let len = entry.postings_len.min(256) as u32;
                let head = block_on(reader.read_range(entry.postings_offset, len)).unwrap();
                decode_head(&head, entry.postings_len).unwrap().0
            };
            let docs = &docs[..docs.len().min(k)];
            let mut prefix = String::new();
            for (i, c) in text.chars().take(chars).enumerate() {
                prefix.push(c);
                let (count, span, best) = tables[i].entry(prefix.clone()).or_default();
                *count += 1;
                *span += entry.postings_len;
                offer(best, docs, k);
            }
        }
    }

    println!(
        "{terms} terms in {blocks} dictionary blocks; top {k} for each prefix of at least {min_terms} terms or {max_span} bytes of postings\n"
    );
    println!(
        "{:<6} {:>10} {:>10} {:>10} {:>7} {:>8}   {:>10} {:>10} {:>10}",
        "chars", "kept", "raw", "deflated", "blocks", "lookup", "left out", "max terms", "max span"
    );
    let mut total = 0u64;
    for (i, table) in tables.iter().enumerate() {
        let expensive = |count: u64, span: u64| count >= min_terms || span >= max_span;
        let mut entries: Vec<(&String, &Vec<u64>)> = table
            .iter()
            .filter(|(_, (count, span, _))| expensive(*count, *span))
            .map(|(prefix, (_, _, best))| (prefix, best))
            .collect();
        let left_out = table.values().filter(|(c, s, _)| !expensive(*c, *s));
        let (mut left, mut max_terms, mut max_bytes) = (0, 0, 0);
        for (count, span, _) in left_out {
            left += 1;
            max_terms = max_terms.max(*count);
            max_bytes = max_bytes.max(*span);
        }
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

        // Front-coded keys and delta-coded IDs, as a dictionary block writes
        // them; deflated per block, since a lookup reads exactly one.
        let (mut raw, mut deflated, mut block_sizes) = (0u64, 0u64, Vec::new());
        for chunk in entries.chunks(per_block) {
            let mut bytes = Vec::new();
            let mut previous: &[u8] = &[];
            for (prefix, docs) in chunk {
                front_coded::encode(prefix.as_bytes(), previous, &mut bytes);
                previous = prefix.as_bytes();
                varint::encode(docs.len() as u64, &mut bytes);
                let mut last = None;
                for &doc in docs.iter() {
                    varint::encode(last.map_or(doc, |l: u64| doc - l - 1), &mut bytes);
                    last = Some(doc);
                }
            }
            raw += bytes.len() as u64;
            let unit = block::compress(&bytes).unwrap();
            deflated += unit.len() as u64;
            block_sizes.push(unit.len() as u64);
        }
        total += deflated;
        let mean = deflated / block_sizes.len().max(1) as u64;
        println!(
            "{:<6} {:>10} {:>10} {:>10} {:>7} {:>8}   {:>10} {:>10} {:>10}",
            i + 1,
            entries.len(),
            raw,
            deflated,
            block_sizes.len(),
            mean,
            left,
            max_terms,
            max_bytes
        );
    }
    println!(
        "\nall lengths: {total} bytes, against a file of {}",
        reader.file_len()
    );
}
