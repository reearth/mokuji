//! Reads the conformance fixture, which was built by an independent
//! implementation from `SPEC.md` alone.
//!
//! This is the first point where the two implementations meet. A test built on
//! bytes this workspace produced itself would only show that it is
//! self-consistent.

use std::path::PathBuf;

use mokuji_format::{parse_section_table, Flags, Header, SectionId};
use mokuji_read::{block_on::block_on, CountingReader, RangeReader, SliceReader};

fn fixture() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/fixtures/tiny.mokuji");
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
            // Optional, and present here because the fixture is a document
            // collection rather than a dictionary.
            SectionId::DOC_LENGTHS,
            SectionId::POSITIONS,
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

/// The `dict` fixture: a dictionary index, with postings implied throughout and
/// payload stored as delta-coded integers.
///
/// The `tiny` fixture covers none of that, so without this the codecs added
/// most recently would be agreed on by nobody but themselves.
mod dictionary {
    use std::path::PathBuf;

    use mokuji_cli::payload::parse_values;
    use mokuji_format::{parse_section_table, Header, SectionId};
    use mokuji_write::{Builder, Options, PayloadEncoding};

    fn conformance(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/fixtures")
            .join(name)
    }

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

    /// Builds the dictionary index, raw or compressed.
    fn build(corpus: &str, meta: &[u8], compress: bool) -> Vec<u8> {
        let mut builder = Builder::new(Options {
            terms_per_block: 2,
            docs_per_payload_block: 2,
            has_freqs: true,
            payload: PayloadEncoding::Ints {
                values_per_doc: 2,
                scale_exponent: -6,
            },
            positions: false,
            doc_lengths: false,
            compress,
        })
        .unwrap();

        // The corpus is already in term order, which is what lets every term's
        // postings be implied.
        let rows: Vec<(&str, &str)> = corpus
            .lines()
            .map(|line| line.split_once('\t').expect("corpus line"))
            .collect();
        for (_, payload) in &rows {
            builder
                .add_document_values(&parse_values(payload, 6).unwrap())
                .unwrap();
        }
        for (doc, (term, _)) in rows.iter().enumerate() {
            builder.add_posting(term.as_bytes(), doc as u64, 1).unwrap();
        }
        // A ranking prior, which is what an attribute column is for: read for
        // many records at once, before any payload is fetched.
        builder
            .add_column(mokuji_write::Column {
                name: "importance".into(),
                scale_exponent: 0,
                values: vec![40, 90, 85, 70, 60],
            })
            .unwrap();
        builder.finish(meta).unwrap()
    }

    /// The compressed fixture, which the other implementations read but cannot
    /// reproduce.
    ///
    /// Two writers using different deflate implementations produce different,
    /// equally valid files, so byte-identity is a property of a writer rather
    /// than of the format — which is why `generator.name` and
    /// `generator.version` are in the metadata. This one is kept current by
    /// this test: `MOKUJI_REGENERATE=1 cargo test` rewrites it.
    #[test]
    fn the_compressed_fixture_is_current() {
        let path = conformance("dict-deflate.mokuji");
        let meta = meta_of(&std::fs::read(conformance("dict.mokuji")).expect("fixture"));
        let corpus = std::fs::read_to_string(conformance("dict.corpus.txt")).expect("corpus");
        let built = build(&corpus, &meta, true);

        if std::env::var_os("MOKUJI_REGENERATE").is_some() {
            std::fs::write(&path, &built).expect("write");
            return;
        }
        let committed = std::fs::read(&path).unwrap_or_default();
        assert_eq!(
            committed, built,
            "dict-deflate.mokuji is stale; re-run with MOKUJI_REGENERATE=1"
        );
    }

    #[test]
    fn reproduces_the_dictionary_fixture_byte_for_byte() {
        let expected = std::fs::read(conformance("dict.mokuji")).expect("fixture");
        let corpus = std::fs::read_to_string(conformance("dict.corpus.txt")).expect("corpus");
        // Stored raw, so the fixture can be read against the specification by
        // hand, and so that two writers can be compared byte for byte.
        let built = build(&corpus, &meta_of(&expected), false);
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
}
