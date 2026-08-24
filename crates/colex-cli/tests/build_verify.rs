//! End-to-end tests of the command line tool.
//!
//! These run the real binary rather than calling into it, because what is being
//! checked includes the exit status: a `verify` that prints a complaint and
//! exits zero is worse than one that does not run at all, since it would pass
//! in a pipeline.

use std::path::Path;
use std::process::{Command, Output};

use colex_format::{parse_section_table, Header, SectionId};

const CORPUS: &str = "tokyo chiyoda\t35.694,139.754\n\
                      tokyo chuo\t35.671,139.772\n\
                      tokyo shibuya\t35.664,139.698\n\
                      osaka chuo\t34.681,135.513\n";

fn colex(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_colex"))
        .args(args)
        .output()
        .expect("running colex")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn build_into(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let corpus = dir.join("corpus.tsv");
    let index = dir.join("index.colex");
    std::fs::write(&corpus, CORPUS).unwrap();

    let output = colex(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--terms-per-block",
        "2",
        "--docs-per-payload-block",
        "2",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (corpus, index)
}

#[test]
fn builds_an_index_that_can_be_searched() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());

    let output = colex(&["search", index.to_str().unwrap(), "tokyo", "chuo"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "1\t35.671,139.772");
}

#[test]
fn building_twice_produces_identical_files() {
    let dir = tempfile::tempdir().unwrap();
    let (_, first) = build_into(dir.path());
    let bytes = std::fs::read(&first).unwrap();

    let second = tempfile::tempdir().unwrap();
    let (_, second) = build_into(second.path());

    // The paths differ, and the recorded URI with them, so only the index
    // proper is compared: everything after the metadata section.
    let header = Header::parse(&bytes).unwrap();
    let start = header.section_table_offset as usize;
    let end = start + header.section_table_len as usize;
    let sections = parse_section_table(&bytes[start..end], header.section_count).unwrap();
    let meta = sections.iter().find(|s| s.id == SectionId::META).unwrap();
    let after_meta = (meta.offset + meta.length) as usize;

    let other = std::fs::read(&second).unwrap();
    assert_eq!(bytes.len(), other.len());
    assert_eq!(bytes[after_meta..], other[after_meta..]);
}

#[test]
fn verify_accepts_the_source_it_was_built_from() {
    let dir = tempfile::tempdir().unwrap();
    let (corpus, index) = build_into(dir.path());

    let output = colex(&[
        "verify",
        index.to_str().unwrap(),
        "--source",
        corpus.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("matches the recorded provenance"));
}

#[test]
fn verify_rejects_a_source_the_index_did_not_come_from() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());
    let other = dir.path().join("other.tsv");
    std::fs::write(&other, "kyoto\t35.011,135.768\n").unwrap();

    let output = colex(&[
        "verify",
        index.to_str().unwrap(),
        "--source",
        other.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "verify accepted the wrong source");
}

#[test]
fn verify_reports_when_provenance_was_not_checked() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());

    // Verifying structure without a source is useful, but must not read as a
    // provenance check that passed.
    let output = colex(&["verify", index.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("provenance not checked"));
}

#[test]
fn verify_rejects_a_corrupted_postings_run() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());

    let mut bytes = std::fs::read(&index).unwrap();
    let header = Header::parse(&bytes).unwrap();
    let start = header.section_table_offset as usize;
    let end = start + header.section_table_len as usize;
    let sections = parse_section_table(&bytes[start..end], header.section_count).unwrap();
    let postings = sections
        .iter()
        .find(|s| s.id == SectionId::POSTINGS)
        .unwrap();
    // The codec byte of the first run. A query that never touches this term
    // would not notice; verify reads every run, which is the point of it.
    bytes[postings.offset as usize] = 0xff;
    std::fs::write(&index, &bytes).unwrap();

    let output = colex(&["verify", index.to_str().unwrap()]);
    assert!(!output.status.success(), "verify accepted a corrupted run");
}

#[test]
fn verify_rejects_a_truncated_file() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());

    let mut bytes = std::fs::read(&index).unwrap();
    bytes.truncate(bytes.len() - 8);
    std::fs::write(&index, &bytes).unwrap();

    let output = colex(&["verify", index.to_str().unwrap()]);
    assert!(!output.status.success(), "verify accepted a truncated file");
}

#[test]
fn build_rejects_a_corpus_without_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("bad.tsv");
    std::fs::write(&corpus, "no tab on this line\n").unwrap();

    let output = colex(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.colex").to_str().unwrap(),
    ]);
    assert!(!output.status.success());
}
