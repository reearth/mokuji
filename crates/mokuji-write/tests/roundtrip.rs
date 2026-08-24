//! Round-trip properties: whatever is written can be read back unchanged, and
//! reading it stays within the bound `SPEC.md` §4 promises.
//!
//! Generated corpora reach shapes a hand-written fixture does not: blocks that
//! divide evenly and blocks that do not, terms that share long prefixes,
//! documents holding one term and documents holding all of them.

use std::collections::BTreeMap;

use mokuji_read::{block_on::block_on, CountingReader, Index, SliceReader};
use mokuji_write::{Builder, Options};
use proptest::prelude::*;

/// An empty CBOR map. Metadata is opaque to the writer, so a round-trip has no
/// reason to compose a real one.
const META: &[u8] = &[0xa0];

/// Terms chosen to share prefixes, so that front-coding is exercised rather
/// than merely executed.
const VOCABULARY: &[&str] = &[
    "a", "ab", "abc", "abcd", "b", "chiyoda", "chuo", "osaka", "shibuya", "tokyo", "tokyoto",
];

fn corpus_strategy() -> impl Strategy<Value = Vec<Vec<usize>>> {
    prop::collection::vec(prop::collection::vec(0..VOCABULARY.len(), 1..6), 1..12usize)
}

fn options_strategy() -> impl Strategy<Value = Options> {
    (
        1..6usize,
        1..6u32,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(terms, docs, has_freqs, doc_lengths, compress)| Options {
            terms_per_block: terms,
            docs_per_payload_block: docs,
            has_freqs,
            payload: mokuji_write::PayloadEncoding::Opaque,
            doc_lengths,
            compress,
        })
}

/// Builds the index and the answer it is supposed to give.
fn build(
    corpus: &[Vec<usize>],
    options: Options,
) -> (Vec<u8>, BTreeMap<String, BTreeMap<u64, u64>>) {
    let mut builder = Builder::new(options).unwrap();
    let mut expected: BTreeMap<String, BTreeMap<u64, u64>> = BTreeMap::new();

    for (i, tokens) in corpus.iter().enumerate() {
        let doc = builder
            .add_document(format!("payload-{i}").into_bytes())
            .unwrap();
        for &token in tokens {
            *expected
                .entry(VOCABULARY[token].to_string())
                .or_default()
                .entry(doc)
                .or_default() += 1;
        }
    }
    for (term, docs) in &expected {
        for (&doc, &freq) in docs {
            builder.add_posting(term.as_bytes(), doc, freq).unwrap();
        }
    }
    (builder.finish(META).unwrap(), expected)
}

proptest! {
    #[test]
    fn every_term_reads_back_as_it_was_written(
        corpus in corpus_strategy(),
        options in options_strategy(),
    ) {
        let (bytes, expected) = build(&corpus, options);
        let index = block_on(Index::open(SliceReader::new(bytes))).unwrap();

        prop_assert_eq!(index.header().num_docs, corpus.len() as u64);

        for (term, docs) in &expected {
            let entry = block_on(index.lookup(term.as_bytes())).unwrap();
            let entry = entry.ok_or_else(|| TestCaseError::fail(format!("{term} is missing")))?;
            prop_assert_eq!(entry.doc_freq, docs.len() as u64);

            let postings = block_on(index.postings(&entry)).unwrap();
            prop_assert_eq!(&postings.docs, &docs.keys().copied().collect::<Vec<_>>());
            if options.has_freqs {
                prop_assert_eq!(&postings.freqs, &docs.values().copied().collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn absent_terms_are_reported_absent(
        corpus in corpus_strategy(),
        options in options_strategy(),
    ) {
        let (bytes, expected) = build(&corpus, options);
        let index = block_on(Index::open(SliceReader::new(bytes))).unwrap();

        // Probes that fall before, between, and after the terms present.
        for probe in ["", "!", "abcde", "kyoto", "zzz"] {
            if expected.contains_key(probe) {
                continue;
            }
            prop_assert!(block_on(index.lookup(probe.as_bytes())).unwrap().is_none());
        }
    }

    #[test]
    fn a_search_stays_within_the_bound(
        corpus in corpus_strategy(),
        options in options_strategy(),
        picks in prop::collection::vec(0..VOCABULARY.len(), 1..4),
    ) {
        let (bytes, expected) = build(&corpus, options);
        let reader = CountingReader::new(SliceReader::new(bytes));
        let index = block_on(Index::open(&reader)).unwrap();
        reader.reset();

        let mut query: Vec<&[u8]> = picks.iter().map(|&i| VOCABULARY[i].as_bytes()).collect();
        query.sort_unstable();
        query.dedup();

        let matches = block_on(index.search(&query)).unwrap();

        // The intersection of the terms, computed independently of the index.
        let mut wanted: Option<Vec<u64>> = None;
        for term in &query {
            let docs: Vec<u64> = expected
                .get(std::str::from_utf8(term).unwrap())
                .map(|d| d.keys().copied().collect())
                .unwrap_or_default();
            wanted = Some(match wanted {
                None => docs,
                Some(previous) => previous.into_iter().filter(|d| docs.contains(d)).collect(),
            });
        }
        let wanted = wanted.unwrap_or_default();

        prop_assert_eq!(matches.iter().map(|m| m.doc).collect::<Vec<_>>(), wanted.clone());
        for m in &matches {
            let expected_payload = format!("payload-{}", m.doc);
            prop_assert_eq!(m.payload.bytes().unwrap(), expected_payload.as_bytes());
        }

        let bound = 2 * query.len() as u32 + wanted.len() as u32;
        prop_assert!(
            reader.reads() <= bound,
            "{} reads exceeds the bound of {}",
            reader.reads(),
            bound
        );
    }

    #[test]
    fn building_the_same_input_twice_is_byte_identical(
        corpus in corpus_strategy(),
        options in options_strategy(),
    ) {
        prop_assert_eq!(build(&corpus, options).0, build(&corpus, options).0);
    }
}

proptest! {
    #[test]
    fn document_lengths_read_back_as_the_token_counts_they_were(
        corpus in corpus_strategy(),
        options in options_strategy(),
    ) {
        let options = Options { doc_lengths: true, ..options };
        let (bytes, expected) = build(&corpus, options);
        let index = block_on(Index::open(SliceReader::new(bytes))).unwrap();

        let lengths = index.doc_lengths().expect("stored");
        prop_assert_eq!(lengths.len(), corpus.len());
        for (doc, tokens) in corpus.iter().enumerate() {
            prop_assert_eq!(lengths[doc], tokens.len() as u64, "document {}", doc);
        }
        // The header's total is the sum, which is what makes the mean usable.
        prop_assert_eq!(index.header().sum_doclen, lengths.iter().sum::<u64>());

        // And a term's frequency in a document never exceeds that document's
        // length, which is the invariant a scorer relies on.
        for term in expected.keys() {
            let entry = block_on(index.lookup(term.as_bytes())).unwrap().unwrap();
            let postings = block_on(index.postings(&entry)).unwrap();
            for (doc, freq) in postings.docs.iter().zip(&postings.freqs) {
                prop_assert!(*freq <= lengths[*doc as usize]);
            }
        }
    }
}
