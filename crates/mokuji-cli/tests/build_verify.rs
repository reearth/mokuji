//! End-to-end tests of the command line tool.
//!
//! These run the real binary rather than calling into it, because what is being
//! checked includes the exit status: a `verify` that prints a complaint and
//! exits zero is worse than one that does not run at all, since it would pass
//! in a pipeline.

use std::path::Path;
use std::process::{Command, Output};

use mokuji_format::{parse_section_table, Header, SectionId};

const CORPUS: &str = "tokyo chiyoda\t35.694,139.754\n\
                      tokyo chuo\t35.671,139.772\n\
                      tokyo shibuya\t35.664,139.698\n\
                      osaka chuo\t34.681,135.513\n";

fn mokuji(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mokuji"))
        .args(args)
        .output()
        .expect("running mokuji")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn build_into(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let corpus = dir.join("corpus.tsv");
    let index = dir.join("index.mokuji");
    std::fs::write(&corpus, CORPUS).unwrap();

    let output = mokuji(&[
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

    let output = mokuji(&["search", index.to_str().unwrap(), "tokyo", "chuo"]);
    assert!(output.status.success());

    // Document, score, payload: the index carries document lengths, so the
    // sample scorer ranks rather than returning them in document order.
    let line = stdout(&output);
    let fields: Vec<&str> = line.trim().split('\t').collect();
    assert_eq!(fields.len(), 3, "got {line:?}");
    assert_eq!(fields[0], "1");
    assert!(
        fields[1].parse::<f64>().is_ok(),
        "score was {:?}",
        fields[1]
    );
    assert_eq!(fields[2], "35.671,139.772");
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

    let output = mokuji(&[
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

    let output = mokuji(&[
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
    let output = mokuji(&["verify", index.to_str().unwrap()]);
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

    let output = mokuji(&["verify", index.to_str().unwrap()]);
    assert!(!output.status.success(), "verify accepted a corrupted run");
}

#[test]
fn verify_rejects_a_truncated_file() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_into(dir.path());

    let mut bytes = std::fs::read(&index).unwrap();
    bytes.truncate(bytes.len() - 8);
    std::fs::write(&index, &bytes).unwrap();

    let output = mokuji(&["verify", index.to_str().unwrap()]);
    assert!(!output.status.success(), "verify accepted a truncated file");
}

#[test]
fn build_rejects_a_corpus_without_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("bad.tsv");
    std::fs::write(&corpus, "no tab on this line\n").unwrap();

    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.mokuji").to_str().unwrap(),
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
    let index = dir.path().join("addresses.mokuji");
    std::fs::write(&corpus, ADDRESSES).unwrap();

    let output = mokuji(&[
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

    let output = mokuji(&["prefix", index.to_str().unwrap(), "東京都千代田区丸の内"]);
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
    let index = dir.path().join("addresses.mokuji");
    std::fs::write(&corpus, ADDRESSES).unwrap();
    mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "identity",
    ]);

    let output = mokuji(&["prefix", index.to_str().unwrap(), "京都府"]);
    assert!(output.status.success());
    assert!(stdout(&output).trim().is_empty());
}

#[test]
fn the_tokenizer_is_recorded_in_the_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    let index = dir.path().join("addresses.mokuji");
    std::fs::write(&corpus, ADDRESSES).unwrap();
    mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "identity",
    ]);

    // Provenance is only repeatable if the analyzer is named, since the same
    // corpus tokenized differently is a different index.
    let output = mokuji(&["verify", index.to_str().unwrap()]);
    assert!(stdout(&output).contains("tokenizer       identity"));
}

#[test]
fn build_rejects_an_unregistered_tokenizer() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("addresses.tsv");
    std::fs::write(&corpus, ADDRESSES).unwrap();

    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.mokuji").to_str().unwrap(),
        "--tokenizer",
        "kuromoji",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("TOKENIZERS.md"));
}

#[test]
fn build_records_the_register_a_corpus_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    let register = dir.path().join("register.csv");
    let index = dir.path().join("index.mokuji");
    std::fs::write(&corpus, CORPUS).unwrap();
    std::fs::write(
        &register,
        "the source of truth this corpus was converted from\n",
    )
    .unwrap();

    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--source",
        register.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // An index converted from a published register should name the register,
    // not only the intermediate file the builder happened to read.
    let output = mokuji(&[
        "verify",
        index.to_str().unwrap(),
        "--source",
        register.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("matches the recorded provenance"));

    // And the corpus it read is still recorded, so both links in the chain are
    // checkable.
    let output = mokuji(&[
        "verify",
        index.to_str().unwrap(),
        "--source",
        corpus.to_str().unwrap(),
    ]);
    assert!(output.status.success());
}

#[test]
fn an_application_parameter_is_recorded_in_the_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    let index = dir.path().join("index.mokuji");
    std::fs::write(&corpus, CORPUS).unwrap();
    mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--meta",
        "geohash-precisions=6,7",
    ]);

    // The format has no opinion about what an application needs to know to
    // query an index it did not build. It only has to carry it.
    let output = mokuji(&["verify", index.to_str().unwrap()]);
    assert!(stdout(&output).contains("attr            geohash-precisions = 6,7"));
}

#[test]
fn build_rejects_a_meta_entry_that_is_not_a_pair() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    std::fs::write(&corpus, CORPUS).unwrap();

    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.mokuji").to_str().unwrap(),
        "--meta",
        "precisions",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("<key>=<value>"));
}

#[test]
fn application_metadata_does_not_change_a_file_that_declares_none() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    std::fs::write(&corpus, CORPUS).unwrap();
    let plain = dir.path().join("plain.mokuji");
    let again = dir.path().join("again.mokuji");
    mokuji(&["build", corpus.to_str().unwrap(), plain.to_str().unwrap()]);
    mokuji(&["build", corpus.to_str().unwrap(), again.to_str().unwrap()]);

    // An empty map would be a difference in the metadata of every index ever
    // built, for a feature almost none of them use.
    assert_eq!(
        std::fs::read(&plain).unwrap(),
        std::fs::read(&again).unwrap()
    );
}

/// Builds CORPUS into an index and returns its path, with the directory that
/// has to outlive it.
fn searchable() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    let index = dir.path().join("index.mokuji");
    std::fs::write(&corpus, CORPUS).unwrap();
    mokuji(&["build", corpus.to_str().unwrap(), index.to_str().unwrap()]);
    (dir, index)
}

#[test]
fn any_takes_the_union_where_the_default_takes_the_intersection() {
    let (_dir, index) = searchable();
    let path = index.to_str().unwrap();

    // No document holds both, so an intersection is empty.
    let strict = mokuji(&["search", path, "shibuya", "osaka"]);
    assert_eq!(stdout(&strict).lines().count(), 0);

    // A union finds both documents. This is what an n-gram query needs: one
    // character wrong and the intersection is empty for a term that is right.
    let loose = mokuji(&["search", path, "shibuya", "osaka", "--any"]);
    let lines = stdout(&loose);
    assert!(
        lines.contains("35.664,139.698"),
        "expected the shibuya record"
    );
    assert!(
        lines.contains("34.681,135.513"),
        "expected the osaka record"
    );
}

#[test]
fn any_ignores_a_term_that_is_not_in_the_dictionary() {
    let (_dir, index) = searchable();
    let path = index.to_str().unwrap();

    // A misspelling contributes nothing rather than emptying the result, which
    // is the whole point of scoring a union.
    let output = mokuji(&["search", path, "shibuya", "shibuyaa", "--any"]);
    assert!(stdout(&output).contains("35.664,139.698"));
}

#[test]
fn probe_fetches_postings_for_only_the_rarest_terms() {
    let (_dir, index) = searchable();
    let path = index.to_str().unwrap();

    // `tokyo` names three documents and `osaka` one; asked for one term, the
    // rarer is the one worth reading.
    let output = mokuji(&["search", path, "tokyo", "osaka", "--any", "--probe", "1"]);
    let report = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(report.contains("probed:   osaka:1"), "{report}");
    assert!(report.contains("skipped:  1 term"), "{report}");
    assert!(stdout(&output).contains("34.681,135.513"));
    assert!(!stdout(&output).contains("35.664,139.698"));
}

#[test]
fn probe_needs_a_union() {
    let (_dir, index) = searchable();
    let output = mokuji(&["search", index.to_str().unwrap(), "tokyo", "--probe", "1"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--any"));
}

#[test]
fn probe_and_phrase_are_incompatible() {
    let (_dir, index) = searchable();
    let output = mokuji(&[
        "search",
        index.to_str().unwrap(),
        "tokyo",
        "--any",
        "--probe",
        "1",
        "--phrase",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("in order"));
}

#[test]
fn an_application_section_rides_along_without_moving_anything() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    std::fs::write(&corpus, CORPUS).unwrap();
    let plain = dir.path().join("plain.mokuji");
    let carried = dir.path().join("carried.mokuji");
    let blob = dir.path().join("quantizer.bin");
    std::fs::write(&blob, b"a coarse quantizer, or whatever else").unwrap();

    mokuji(&["build", corpus.to_str().unwrap(), plain.to_str().unwrap()]);
    mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        carried.to_str().unwrap(),
        "--section",
        &format!("0x8000={}", blob.to_str().unwrap()),
    ]);

    let listing = stdout(&mokuji(&["info", carried.to_str().unwrap()]));
    assert!(listing.contains("0x8000 application"), "{listing}");

    // Attached last, so the file it was attached to is a prefix of this one.
    let a = std::fs::read(&plain).unwrap();
    let b = std::fs::read(&carried).unwrap();
    assert!(b.len() > a.len());
    assert!(b.ends_with(b"a coarse quantizer, or whatever else"));

    // And the index still reads.
    let found = stdout(&mokuji(&["search", carried.to_str().unwrap(), "tokyo"]));
    assert_eq!(found.lines().count(), 3);
    assert!(stdout(&mokuji(&["verify", carried.to_str().unwrap()])).contains("ok"));
}

#[test]
fn a_specified_section_cannot_be_supplied_by_an_application() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    let blob = dir.path().join("blob.bin");
    std::fs::write(&corpus, CORPUS).unwrap();
    std::fs::write(&blob, b"not yours").unwrap();

    // 0x0020 is POSTINGS. Letting an application write one would let it
    // contradict the index it is attached to.
    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.mokuji").to_str().unwrap(),
        "--section",
        &format!("0x0020={}", blob.to_str().unwrap()),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("application"));
}

const RANKED: &str = "tokyo chiyoda\tA\t10\n\
                      tokyo chuo\tB\t900\n\
                      tokyo shibuya\tC\t50\n\
                      osaka chuo\tD\t900\n";

#[test]
fn order_by_numbers_documents_by_a_column_highest_first() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    let index = dir.path().join("index.mokuji");
    std::fs::write(&corpus, RANKED).unwrap();
    let build = |out: &Path| {
        mokuji(&[
            "build",
            corpus.to_str().unwrap(),
            out.to_str().unwrap(),
            "--attribute",
            "pop",
            "--order-by",
            "pop",
        ])
    };
    assert!(build(&index).status.success());

    // A term's run is in document order, which is now the order of the column:
    // B, then C, then A. The tie between B and D keeps the corpus order.
    let found = stdout(&mokuji(&["search", index.to_str().unwrap(), "tokyo"]));
    let docs: Vec<&str> = found
        .lines()
        .take(3)
        .map(|line| line.split('\t').next_back().unwrap())
        .collect();
    assert_eq!(docs, ["B", "C", "A"]);
    let chuo = stdout(&mokuji(&["search", index.to_str().unwrap(), "chuo"]));
    assert!(chuo.starts_with("0\t") && chuo.contains("\n1\t"), "{chuo}");

    let again = dir.path().join("again.mokuji");
    assert!(build(&again).status.success());
    assert_eq!(
        std::fs::read(&index).unwrap(),
        std::fs::read(&again).unwrap()
    );
}

#[test]
fn order_by_needs_a_declared_column() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("corpus.tsv");
    std::fs::write(&corpus, RANKED).unwrap();
    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        dir.path().join("out.mokuji").to_str().unwrap(),
        "--attribute",
        "pop",
        "--order-by",
        "population",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a declared attribute"));
}

/// Places under all their names, `name-list` style: U+001F between names.
const NAMES: &str = "Black Sea\u{1f}Černé moře\tBlack Sea\n\
                     Black Forest\u{1f}Schwarzwald\tBlack Forest\n\
                     Blackpool\tBlackpool\n\
                     Bled\tBled\n\
                     Tokyo\u{1f}東京\tTokyo\n";

fn names_index(dir: &Path, completions: &str) -> std::path::PathBuf {
    let corpus = dir.join("names.tsv");
    let index = dir.join("names.mokuji");
    std::fs::write(&corpus, NAMES).unwrap();
    let output = mokuji(&[
        "build",
        corpus.to_str().unwrap(),
        index.to_str().unwrap(),
        "--tokenizer",
        "name-list",
        "--completions",
        completions,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    index
}

#[test]
fn complete_answers_an_expensive_prefix_from_the_table() {
    let dir = tempfile::tempdir().unwrap();
    // `b`, `bl` and `bla`... are shared by at least three names.
    let index = names_index(dir.path(), "3,0,2");

    let output = mokuji(&["complete", index.to_str().unwrap(), "BL", "-n", "2"]);
    let out = stdout(&output);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(out, "0\tBlack Sea\n1\tBlack Forest\n");
    assert!(err.contains("from the completions section"), "{err}");
    assert!(err.contains("0 before the payload"), "{err}");

    // `black s` is two names' worth, so it is cheap and walks the dictionary.
    let output = mokuji(&["complete", index.to_str().unwrap(), "Black S"]);
    assert_eq!(stdout(&output), "0\tBlack Sea\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("so it is cheap"));

    let verified = mokuji(&["verify", index.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", stdout(&verified));
    assert!(stdout(&verified).contains("completions     "));
}

#[test]
fn verify_rejects_completions_that_break_their_promises() {
    let dir = tempfile::tempdir().unwrap();
    let built = std::fs::read(names_index(dir.path(), "3,0,2")).unwrap();
    let sections = |bytes: &[u8]| {
        let header = Header::parse(bytes).unwrap();
        let start = header.section_table_offset as usize;
        parse_section_table(
            &bytes[start..start + header.section_table_len as usize],
            header.section_count,
        )
        .unwrap()
    };
    let meta = sections(&built)
        .into_iter()
        .find(|s| s.id == SectionId::META)
        .map(|s| built[s.offset as usize..(s.offset + s.length) as usize].to_vec())
        .unwrap();

    // The same file stored raw, so that a promise can be broken by one byte.
    let corpus =
        mokuji_cli::corpus::parse(NAMES, mokuji_cli::corpus::Tokenizer::NameList, 0, false)
            .unwrap();
    let mut builder = mokuji_write::Builder::new(mokuji_write::Options {
        compress: false,
        completions: Some(mokuji_write::Completions {
            min_terms: 3,
            max_span: 0,
            docs_per_entry: 2,
        }),
        ..mokuji_write::Options::default()
    })
    .unwrap();
    for payload in &corpus.payloads {
        builder.add_document(payload.clone()).unwrap();
    }
    for (term, docs) in &corpus.postings {
        for (&doc, &freq) in docs {
            builder.add_posting(term, doc, freq).unwrap();
        }
    }
    let raw = builder.finish(&meta).unwrap();
    let at = sections(&raw)
        .into_iter()
        .find(|s| s.id == SectionId::COMPLETIONS)
        .expect("a completions section")
        .offset as usize;
    let check = |bytes: &[u8], name: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        mokuji(&["verify", path.to_str().unwrap()])
    };
    assert!(check(&raw, "raw.mokuji").status.success());

    // Claiming that two terms are enough makes `black ` expensive — two names
    // begin with it — and it has no entry.
    let mut claims_more = raw.clone();
    assert_eq!(claims_more[at], 3, "min_terms leads the section");
    claims_more[at] = 2;
    let output = check(&claims_more, "more.mokuji");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("has no completion entry"));

    // The first entry is `b`, listing documents 0 and 1: after the four
    // header varints, a key of 0, 1, `b`, a count of 2, then zigzag deltas.
    // Making the second delta 4 lists Tokyo, which no `b` term names.
    let mut stray = raw.clone();
    assert_eq!(&stray[at + 4..at + 10], &[0, 1, b'b', 2, 0, 2]);
    stray[at + 9] = 8;
    let output = check(&stray, "stray.mokuji");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("which holds no term beginning with it")
    );
}
