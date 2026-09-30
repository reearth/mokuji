//! Collections against the conformance suite (`conformance/cases.toml`), the
//! same cases `build_fixtures.py` and `mokuji.js` answer to.

use std::path::PathBuf;

use mokuji_manifest::{Collection, Manifest};
use mokuji_read::{block_on::block_on, CountingReader, SliceReader};
use serde::Deserialize;

#[derive(Deserialize)]
struct Suite {
    collection: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    manifest: String,
    prefix: String,
    limit: usize,
    segments: Vec<usize>,
    docs: Vec<u64>,
    payloads: Vec<String>,
    reads: u32,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/fixtures")
}

#[test]
fn every_collection_case_holds() {
    let text = std::fs::read_to_string(fixtures().join("../cases.toml")).unwrap();
    let suite: Suite = toml::from_str(&text).unwrap();
    assert!(!suite.collection.is_empty());

    for case in &suite.collection {
        let manifest = Manifest::parse(
            &std::fs::read_to_string(fixtures().join(format!("{}.json", case.manifest))).unwrap(),
        )
        .unwrap();
        let readers: Vec<CountingReader<SliceReader>> = manifest
            .segments
            .iter()
            .map(|s| {
                CountingReader::new(SliceReader::new(
                    std::fs::read(fixtures().join(&s.path)).unwrap(),
                ))
            })
            .collect();
        let collection = block_on(Collection::open(&manifest, readers.iter().collect())).unwrap();
        readers.iter().for_each(CountingReader::reset);

        let found = block_on(collection.complete(case.prefix.as_bytes(), case.limit)).unwrap();
        let payloads: Vec<String> = block_on(collection.payloads(&found.hits))
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        let segments: Vec<usize> = found.hits.iter().map(|h| h.segment).collect();
        let docs: Vec<u64> = found.hits.iter().map(|h| h.doc).collect();
        assert_eq!(segments, case.segments, "{}: segments", case.name);
        assert_eq!(docs, case.docs, "{}: documents", case.name);
        assert_eq!(payloads, case.payloads, "{}: payloads", case.name);
        let reads: u32 = readers.iter().map(CountingReader::reads).sum();
        assert_eq!(reads, case.reads, "{}: reads", case.name);
    }
}

#[test]
fn the_manifest_describes_its_fixtures() {
    let manifest =
        Manifest::parse(&std::fs::read_to_string(fixtures().join("collection.json")).unwrap())
            .unwrap();
    for segment in &manifest.segments {
        let bytes = std::fs::read(fixtures().join(&segment.path)).unwrap();
        assert_eq!(bytes.len() as u64, segment.length, "{}", segment.path);
    }
}
