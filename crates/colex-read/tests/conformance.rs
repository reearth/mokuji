//! The conformance suite (`SPEC.md` §7).
//!
//! Each case asserts the query result **and the number of range reads it took**.
//! The I/O bound is the property the format exists to provide, so it is checked
//! the way correctness is; a change that quietly adds a fetch fails here.
//!
//! The fixtures were built by `conformance/fixtures/build_tiny.py`, an
//! independent implementation written from the specification. This test is
//! therefore a comparison between two readings of the document, not a
//! round-trip through one implementation.

use std::path::PathBuf;

use colex_read::{block_on::block_on, CountingReader, Index, SliceReader};
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
        .join(format!("{fixture}.colex"));
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
        let payloads: Vec<String> = matches
            .iter()
            .map(|m| String::from_utf8(m.payload.clone()).expect("fixture payloads are text"))
            .collect();

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
    // Term block 0 begins at offset 600; the first entry's doc_freq is the
    // twelfth byte of it. Claiming two documents for a run holding one must be
    // caught rather than trusted.
    assert_eq!(bytes[611], 1);
    bytes[611] = 2;

    let index = block_on(Index::open(SliceReader::new(bytes))).unwrap();
    assert!(block_on(index.search(&[b"chiyoda"])).is_err());
}
