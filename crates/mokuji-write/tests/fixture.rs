//! The writer is held to the fixture, not the other way round.
//!
//! `conformance/fixtures/tiny.mokuji` was built by an independent Python
//! implementation written from `SPEC.md`. If this writer reproduces it byte for
//! byte, the two implementations agree on the layout down to the last varint —
//! which is a far stronger statement than a round-trip through this workspace
//! could make.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mokuji_format::{parse_section_table, Header, SectionId};
use mokuji_write::{Builder, Options};

/// The parameters the fixture was built with, recorded in its metadata.
fn options() -> Options {
    Options {
        terms_per_block: 2,
        docs_per_payload_block: 2,
        has_freqs: true,
        payload: mokuji_write::PayloadEncoding::Opaque,
        // The fixture is stored raw so that its hex dump can be read against
        // the specification by hand.
        compress: false,
    }
}

fn conformance(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .join(name)
}

/// The corpus, as `text<TAB>payload` per line.
fn corpus() -> Vec<(String, String)> {
    std::fs::read_to_string(conformance("tiny.corpus.txt"))
        .expect("corpus")
        .lines()
        .map(|line| {
            let (text, payload) = line.split_once('\t').expect("corpus line");
            (text.to_string(), payload.to_string())
        })
        .collect()
}

/// The fixture's metadata section, taken verbatim.
///
/// Metadata carries the provenance of a source the writer never sees, so it is
/// composed by the caller. Copying it here keeps the comparison about the
/// layout rather than about CBOR.
fn meta_of(file: &[u8]) -> Vec<u8> {
    let header = Header::parse(file).unwrap();
    let start = header.section_table_offset as usize;
    let end = start + header.section_table_len as usize;
    let sections = parse_section_table(&file[start..end], header.section_count).unwrap();
    let meta = sections
        .iter()
        .find(|s| s.id == SectionId::META)
        .expect("meta section");
    file[meta.offset as usize..(meta.offset + meta.length) as usize].to_vec()
}

fn build(meta: &[u8]) -> Vec<u8> {
    let mut builder = Builder::new(options()).unwrap();

    // Postings are pushed in term order, so the caller sorts. The writer owns
    // no sort: a corpus larger than memory is sorted outside it.
    let mut postings: BTreeMap<String, BTreeMap<u64, u64>> = BTreeMap::new();
    for (text, payload) in corpus() {
        let doc = builder.add_document(payload.as_bytes()).unwrap();
        for token in text.split(' ') {
            *postings
                .entry(token.to_string())
                .or_default()
                .entry(doc)
                .or_default() += 1;
        }
    }
    for (term, docs) in &postings {
        for (&doc, &freq) in docs {
            builder.add_posting(term.as_bytes(), doc, freq).unwrap();
        }
    }
    builder.finish(meta).unwrap()
}

#[test]
fn reproduces_the_conformance_fixture_byte_for_byte() {
    let expected = std::fs::read(conformance("tiny.mokuji")).expect("fixture");
    let built = build(&meta_of(&expected));

    assert_eq!(
        built.len(),
        expected.len(),
        "length differs: built {} bytes, fixture {}",
        built.len(),
        expected.len()
    );
    if let Some(at) = built.iter().zip(&expected).position(|(a, b)| a != b) {
        panic!(
            "first difference at offset {at}: built {:#04x}, fixture {:#04x}",
            built[at], expected[at]
        );
    }
}

#[test]
fn building_twice_produces_identical_bytes() {
    let meta = meta_of(&std::fs::read(conformance("tiny.mokuji")).unwrap());
    assert_eq!(build(&meta), build(&meta));
}

#[test]
fn rejects_terms_out_of_order() {
    let mut builder = Builder::new(options()).unwrap();
    builder.add_document(b"payload".to_vec()).unwrap();
    builder.add_posting(b"tokyo", 0, 1).unwrap();
    assert!(builder.add_posting(b"osaka", 0, 1).is_err());
}

#[test]
fn rejects_documents_out_of_order_within_a_term() {
    let mut builder = Builder::new(options()).unwrap();
    builder.add_document(b"a".to_vec()).unwrap();
    builder.add_document(b"b".to_vec()).unwrap();
    builder.add_posting(b"tokyo", 1, 1).unwrap();
    assert!(builder.add_posting(b"tokyo", 0, 1).is_err());
}

#[test]
fn rejects_a_posting_for_a_document_that_does_not_exist() {
    let mut builder = Builder::new(options()).unwrap();
    builder.add_document(b"a".to_vec()).unwrap();
    assert!(builder.add_posting(b"tokyo", 7, 1).is_err());
}

#[test]
fn rejects_a_zero_frequency() {
    let mut builder = Builder::new(options()).unwrap();
    builder.add_document(b"a".to_vec()).unwrap();
    assert!(builder.add_posting(b"tokyo", 0, 0).is_err());
}
