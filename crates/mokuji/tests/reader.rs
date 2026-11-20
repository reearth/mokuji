//! `MokujiIndex` against the conformance fixtures, asked the same questions
//! `js/test/mokuji.test.js` asks the TypeScript reader, with the same answers
//! expected: the two are one API in two languages, and the Python reader is a
//! third.

use mokuji::{block_on, MokujiIndex, SearchOptions, SliceReader};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .join(format!("{name}.mokuji"))
}

#[test]
fn reads_a_deflated_file() {
    // The fixtures the other implementations reproduce byte for byte are
    // stored raw, so without this the decompression is never exercised.
    let index = block_on(MokujiIndex::open_file(fixture("dict-deflate"))).unwrap();
    let found = block_on(index.lookup("東京都千代田区丸の内1丁目1番1号", 10)).unwrap();
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].payload.to_string(), "139.767201,35.681560");
}

#[test]
fn a_file_on_disk_costs_what_the_same_bytes_in_memory_do() {
    let path = fixture("dict");
    let on_disk = block_on(MokujiIndex::open_file(&path)).unwrap();
    let in_memory = block_on(MokujiIndex::open(SliceReader::new(
        std::fs::read(&path).unwrap(),
    )))
    .unwrap();
    on_disk.reset_stats();
    in_memory.reset_stats();
    let a = block_on(on_disk.lookup("東京都千代田区丸の内", 100)).unwrap();
    let b = block_on(in_memory.lookup("東京都千代田区丸の内", 100)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.results.len(), 3);
    assert_eq!(on_disk.stats(), in_memory.stats());
    // Only what the query touched, not the file.
    assert!(on_disk.stats().bytes < on_disk.file_len());
}

#[test]
fn ranks_as_mokuji_search_and_the_typescript_reader_do() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let rounded = |r: &mokuji::SearchResult| -> Vec<(u64, String, String)> {
        r.results
            .iter()
            .map(|x| {
                (
                    x.doc,
                    format!("{:.3}", x.score.unwrap()),
                    x.payload.to_string(),
                )
            })
            .collect()
    };
    let row = |doc: u64, score: &str, payload: &str| (doc, score.to_string(), payload.to_string());

    let all = block_on(tiny.search("tokyo chuo", &SearchOptions::default())).unwrap();
    assert_eq!(rounded(&all), [row(1, "1.050", "35.671,139.772")]);

    let any = SearchOptions {
        any: true,
        ..SearchOptions::default()
    };
    let found = block_on(tiny.search("tokyo chuo", &any)).unwrap();
    assert_eq!(
        rounded(&found),
        [
            row(1, "1.050", "35.671,139.772"),
            row(3, "0.693", "34.681,135.513"),
            row(0, "0.357", "35.694,139.754"),
            row(2, "0.357", "35.664,139.698"),
        ]
    );

    let probed = SearchOptions {
        any: true,
        probe: Some(1),
        ..SearchOptions::default()
    };
    let found = block_on(tiny.search("tokyo chuo", &probed)).unwrap();
    assert_eq!(
        found
            .probed
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>(),
        ["chuo"]
    );
    assert_eq!(found.skipped, 1);

    let none = block_on(tiny.search("tokyo nowhere", &SearchOptions::default())).unwrap();
    assert_eq!(none.matched, 0);

    let absent = SearchOptions {
        boost: Some("absent".into()),
        ..SearchOptions::default()
    };
    assert!(block_on(tiny.search("tokyo", &absent)).is_err());
}

#[test]
fn carries_its_credit_and_normalizes_what_was_typed() {
    let names = block_on(MokujiIndex::open_file(fixture("names"))).unwrap();
    assert_eq!(names.license(), Some("CC0-1.0"));
    assert_eq!(names.attribution(), Some("Mokuji conformance fixtures"));
    assert_eq!(names.tokenizer_id(), "name-list");
    // What was typed goes through the file's tokenizer before it is looked up.
    let done = block_on(names.complete("Black\u{a0}Se", 10)).unwrap();
    assert_eq!(done.normalized, "black se");
    assert!(!done.docs.is_empty());
}
