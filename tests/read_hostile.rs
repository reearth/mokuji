//! A Mokuji file is untrusted input.
//!
//! The whole proposition is that a stranger's file is fetched from object
//! storage and read in a page, so every offset, length, and count in it is
//! attacker-controlled. `SPEC.md` §8 requires a reader to fail cleanly rather
//! than panic on any of it. This checks that against a large number of damaged
//! files derived from a valid one.
//!
//! Deterministic rather than random: the same mutations run every time, so a
//! failure is reproducible from the test name alone. `fuzz/` goes further with
//! a real fuzzer; this is what runs on every commit.

use std::path::PathBuf;

use mokuji::read::{block_on::block_on, Index, OpenOptions, SliceReader};

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/fixtures/tiny.mokuji");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Opens and exercises a file as far as it will go, ignoring errors.
///
/// The result does not matter. What matters is returning at all: a panic here
/// is a reader that would take down the page it runs in.
fn exercise(bytes: Vec<u8>) {
    let reader = SliceReader::new(bytes);
    let Ok(index) = block_on(Index::open_with(
        &reader,
        OpenOptions {
            resident_dictionary: true,
        },
    )) else {
        return;
    };

    let _ = index.header().avg_doclen();
    let _ = index.meta();
    let _ = block_on(index.search(&[b"chiyoda", b"tokyo"]));
    let _ = block_on(index.search(&[b""]));

    if let Ok(matches) = block_on(index.prefix(b"", 1000)) {
        let entries: Vec<_> = matches.iter().map(|(_, entry)| entry.clone()).collect();
        let _ = block_on(index.postings_span(&entries));
        for (_, entry) in &matches {
            let _ = block_on(index.postings(entry));
        }
    }
    for doc in 0..index.header().num_docs.min(64) {
        let _ = block_on(index.read_payload(doc));
    }
    if let Ok(term_index) = index.term_index() {
        let _ = term_index.keys();
        for ordinal in 0..term_index.block_count().min(64) {
            let _ = block_on(index.read_term_block(ordinal));
        }
    }
}

#[test]
fn survives_every_single_byte_being_wrong() {
    let original = fixture();
    // Every byte, flipped to three different values. The header, the section
    // table, and every offset table are covered exhaustively.
    for at in 0..original.len() {
        for delta in [1u8, 0x80, 0xff] {
            let mut bytes = original.clone();
            bytes[at] = bytes[at].wrapping_add(delta);
            exercise(bytes);
        }
    }
}

#[test]
fn survives_truncation_at_every_length() {
    let original = fixture();
    for len in 0..original.len() {
        exercise(original[..len].to_vec());
    }
}

#[test]
fn survives_every_byte_being_zeroed_or_maxed() {
    let original = fixture();
    for at in 0..original.len() {
        for value in [0u8, 0xff] {
            let mut bytes = original.clone();
            bytes[at] = value;
            exercise(bytes);
        }
    }
}

#[test]
fn survives_a_valid_header_over_nonsense() {
    // The header and section table say the right things; everything they point
    // at is garbage. This is the shape a reader is most likely to trust.
    let original = fixture();
    let table_end = 64 + 7 * 24;
    for filler in [0u8, 0x7f, 0xff] {
        let mut bytes = original.clone();
        bytes[table_end..].fill(filler);
        exercise(bytes);
    }
}

#[test]
fn survives_extreme_lengths_in_the_header() {
    let original = fixture();
    // Counts and offsets at their extremes, which is where an allocation or a
    // multiplication would overflow if one were going to.
    for at in [12usize, 16, 24, 32, 40] {
        for width in [4usize, 8] {
            if at + width > 64 {
                continue;
            }
            let mut bytes = original.clone();
            bytes[at..at + width].fill(0xff);
            exercise(bytes);
        }
    }
}

#[test]
fn survives_extreme_offsets_in_the_section_table() {
    let original = fixture();
    for entry in 0..7usize {
        for field in [8usize, 16] {
            let mut bytes = original.clone();
            let at = 64 + entry * 24 + field;
            bytes[at..at + 8].fill(0xff);
            exercise(bytes);
            let mut bytes = original.clone();
            bytes[at..at + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
            exercise(bytes);
        }
    }
}

#[test]
fn survives_a_file_of_nothing() {
    for len in [0usize, 1, 8, 63, 64, 65, 1024] {
        exercise(vec![0; len]);
        exercise(vec![0xff; len]);
    }
    // A valid magic and nothing else is the most tempting input of all.
    let mut bytes = b"MOKUJI\0\0".to_vec();
    bytes.resize(64, 0);
    exercise(bytes);
}
