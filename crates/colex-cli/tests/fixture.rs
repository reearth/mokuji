//! Reads the conformance fixture, which was built by an independent
//! implementation from `SPEC.md` alone.
//!
//! This is the first point where the two implementations meet. A test built on
//! bytes this workspace produced itself would only show that it is
//! self-consistent.

use std::path::PathBuf;

use colex_format::{parse_section_table, Flags, Header, SectionId, HEADER_LEN};
use colex_read::{block_on::block_on, CountingReader, RangeReader, SliceReader};

fn fixture() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/fixtures/tiny.colex");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn header_matches_the_corpus_the_fixture_was_built_from() {
    let header = Header::parse(&fixture()).unwrap();

    assert_eq!(header.spec_major, 0);
    assert_eq!(header.spec_minor, 1);
    assert_eq!(header.num_docs, 4);
    assert_eq!(header.sum_doclen, 8);
    assert_eq!(header.avg_doclen(), Some(2.0));
    assert!(header.flags.contains(Flags::HAS_PAYLOAD));
    assert!(header.flags.contains(Flags::HAS_FREQS));
    assert!(!header.flags.contains(Flags::TERMDICT_MONOLITHIC));
}

#[test]
fn every_section_is_present_and_within_the_file() {
    let bytes = fixture();
    let file_len = bytes.len() as u64;
    let header = Header::parse(&bytes).unwrap();
    let table_start = header.section_table_offset as usize;
    let table_end = table_start + header.section_table_len as usize;
    let sections =
        parse_section_table(&bytes[table_start..table_end], header.section_count).unwrap();

    let ids: Vec<SectionId> = sections.iter().map(|s| s.id).collect();
    assert_eq!(
        ids,
        vec![
            SectionId::META,
            SectionId::TERM_INDEX,
            SectionId::TERM_BLOCKS,
            SectionId::TERM_BLOCK_OFFSETS,
            SectionId::POSTINGS,
            SectionId::PAYLOAD,
            SectionId::PAYLOAD_OFFSETS,
        ]
    );
    for section in &sections {
        section.check_bounds(file_len).unwrap();
    }
}

#[test]
fn the_resident_set_precedes_the_blocks_it_indexes() {
    let bytes = fixture();
    let header = Header::parse(&bytes).unwrap();
    let table_start = header.section_table_offset as usize;
    let table_end = table_start + header.section_table_len as usize;
    let sections =
        parse_section_table(&bytes[table_start..table_end], header.section_count).unwrap();
    let at = |id: SectionId| sections.iter().find(|s| s.id == id).unwrap().offset;

    // The recommended order exists so that a cold start can fetch the whole
    // resident set in one range read; a fixture that did not honour it would
    // make the claim in SPEC.md §4 untestable.
    let resident_end = [
        SectionId::META,
        SectionId::TERM_INDEX,
        SectionId::TERM_BLOCK_OFFSETS,
        SectionId::PAYLOAD_OFFSETS,
    ]
    .map(at)
    .into_iter()
    .max()
    .unwrap();
    for id in [
        SectionId::TERM_BLOCKS,
        SectionId::POSTINGS,
        SectionId::PAYLOAD,
    ] {
        assert!(
            at(id) > resident_end,
            "{id:?} is not after the resident set"
        );
    }
}

#[test]
fn the_resident_set_is_one_contiguous_range() {
    let bytes = fixture();
    let header = Header::parse(&bytes).unwrap();
    let table_start = header.section_table_offset as usize;
    let table_end = table_start + header.section_table_len as usize;
    let sections =
        parse_section_table(&bytes[table_start..table_end], header.section_count).unwrap();
    let at = |id: SectionId| sections.iter().find(|s| s.id == id).unwrap();

    // A cold start costs one read only if everything resident lies in an
    // unbroken stretch from offset zero. Measure that stretch by reading it,
    // and check that each resident section falls inside what came back.
    let prefix = at(SectionId::TERM_BLOCKS).offset;
    let reader = CountingReader::new(SliceReader::new(bytes));
    let resident = block_on(reader.read_range(0, prefix as u32)).unwrap();

    assert_eq!(reader.reads(), 1);
    for id in [
        SectionId::META,
        SectionId::TERM_INDEX,
        SectionId::TERM_BLOCK_OFFSETS,
        SectionId::PAYLOAD_OFFSETS,
    ] {
        let section = at(id);
        assert!(
            section.offset + section.length <= resident.len() as u64,
            "{id:?} is not covered by the prefix read"
        );
    }

    // Deliberately not asserted here: that the resident set is a small
    // fraction of the file. At four documents it is most of it, since the
    // provenance hash alone outweighs the index. That ratio is a property of
    // scale, and belongs to a benchmark on real data rather than to a fixture.
}
