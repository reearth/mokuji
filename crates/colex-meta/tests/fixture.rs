//! Checks this canonical CBOR encoder against the one in the Python fixture
//! builder.
//!
//! Two independent encoders producing the same bytes for the same content is
//! the only way to know that "canonical" means the same thing in both. If they
//! disagreed, a rebuild would differ from the original while both files were
//! perfectly valid — which is precisely the failure `SPEC.md` §3.4 exists to
//! prevent.

use std::path::PathBuf;

use colex_format::{parse_section_table, Header, SectionId};
use colex_meta::{ContentHash, Meta};

fn fixture_meta() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/fixtures/tiny.colex");
    let file = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    let header = Header::parse(&file).unwrap();
    let start = header.section_table_offset as usize;
    let end = start + header.section_table_len as usize;
    let sections = parse_section_table(&file[start..end], header.section_count).unwrap();
    let meta = sections
        .iter()
        .find(|s| s.id == SectionId::META)
        .expect("meta section");
    file[meta.offset as usize..(meta.offset + meta.length) as usize].to_vec()
}

#[test]
fn re_encoding_the_fixture_metadata_reproduces_it_byte_for_byte() {
    let bytes = fixture_meta();
    let meta = Meta::parse(&bytes).expect("fixture metadata");

    let rebuilt = meta.encode();
    assert_eq!(
        rebuilt.len(),
        bytes.len(),
        "length differs: rebuilt {} bytes, fixture {}",
        rebuilt.len(),
        bytes.len()
    );
    if let Some(at) = rebuilt.iter().zip(&bytes).position(|(a, b)| a != b) {
        panic!("first difference at offset {at}");
    }
}

#[test]
fn the_fixture_records_where_it_came_from() {
    let meta = Meta::parse(&fixture_meta()).unwrap();

    assert_eq!(meta.generation, 1);
    assert_eq!(meta.tokenizer.id, "space-split");
    assert_eq!(meta.generator.name, "build_fixtures.py");

    let source = &meta.sources[0];
    assert_eq!(source.content_hash.algorithm, ContentHash::SHA2_256);
    assert_eq!(source.content_hash.hex.len(), 64);
    assert_eq!(
        source.uri.as_deref(),
        Some("conformance/fixtures/tiny.corpus.txt")
    );
}

#[test]
fn the_build_parameters_are_recorded() {
    let meta = Meta::parse(&fixture_meta()).unwrap();

    // SPEC.md §3.4 requires the parameters to be sufficient to rebuild the
    // file, so the block sizes a reader can observe must be among them.
    assert_eq!(
        meta.generator
            .params
            .get("terms_per_block")
            .unwrap()
            .as_uint(),
        Some(2)
    );
    assert_eq!(
        meta.generator
            .params
            .get("docs_per_payload_block")
            .unwrap()
            .as_uint(),
        Some(2)
    );
}
