//! Round-trip properties: whatever is written can be read back unchanged, and
//! reading it stays within the bound `SPEC.md` §4 promises.
//!
//! Generated corpora reach shapes a hand-written fixture does not: blocks that
//! divide evenly and blocks that do not, terms that share long prefixes,
//! documents holding one term and documents holding all of them.

use std::collections::BTreeMap;

use mokuji_read::{block_on::block_on, CountingReader, Index, SliceReader};
use mokuji_write::{Builder, Column, Options};
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
            positions: false,
            doc_lengths,
            compress,
            completions: None,
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

proptest! {
    #[test]
    fn attribute_columns_read_back_as_they_were_written(
        corpus in corpus_strategy(),
        options in options_strategy(),
        scale in -6i8..6,
    ) {
        let mut builder = Builder::new(options).unwrap();
        let mut expected: BTreeMap<String, BTreeMap<u64, u64>> = BTreeMap::new();
        for (i, tokens) in corpus.iter().enumerate() {
            let doc = builder.add_document(format!("payload-{i}").into_bytes()).unwrap();
            for &token in tokens {
                *expected.entry(VOCABULARY[token].to_string()).or_default()
                    .entry(doc).or_default() += 1;
            }
        }
        for (term, docs) in &expected {
            for (&doc, &freq) in docs {
                builder.add_posting(term.as_bytes(), doc, freq).unwrap();
            }
        }

        // Negative and positive, small and large: the values go through zigzag
        // and bit-packing, and both halves have to survive.
        let importance: Vec<i64> = (0..corpus.len() as i64).map(|i| i * 7 - 13).collect();
        let cell: Vec<i64> = (0..corpus.len() as i64).map(|i| 1_000_000 + i).collect();
        builder.add_column(Column {
            name: "importance".into(),
            scale_exponent: scale,
            values: importance.clone(),
        }).unwrap();
        builder.add_column(Column {
            name: "cell".into(),
            scale_exponent: 0,
            values: cell.clone(),
        }).unwrap();

        let index = block_on(Index::open(SliceReader::new(builder.finish(META).unwrap()))).unwrap();

        prop_assert_eq!(index.columns().len(), 2);
        let a = index.column("importance").expect("present");
        prop_assert_eq!(&a.values, &importance);
        prop_assert_eq!(a.scale_exponent, scale);
        prop_assert_eq!(&index.column("cell").expect("present").values, &cell);
        prop_assert!(index.column("missing").is_none());
    }
}

#[test]
fn a_column_must_have_one_value_per_document() {
    let mut builder = Builder::new(Options::default()).unwrap();
    builder.add_document(b"a".to_vec()).unwrap();
    builder.add_document(b"b".to_vec()).unwrap();
    builder.add_posting(b"t", 0, 1).unwrap();
    builder
        .add_column(Column {
            name: "short".into(),
            scale_exponent: 0,
            values: vec![1],
        })
        .unwrap();
    assert!(builder.finish(META).is_err());
}

#[test]
fn columns_must_have_distinct_names() {
    let mut builder = Builder::new(Options::default()).unwrap();
    let column = || Column {
        name: "x".into(),
        scale_exponent: 0,
        values: vec![],
    };
    builder.add_column(column()).unwrap();
    assert!(builder.add_column(column()).is_err());
}

proptest! {
    #[test]
    fn positions_read_back_as_the_places_the_terms_were(
        corpus in corpus_strategy(),
        options in options_strategy(),
    ) {
        // Positions need frequencies: the frequency is what says how many
        // positions belong to each document.
        let options = Options { positions: true, has_freqs: true, ..options };
        let mut builder = Builder::new(options).unwrap();

        // Where each term sits in each document, which is what a phrase query
        // tests and what nothing else needs.
        let mut expected: BTreeMap<String, BTreeMap<u64, Vec<u64>>> = BTreeMap::new();
        for (i, tokens) in corpus.iter().enumerate() {
            let doc = builder.add_document(format!("payload-{i}").into_bytes()).unwrap();
            for (at, &token) in tokens.iter().enumerate() {
                expected.entry(VOCABULARY[token].to_string()).or_default()
                    .entry(doc).or_default().push(at as u64);
            }
        }
        for (term, docs) in &expected {
            for (&doc, places) in docs {
                builder.add_posting_at(term.as_bytes(), doc, places).unwrap();
            }
        }

        let index = block_on(Index::open(SliceReader::new(builder.finish(META).unwrap()))).unwrap();
        prop_assert!(index.has_positions());

        for (term, docs) in &expected {
            let entry = block_on(index.lookup(term.as_bytes())).unwrap().unwrap();
            let postings = block_on(index.postings(&entry)).unwrap();
            let positions = block_on(index.positions(&entry, &postings)).unwrap();

            prop_assert_eq!(positions.len(), postings.docs.len());
            for (i, &doc) in postings.docs.iter().enumerate() {
                prop_assert_eq!(&positions[i], &docs[&doc]);
                // The frequency is how many positions there are, by definition.
                prop_assert_eq!(postings.freqs[i] as usize, positions[i].len());
            }
        }
    }
}

#[test]
fn a_file_without_positions_reports_none() {
    let mut builder = Builder::new(Options::default()).unwrap();
    builder.add_document(b"p".to_vec()).unwrap();
    builder.add_posting(b"t", 0, 3).unwrap();
    let index = block_on(Index::open(SliceReader::new(builder.finish(META).unwrap()))).unwrap();

    assert!(!index.has_positions());
    let entry = block_on(index.lookup(b"t")).unwrap().unwrap();
    let postings = block_on(index.postings(&entry)).unwrap();
    assert!(block_on(index.positions(&entry, &postings))
        .unwrap()
        .is_empty());
}

#[test]
fn positions_must_ascend() {
    let mut builder = Builder::new(Options {
        positions: true,
        ..Options::default()
    })
    .unwrap();
    builder.add_document(b"p".to_vec()).unwrap();
    assert!(builder.add_posting_at(b"t", 0, &[3, 1]).is_err());
    assert!(builder.add_posting_at(b"t", 0, &[2, 2]).is_err());
    assert!(builder.add_posting_at(b"t", 0, &[]).is_err());
}

#[test]
fn positions_need_frequencies() {
    let options = Options {
        positions: true,
        has_freqs: false,
        ..Options::default()
    };
    assert!(Builder::new(options).is_err());
}

proptest! {
    /// Every expensive prefix has an entry, every entry is an expensive prefix,
    /// and each lists the first documents of its prefix — checked against every
    /// byte prefix of every term, the definition in `SPEC.md` §5.3.5, rather
    /// than against the walk the writer uses to avoid enumerating them.
    #[test]
    fn completions_cover_exactly_the_expensive_prefixes(
        corpus in corpus_strategy(),
        options in options_strategy(),
        min_terms in 0..4u64,
        max_span in prop_oneof![Just(0u64), 1..24u64],
        docs_per_entry in 1..4u32,
    ) {
        prop_assume!(min_terms != 0 || max_span != 0);
        let options = Options {
            completions: Some(mokuji_write::Completions { min_terms, max_span, docs_per_entry }),
            ..options
        };
        let (bytes, expected) = build(&corpus, options);
        let reader = CountingReader::new(SliceReader::new(bytes));
        let index = block_on(Index::open(&reader)).unwrap();
        let completions = index.completions().expect("asked for").clone();

        let mut prefixes = std::collections::BTreeSet::new();
        for term in expected.keys() {
            for len in 1..=term.len() {
                prefixes.insert(term.as_bytes()[..len].to_vec());
            }
        }
        let mut expensive = 0;
        for prefix in &prefixes {
            let entries = block_on(index.prefix(prefix, usize::MAX)).unwrap();
            let span: u64 = entries.iter().map(|(_, e)| e.postings_len).sum();
            let costly = (min_terms != 0 && entries.len() as u64 >= min_terms)
                || (max_span != 0 && span >= max_span);
            let mut docs: Vec<u64> = expected
                .iter()
                .filter(|(term, _)| term.as_bytes().starts_with(prefix))
                .flat_map(|(_, docs)| docs.keys().copied())
                .collect();
            docs.sort_unstable();
            docs.dedup();
            docs.truncate(docs_per_entry as usize);

            match completions.get(prefix) {
                Some(listed) => {
                    prop_assert!(costly, "{prefix:?} has an entry and is cheap");
                    prop_assert_eq!(listed, docs.as_slice());
                    expensive += 1;

                    // Answered from the resident set, at no read.
                    reader.reset();
                    let (got, from_table) =
                        block_on(index.complete(prefix, docs_per_entry as usize)).unwrap();
                    prop_assert!(from_table);
                    prop_assert_eq!(got, docs);
                    prop_assert_eq!(reader.reads(), 0);
                }
                None => {
                    prop_assert!(!costly, "{prefix:?} is expensive and has no entry");
                    let (got, from_table) =
                        block_on(index.complete(prefix, docs_per_entry as usize)).unwrap();
                    prop_assert!(!from_table);
                    prop_assert_eq!(got, docs);
                }
            }
        }
        prop_assert_eq!(completions.len(), expensive);
    }
}

#[test]
fn completions_need_something_to_cover() {
    let options = Options {
        completions: Some(mokuji_write::Completions {
            min_terms: 0,
            max_span: 0,
            docs_per_entry: 10,
        }),
        ..Options::default()
    };
    assert!(Builder::new(options).is_err());
}
