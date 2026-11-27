//! `MokujiIndex` against the conformance fixtures, asked the same questions
//! `js/test/mokuji.test.js` asks the TypeScript reader, with the same answers
//! expected: the two are one API in two languages, and the Python reader is a
//! third.

use mokuji::{block_on, MokujiIndex, SearchOptions, SliceReader};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("conformance/fixtures")
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

    // A probe of zero reads no postings, so nothing is matched, and both
    // present terms are skipped.
    // An intersection is where it panicked, indexing the first of no runs; a
    // union of none was already empty.
    for any in [false, true] {
        let zero = SearchOptions {
            any,
            probe: Some(0),
            ..SearchOptions::default()
        };
        let found = block_on(tiny.search("tokyo chuo", &zero)).unwrap();
        assert_eq!((found.matched, found.skipped), (0, 2));
    }

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

fn docs(found: &mokuji::SearchResult) -> Vec<u64> {
    found.results.iter().map(|r| r.doc).collect()
}

#[test]
fn a_phrase_holds_its_terms_where_they_were_typed() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let phrase = SearchOptions {
        phrase: true,
        ..SearchOptions::default()
    };
    let ask = |query: &str, options: &SearchOptions| block_on(tiny.search(query, options)).unwrap();
    assert_eq!(docs(&ask("tokyo chuo", &phrase)), [1]);
    assert!(docs(&ask("chuo tokyo", &phrase)).is_empty());
    assert_eq!(docs(&ask("osaka chuo", &phrase)), [3]);

    // `probe` reads the rarer term alone; a phrase of one term is the term.
    let probed = SearchOptions {
        probe: Some(1),
        ..phrase.clone()
    };
    let plain = SearchOptions {
        probe: Some(1),
        ..SearchOptions::default()
    };
    let found = ask("tokyo chuo", &probed);
    assert_eq!(
        found
            .probed
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>(),
        ["chuo"]
    );
    assert_eq!(docs(&found), [1, 3]);
    assert_eq!(docs(&found), docs(&ask("tokyo chuo", &plain)));

    // A union admits documents missing a term, and no phrase holds them.
    let any = SearchOptions {
        any: true,
        ..phrase.clone()
    };
    assert_eq!(docs(&ask("tokyo chuo", &any)), [1]);

    let dict = block_on(MokujiIndex::open_file(fixture("dict"))).unwrap();
    let refused = block_on(dict.search("東京都", &phrase)).unwrap_err();
    assert!(refused
        .to_string()
        .contains("this index carries no positions, so it cannot test a phrase"));
}

#[test]
fn a_filter_leaves_documents_out_before_their_positions_or_payload() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let not_one: mokuji::DocFilter = std::sync::Arc::new(|doc| doc != 1);
    let filtered = SearchOptions {
        filter: Some(not_one.clone()),
        ..SearchOptions::default()
    };
    let phrase = SearchOptions {
        phrase: true,
        filter: Some(not_one),
        ..SearchOptions::default()
    };
    tiny.reset_stats();
    let without = block_on(tiny.search("tokyo chuo", &filtered)).unwrap();
    let reads = tiny.stats().reads;
    tiny.reset_stats();
    let with = block_on(tiny.search("tokyo chuo", &phrase)).unwrap();
    assert_eq!((without.matched, with.matched), (0, 0));
    // Nothing survived to be tested, so no positions were read.
    assert_eq!(tiny.stats().reads, reads);
}

#[test]
fn rank_is_search_without_the_payload() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let options = SearchOptions::default();
    let found = block_on(tiny.search("tokyo chuo", &options)).unwrap();
    tiny.reset_stats();
    let terms = ["tokyo".to_string(), "chuo".to_string()];
    let ranking = block_on(mokuji::rank(tiny.index(), &terms, None, &options)).unwrap();
    let ranked: Vec<(u64, Option<f64>)> = found.results.iter().map(|r| (r.doc, r.score)).collect();
    assert_eq!(ranking.ranked, ranked);
    // The dictionary block and the postings, and no payload block.
    let without_payload = tiny.stats().reads;
    tiny.reset_stats();
    block_on(tiny.search("tokyo chuo", &options)).unwrap();
    assert!(without_payload < tiny.stats().reads);
}

#[test]
fn a_completion_says_what_it_read_before_the_payload() {
    let names = block_on(MokujiIndex::open_file(fixture("names"))).unwrap();
    names.reset_stats();
    let found = block_on(names.complete("t", 2)).unwrap();
    assert!(found.from_table);
    assert_eq!(found.requests_before_payload, 0);
    assert_eq!(names.stats().reads, 1);
}

#[test]
fn reports_every_request() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tiny.set_on_request(Some(Box::new(move |info| log.lock().unwrap().push(info))));
    tiny.reset_stats();
    block_on(tiny.search("tokyo chuo", &SearchOptions::default())).unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len() as u32, tiny.stats().reads);
    assert_eq!(
        seen.iter().map(|r| r.length).sum::<u64>(),
        tiny.stats().bytes
    );
}

#[test]
fn an_open_file_can_move_to_another_thread() {
    fn send<T: Send>() {}
    send::<MokujiIndex<mokuji::FileReader>>();
    send::<mokuji::MokujiCollection<mokuji::FileReader>>();
}

#[test]
fn a_term_typed_twice_in_a_phrase_reads_its_positions_once() {
    let tiny = block_on(MokujiIndex::open_file(fixture("tiny"))).unwrap();
    let options = |phrase| SearchOptions {
        phrase,
        ..SearchOptions::default()
    };
    let plain = block_on(tiny.search("chuo chuo", &options(false))).unwrap();
    tiny.reset_stats();
    let found = block_on(tiny.search("chuo chuo", &options(true))).unwrap();
    // `chuo` is never next to itself.
    assert_eq!((plain.matched, found.matched), (2, 0));
    // Its dictionary block, with the postings inline, and its positions once;
    // nothing matched, so no payload.
    assert_eq!(tiny.stats().reads, 2);
}
