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

/// The address demo in miniature: a hierarchy of keys, one term per document,
/// answered by prefix rather than by intersection.
const ADDRESSES: &str = "\
東京都千代田区丸の内一丁目1番地\t35.6810,139.7670\n\
東京都千代田区丸の内一丁目2番地\t35.6811,139.7671\n\
東京都千代田区丸の内二丁目1番地\t35.6820,139.7680\n\
東京都千代田区大手町一丁目1番地\t35.6870,139.7660\n\
大阪府大阪市中央区本町一丁目1番地\t34.6820,135.5010\n";

#[test]
fn an_address_index_answers_by_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    let index = dir.path().join("addresses.colex");
    std::fs::write(&corpus, ADDRESSES).unwrap();

    let output = colex(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "identity",
        "--terms-per-block",
        "2",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = colex(&["prefix", index.to_str().unwrap(), "東京都千代田区丸の内"]);
    assert!(output.status.success());
    let printed = stdout(&output);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 3, "got {lines:?}");
    assert!(lines[0].starts_with("東京都千代田区丸の内一丁目1番地\t35.6810"));
    assert!(lines[2].starts_with("東京都千代田区丸の内二丁目1番地"));
}

#[test]
fn a_prefix_matching_nothing_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    let index = dir.path().join("addresses.colex");
    std::fs::write(&corpus, ADDRESSES).unwrap();
    colex(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "identity",
    ]);

    let output = colex(&["prefix", index.to_str().unwrap(), "京都府"]);
    assert!(output.status.success());
    assert!(stdout(&output).trim().is_empty());
}

#[test]
fn the_tokenizer_is_recorded_in_the_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    let index = dir.path().join("addresses.colex");
    std::fs::write(&corpus, ADDRESSES).unwrap();
    colex(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "identity",
    ]);

    // Provenance is only repeatable if the analyzer is named, since the same
    // corpus tokenized differently is a different index.
    let output = colex(&["verify", index.to_str().unwrap()]);
    assert!(stdout(&output).contains("tokenizer       identity"));
}

#[test]
fn build_rejects_an_unregistered_tokenizer() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    std::fs::write(&corpus, ADDRESSES).unwrap();

    let output = colex(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.colex").to_str().unwrap(),
        "--tokenizer",
        "kuromoji",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("TOKENIZERS.md"));
}
