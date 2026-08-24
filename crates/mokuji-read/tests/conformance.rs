//! The conformance suite (`SPEC.md` §7).
//!
//! Each case asserts the query result **and the number of range reads it took**.
//! The I/O bound is the property the format exists to provide, so it is checked
//! the way correctness is; a change that quietly adds a fetch fails here.
//!
//! The fixtures were built by `conformance/fixtures/build_fixtures.py`, an
//! independent implementation written from the specification. This test is
//! therefore a comparison between two readings of the document, not a
//! round-trip through one implementation.

use std::path::PathBuf;

use mokuji_format::{parse_section_table, Header, SectionId};
use mokuji_read::{block_on::block_on, CountingReader, Index, SliceReader};
use serde::Deserialize;

#[derive(Deserialize)]
struct Suite {
    case: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    fixture: String,
    query: Vec<String>,
    docs: Vec<u64>,
    payloads: Vec<String>,
    reads: u32,
}

fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

fn load(fixture: &str) -> Vec<u8> {
    let path = conformance_dir()
        .join("fixtures")
        .join(format!("{fixture}.mokuji"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn suite() -> Suite {
    let path = conformance_dir().join("cases.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    toml::from_str(&text).expect("cases.toml")
}

#[test]
fn every_case_returns_the_expected_result_within_the_expected_reads() {
    let suite = suite();
    assert!(!suite.case.is_empty(), "no cases were loaded");

    for case in &suite.case {
        let reader = CountingReader::new(SliceReader::new(load(&case.fixture)));
        let index = block_on(Index::open(&reader)).expect(&case.name);
        // Opening is accounted for separately: SPEC.md §4 charges the resident
        // set to a cold start, not to a query.
        reader.reset();

        let query: Vec<&[u8]> = case.query.iter().map(|t| t.as_bytes()).collect();
        let matches = block_on(index.search(&query)).expect(&case.name);

        let docs: Vec<u64> = matches.iter().map(|m| m.doc).collect();
        let payloads: Vec<String> = matches.iter().map(|m| m.payload.to_string()).collect();

        assert_eq!(docs, case.docs, "{}: documents", case.name);
        assert_eq!(payloads, case.payloads, "{}: payloads", case.name);
        assert_eq!(reader.reads(), case.reads, "{}: read count", case.name);

        // The case files record what this reader actually costs; the bound is
        // what the specification promises. Both are checked, since a case could
        // otherwise be updated to match a regression.
        let bound = 2 * case.query.len() as u32 + case.docs.len() as u32;
        assert!(
            reader.reads() <= bound,
            "{}: {} reads exceeds the bound of {bound}",
            case.name,
            reader.reads()
        );
    }
}

#[test]
fn opening_the_file_costs_one_read() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    block_on(Index::open(&reader)).unwrap();
    assert_eq!(reader.reads(), 1);
}

#[test]
fn the_header_carries_the_statistics_scoring_needs() {
    let reader = SliceReader::new(load("tiny"));
    let index = block_on(Index::open(reader)).unwrap();
    let header = index.header();

    // The format supplies N and the mean document length and stops there;
    // applying them is outside the specification (SPEC.md §1).
    assert_eq!(header.num_docs, 4);
    assert_eq!(header.avg_doclen(), Some(2.0));
}

#[test]
fn a_term_before_every_block_costs_no_read_at_all() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();

    // `aomori` sorts before `chiyoda`, the first boundary key, so the resident
    // index settles it without touching the file.
    assert!(block_on(index.lookup(b"aomori")).unwrap().is_none());
    assert_eq!(reader.reads(), 0);
}

#[test]
fn doc_freq_and_the_postings_run_must_agree() {
    let mut bytes = load("tiny");
    let header = Header::parse(&bytes).unwrap();
    let start = header.section_table_offset as usize;
    let end = start + header.section_table_len as usize;
    let sections = parse_section_table(&bytes[start..end], header.section_count).unwrap();
    let blocks = sections
        .iter()
        .find(|s| s.id == SectionId::TERM_BLOCKS)
        .unwrap();

    // The first block is `entry_count`, `first_term_id`, then the first entry:
    // a front-coded key of `chiyoda` in nine bytes, then its `doc_freq`.
    let doc_freq = blocks.offset as usize + 2 + 2 + 7;
    assert_eq!(bytes[doc_freq], 1);
    // Claiming two documents for a run holding one must be caught rather than
    // trusted: the dictionary and the postings are separate sections, and
    // nothing but this check ties them together.
    bytes[doc_freq] = 2;

    let index = block_on(Index::open(SliceReader::new(bytes))).unwrap();
    assert!(block_on(index.search(&[b"chiyoda"])).is_err());
}

#[test]
fn a_prefix_range_within_one_block_costs_one_read() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();

    // `chiyoda` and `chuo` share a block, and the next block begins with
    // `osaka`, which the resident index shows cannot extend the range. So the
    // second block is never fetched.
    let matches = block_on(index.prefix(b"ch", 100)).unwrap();
    let terms: Vec<&[u8]> = matches.iter().map(|(term, _)| term.as_slice()).collect();

    assert_eq!(terms, [b"chiyoda".as_slice(), b"chuo".as_slice()]);
    assert_eq!(reader.reads(), 1);
}

#[test]
fn a_prefix_matching_nothing_costs_at_most_one_read() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();

    assert!(block_on(index.prefix(b"kyo", 100)).unwrap().is_empty());
    assert!(reader.reads() <= 1);
}

#[test]
fn a_prefix_matching_everything_reads_every_block() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();

    let matches = block_on(index.prefix(b"", 100)).unwrap();
    assert_eq!(matches.len(), 5);
    // Three blocks of two terms. Reading all of them is the honest cost of
    // asking for all of them.
    assert_eq!(reader.reads(), 3);
}

#[test]
fn a_limit_stops_the_scan_early() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();

    let matches = block_on(index.prefix(b"", 2)).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(reader.reads(), 1);
}

#[test]
fn a_prefix_range_reads_its_postings_in_one_request() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();

    let matches = block_on(index.prefix(b"ch", 100)).unwrap();
    let entries: Vec<_> = matches.iter().map(|(_, entry)| *entry).collect();
    reader.reset();

    // Postings are written in term order, so a contiguous range of terms has a
    // contiguous range of runs. Two terms, one read.
    let postings = block_on(index.postings_span(&entries)).unwrap();
    assert_eq!(reader.reads(), 1);
    assert_eq!(postings[0].docs, [0]);
    assert_eq!(postings[1].docs, [1, 3]);
}
