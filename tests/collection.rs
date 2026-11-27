//! `MokujiCollection` against the collection fixture, asked what the other
//! readers' collections are asked, with the same answers expected.

use mokuji::manifest::{Hit, Manifest};
use mokuji::{block_on, Error, MokujiCollection};

fn fixtures() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/fixtures")
}

fn collection() -> MokujiCollection<mokuji::FileReader> {
    block_on(MokujiCollection::open_file(
        fixtures().join("collection.json"),
        None,
    ))
    .unwrap()
}

fn hits(pairs: &[(usize, u64)]) -> Vec<Hit> {
    pairs
        .iter()
        .map(|&(segment, doc)| Hit { segment, doc })
        .collect()
}

fn texts(payloads: &[mokuji::Payload]) -> Vec<String> {
    payloads.iter().map(ToString::to_string).collect()
}

#[test]
fn completes_in_collection_order_leaving_out_deletes() {
    let c = collection();
    c.reset_stats();
    let found = block_on(c.complete("Black", 10)).unwrap();
    assert_eq!(found.normalized, "black");
    assert_eq!(found.hits, hits(&[(0, 1), (0, 2), (1, 0)]));
    assert_eq!(
        texts(&found.payloads),
        ["Black Forest", "Blackpool", "Black Mountain"]
    );
    assert_eq!(
        found.consulted.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(found.requests_before_payload < c.stats().reads);

    // The entry answers, and a later segment is not asked once it is met.
    c.reset_stats();
    let t = block_on(c.complete("t", 2)).unwrap();
    assert_eq!(t.hits, hits(&[(0, 4), (0, 5)]));
    assert_eq!(t.consulted, [(0, true)]);
    assert_eq!(t.requests_before_payload, 0);
}

#[test]
fn candidates_asks_every_segment() {
    let found = block_on(collection().candidates("t", 2)).unwrap();
    assert_eq!(found.hits, hits(&[(0, 4), (0, 5), (1, 1)]));
    assert_eq!(texts(&found.payloads), ["Tokyo", "Tohoku", "Tokyo Tower"]);
    assert_eq!(
        found.consulted.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn searches_every_segment_for_every_term() {
    let c = collection();
    let tower = block_on(c.search("Tokyo Tower", 10)).unwrap();
    assert_eq!(tower.terms, ["tokyo tower"]);
    assert_eq!((tower.matched, tower.hits.clone()), (1, hits(&[(1, 1)])));
    assert_eq!(texts(&tower.payloads), ["Tokyo Tower"]);

    // Deleted from the base, and so from the collection.
    let sea = block_on(c.search("Black Sea", 10)).unwrap();
    assert_eq!((sea.matched, sea.hits.len()), (0, 0));

    let forest = block_on(c.search("Schwarzwald", 10)).unwrap();
    assert_eq!(forest.hits, hits(&[(0, 1)]));

    let limited = block_on(c.search("Tokyo", 0)).unwrap();
    assert_eq!((limited.matched, limited.hits.len()), (1, 0));
}

#[test]
fn counts_what_each_segment_read() {
    let c = collection();
    c.reset_stats();
    assert_eq!(c.stats().reads, 0);
    block_on(c.complete("black", 10)).unwrap();
    let stats = c.stats();
    // The case counts 7 block by block; the base's two payload blocks are
    // neighbours, and read as one.
    assert_eq!(stats.reads, 6);
    assert!(stats.round_trips < stats.reads);
    assert!(stats.bytes > 0);
}

#[test]
fn a_region_leaves_a_segment_unopened() {
    let c = block_on(MokujiCollection::open_file(
        fixtures().join("collection.json"),
        Some(&["FR"]),
    ))
    .unwrap();
    assert_eq!((c.inner().len(), c.manifest_segments()), (1, 2));
    let tower = block_on(c.search("Tokyo Tower", 10)).unwrap();
    assert!(tower.hits.is_empty());
}

#[test]
fn credits_each_segment_once() {
    // Both segments state the same credit.
    assert_eq!(collection().attributions(), ["Mokuji conformance fixtures"]);
}

#[test]
fn refuses_a_segment_of_another_length() {
    let mut manifest =
        Manifest::parse(&std::fs::read_to_string(fixtures().join("collection.json")).unwrap())
            .unwrap();
    manifest.segments[0].length += 1;
    let readers = manifest
        .segments
        .iter()
        .map(|s| mokuji::FileReader::open(fixtures().join(&s.path)).unwrap())
        .collect();
    assert!(matches!(
        block_on(MokujiCollection::open(&manifest, readers)),
        Err(Error::Collection(_))
    ));
}

#[test]
fn writes_the_fixture_manifest_back_byte_for_byte() {
    let text = std::fs::read_to_string(fixtures().join("collection.json")).unwrap();
    assert_eq!(Manifest::parse(&text).unwrap().to_json(), text);
}

#[test]
fn a_segment_past_the_last_is_an_error() {
    let c = collection();
    let missing = block_on(c.live_in(2, "t", 2)).unwrap_err();
    assert_eq!(missing.to_string(), "the collection has no segment 2");
    let missing = block_on(c.payloads(&hits(&[(0, 1), (5, 0)]))).unwrap_err();
    assert_eq!(missing.to_string(), "the collection has no segment 5");
}

#[test]
fn a_limit_past_counting_does_not_overflow() {
    let found = block_on(collection().complete("black", usize::MAX)).unwrap();
    assert_eq!(found.hits.len(), 3);
}
