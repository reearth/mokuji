//! Reading an arbitrary file must fail rather than crash (`SPEC.md` §8).
//!
//! The reader's whole proposition is that a stranger's file is fetched from
//! object storage and read in a page, so this is the operation that matters.

#![no_main]

use mokuji::read::{block_on::block_on, Index, OpenOptions, SliceReader};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let reader = SliceReader::new(data.to_vec());
    let Ok(index) = block_on(Index::open_with(
        &reader,
        OpenOptions {
            resident_dictionary: true,
            ..OpenOptions::default()
        },
    )) else {
        return;
    };

    let _ = index.header().avg_doclen();
    let _ = block_on(index.search(&[b"a", b"b"]));

    if let Ok(matches) = block_on(index.prefix(b"", 256)) {
        let entries: Vec<_> = matches.iter().map(|(_, entry)| entry.clone()).collect();
        let _ = block_on(index.postings_span(&entries));
    }
    for doc in 0..index.header().num_docs.min(256) {
        let _ = block_on(index.read_payload(doc));
    }
});
