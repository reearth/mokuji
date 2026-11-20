//! What a collection costs as it grows, and what deletes cost it.
//!
//! A base and the first `--deltas` of the delta files, with `--delete-rate` of
//! the base's documents deleted — uniformly, or among the first 100,000, the
//! most important, with `--important`. Reports what opening the collection
//! costs, how many of the base's completion entries the deletes touch, and
//! what the partly typed queries cost two ways:
//!
//! - `complete`: collection order, stopping once enough are found.
//! - `candidates`: every segment's own first ten, which ranking across
//!   segments needs.
//!
//! Reads are counted before any payload, summed over segments and, as the
//! round trips a reader asking every segment at once would wait for, the most
//! in any one segment.
//!
//!     segments base.mokuji deltas/ typed.txt [--deltas 4] [--delete-rate 0.001] [--important]

use importance_order::FileReader;
use mokuji::manifest::{Collection, Manifest, Segment, VERSION};
use mokuji::read::block_on::block_on;
use mokuji::read::{CountingReader, RangeReader};

type Counted = CountingReader<FileReader>;

#[derive(Default)]
struct Tally {
    sum: Vec<u64>,
    most: Vec<u64>,
    bytes: Vec<u64>,
    walked: u64,
}

impl Tally {
    fn add(&mut self, readers: &[Counted], walked: bool) {
        let reads: Vec<u64> = readers.iter().map(|r| u64::from(r.reads())).collect();
        self.sum.push(reads.iter().sum());
        self.most.push(reads.iter().copied().max().unwrap_or(0));
        self.bytes
            .push(readers.iter().map(|r| r.stats().bytes).sum());
        self.walked += u64::from(walked);
    }

    fn row(&mut self, label: &str) {
        let pct = |v: &mut Vec<u64>, p: f64| {
            v.sort_unstable();
            v[((v.len() as f64 - 1.0) * p).round() as usize]
        };
        println!(
            "{label:<12} {:>6} {:>6} {:>7} {:>7} {:>9} {:>10} {:>7}",
            pct(&mut self.sum, 0.5),
            pct(&mut self.sum, 0.99),
            pct(&mut self.most, 0.5),
            pct(&mut self.most, 0.99),
            pct(&mut self.bytes, 0.5),
            pct(&mut self.bytes, 0.99),
            self.walked
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (base, deltas_dir, typed) = (&args[1], &args[2], &args[3]);
    let (mut deltas, mut rate, mut important) = (0usize, 0f64, false);
    let mut rest = &args[4..];
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match flag.as_str() {
            "--deltas" => {
                deltas = rest[0].parse().unwrap();
                rest = &rest[1..];
            }
            "--delete-rate" => {
                rate = rest[0].parse().unwrap();
                rest = &rest[1..];
            }
            "--important" => important = true,
            other => panic!("unknown option {other}"),
        }
    }

    let mut paths = vec![base.clone()];
    for k in 0..deltas {
        paths.push(format!("{deltas_dir}/delta-{k:02}.mokuji"));
    }
    let lengths: Vec<u64> = paths
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .collect();

    // Deletes by a fixed-seed generator, so that a rerun deletes the same.
    let num_docs = {
        let r = FileReader::open(base);
        block_on(mokuji::read::Index::open(&r))
            .unwrap()
            .header()
            .num_docs
    };
    let pool = if important {
        num_docs.min(100_000)
    } else {
        num_docs
    };
    let wanted = (num_docs as f64 * rate).round() as u64;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut deletes = std::collections::BTreeSet::new();
    while (deletes.len() as u64) < wanted.min(pool) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        deletes.insert(state % pool);
    }

    let manifest = Manifest {
        mokuji_manifest: VERSION.into(),
        generation: 1,
        segments: paths
            .iter()
            .zip(&lengths)
            .enumerate()
            .map(|(i, (p, &length))| Segment {
                path: p.clone(),
                content_hash: format!("sha2-256:{}", "00".repeat(32)),
                length,
                deletes: if i == 0 {
                    deletes.iter().copied().collect()
                } else {
                    Vec::new()
                },
                region: None,
            })
            .collect(),
    };
    let readers: Vec<Counted> = paths
        .iter()
        .map(|p| CountingReader::new(FileReader::open(p)))
        .collect();
    let collection = block_on(Collection::open(&manifest, readers.iter().collect())).unwrap();
    let open_reads: u32 = readers.iter().map(|r| r.reads()).sum();
    let open_bytes: u64 = readers.iter().map(|r| r.stats().bytes).sum();

    let touched = collection.segment(0).completions().map_or(0, |c| {
        c.entries()
            .filter(|(_, listed)| listed.iter().any(|d| deletes.contains(d)))
            .count()
    });

    println!(
        "{} delta(s) over the base; {} of {} base documents deleted{}",
        deltas,
        deletes.len(),
        num_docs,
        if important {
            ", among the first 100,000"
        } else {
            ""
        }
    );
    println!(
        "open: {open_reads} reads, {open_bytes} bytes; completion entries touched by deletes: {touched}\n"
    );
    println!(
        "{:<12} {:>6} {:>6} {:>7} {:>7} {:>9} {:>10} {:>7}",
        "", "sum50", "sum99", "trips50", "trips99", "B p50", "B p99", "walked"
    );

    let queries: Vec<String> = std::fs::read_to_string(typed)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(_, q)| q.to_string()))
        .collect();
    let (mut complete, mut candidates) = (Tally::default(), Tally::default());
    for query in &queries {
        let key = mokuji::cli::corpus::name(query).into_bytes();

        readers.iter().for_each(CountingReader::reset);
        let found = block_on(collection.complete(&key, 10)).unwrap();
        let walked = found.consulted.iter().any(|&(s, t)| s == 0 && !t);
        complete.add(&readers, walked && base_had_entry(&collection, &key));

        readers.iter().for_each(CountingReader::reset);
        let all = block_on(collection.candidates(&key, 10)).unwrap();
        let walked = !all[0].consulted[0].1;
        candidates.add(&readers, walked && base_had_entry(&collection, &key));
    }
    complete.row("complete");
    candidates.row("candidates");
    println!("\n`walked` counts queries whose prefix has a base entry and still read the base's dictionary.");
}

fn base_had_entry<R: RangeReader>(collection: &Collection<R>, key: &[u8]) -> bool {
    collection
        .segment(0)
        .completions()
        .is_some_and(|c| c.get(key).is_some())
}
