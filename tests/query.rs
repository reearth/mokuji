//! `lookup_prefix` and `locate`, against the conformance fixtures and a file of
//! geohash coverings built here: the checks `js/test/query.test.js` makes.

use std::collections::{BTreeMap, BTreeSet};

use mokuji::meta::{cbor_map, ContentHash, Generator, Meta, Source, Tokenizer, Value};
use mokuji::query::{
    contains, decode_rings, geohash, locate, lookup_prefix, LocateOptions, PrefixMatch,
};
use mokuji::write::{Builder, Options};
use mokuji::{block_on, Error, MokujiIndex, SliceReader};

const MARUNOUCHI: &str = "東京都千代田区丸の内1丁目1番1号";

fn dictionary() -> MokujiIndex<SliceReader> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/fixtures/dict.mokuji");
    block_on(MokujiIndex::open(SliceReader::new(
        std::fs::read(path).unwrap(),
    )))
    .unwrap()
}

fn rows(hits: &[mokuji::query::PrefixHit]) -> Vec<(String, u64, PrefixMatch, String)> {
    hits.iter()
        .map(|h| (h.term.clone(), h.doc, h.r#match, h.payload.to_string()))
        .collect()
}

#[test]
fn lookup_prefix_says_whether_each_term_is_the_one_asked() {
    let dict = dictionary();
    let hits = block_on(lookup_prefix(
        &dict,
        "東京都千代田区丸の内1丁目1番",
        10,
        &[],
    ))
    .unwrap();
    let got: Vec<_> = rows(&hits)
        .into_iter()
        .map(|(t, d, m, _)| (t, d, m))
        .collect();
    assert_eq!(
        got,
        vec![
            (MARUNOUCHI.to_string(), 1, PrefixMatch::Prefix),
            (
                "東京都千代田区丸の内1丁目1番2号".to_string(),
                2,
                PrefixMatch::Prefix
            ),
        ]
    );
    assert_eq!(hits[0].payload.to_string(), "139.767201,35.681560");
    let exact = block_on(lookup_prefix(&dict, MARUNOUCHI, 10, &[])).unwrap();
    assert_eq!(
        exact.iter().map(|h| h.r#match).collect::<Vec<_>>(),
        vec![PrefixMatch::Exact]
    );
}

#[test]
fn lookup_prefix_finds_the_term_an_address_typed_past_it_begins_with() {
    let dict = dictionary();
    let typed = format!("{MARUNOUCHI}5階");
    assert!(block_on(lookup_prefix(&dict, &typed, 10, &[]))
        .unwrap()
        .is_empty());
    let hits = block_on(lookup_prefix(&dict, &typed, 10, &[&typed])).unwrap();
    assert_eq!(
        rows(&hits),
        vec![(
            MARUNOUCHI.to_string(),
            1,
            PrefixMatch::Within,
            "139.767201,35.681560".to_string()
        )]
    );
    assert!(
        block_on(lookup_prefix(&dict, "京都府", 10, &["京都府京都市"]))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn lookup_prefix_limit_counts_documents() {
    assert_eq!(
        block_on(lookup_prefix(&dictionary(), "東京都", 2, &[]))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn geohash_agrees_with_the_published_example() {
    assert_eq!(geohash(57.64911, 10.40744, 11), "u4pruydqqvj");
}

#[test]
fn decode_rings_closes_each_ring_and_contains_is_even_odd() {
    let rings = decode_rings("0,0,10,0,0,10,-10,0;3,3,4,0,0,4,-4,0", 1.0).unwrap();
    assert_eq!(
        rings[0],
        vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0]
        ]
    );
    assert!(contains(&rings, 1.0, 1.0));
    assert!(!contains(&rings, 5.0, 5.0));
    assert!(!contains(&rings, 11.0, 5.0));
    assert!(matches!(decode_rings("0,x", 1.0), Err(Error::Rings)));
}

/// Rings as delta-int-1e5: the first vertex, then differences, unclosed.
fn encode_rings(rings: &[Vec<[f64; 2]>]) -> String {
    rings
        .iter()
        .map(|ring| {
            let ints: Vec<(i64, i64)> = ring
                .iter()
                .map(|[x, y]| ((x * 1e5).round() as i64, (y * 1e5).round() as i64))
                .collect();
            ints.iter()
                .enumerate()
                .map(|(i, &(x, y))| {
                    let (dx, dy) = if i == 0 {
                        (x, y)
                    } else {
                        (x - ints[i - 1].0, y - ints[i - 1].1)
                    };
                    format!("{dx},{dy}")
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Two blocks side by side, the first with a courtyard, each keyed by the
/// cells its corners are in at precisions 5 and 6.
fn coverings() -> MokujiIndex<SliceReader> {
    let places = [
        (
            "A",
            "Marunouchi",
            vec![
                square(139.76, 35.68, 139.765, 35.685),
                square(139.762, 35.682, 139.763, 35.683),
            ],
        ),
        (
            "B",
            "Otemachi",
            vec![square(139.765, 35.68, 139.77, 35.685)],
        ),
    ];
    let mut builder = Builder::new(Options {
        terms_per_block: 4,
        docs_per_payload_block: 2,
        ..Options::default()
    })
    .unwrap();
    let mut postings: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for (doc, (code, name, rings)) in places.iter().enumerate() {
        builder
            .add_document(format!("{code}\u{1f}{name}|{}", encode_rings(rings)).into_bytes())
            .unwrap();
        for [lon, lat] in &rings[0] {
            for precision in [5, 6] {
                postings
                    .entry(geohash(*lat, *lon, precision))
                    .or_default()
                    .insert(doc as u64);
            }
        }
    }
    for (cell, docs) in &postings {
        for &doc in docs {
            builder.add_posting(cell.as_bytes(), doc, 1).unwrap();
        }
    }
    let text = |s: &str| Value::Text(s.to_string());
    let meta = Meta {
        generation: 1,
        sources: vec![Source {
            content_hash: ContentHash::parse(&format!("sha2-256:{}", "0".repeat(64))).unwrap(),
            uri: None,
            retrieved: None,
            modified: None,
        }],
        generator: Generator {
            name: "test".to_string(),
            version: "0".to_string(),
            params: cbor_map(Vec::<(String, Value)>::new()),
        },
        tokenizer: Tokenizer {
            id: "identity".to_string(),
            params: cbor_map(Vec::<(String, Value)>::new()),
        },
        attrs: Some(cbor_map([
            ("geohash-precisions", text("5,6")),
            ("geometry", text("delta-int-1e5")),
            ("payload-fields", text("CODE,NAME")),
        ])),
        license: None,
        attribution: None,
    };
    let bytes = builder.finish(&meta.encode()).unwrap();
    block_on(MokujiIndex::open(SliceReader::new(bytes))).unwrap()
}

#[test]
fn locate_finds_the_polygons_holding_a_point_named_by_the_files_fields() {
    let index = coverings();
    let options = LocateOptions::default();
    let found = block_on(locate(&index, 35.6801, 139.7601, &options)).unwrap();
    assert_eq!(found.cell, geohash(35.6801, 139.7601, 6));
    assert!(found.candidates >= 1);
    assert_eq!(found.inside.len(), 1);
    let p = &found.inside[0];
    assert_eq!((p.doc, p.label.as_str()), (0, "A\u{1f}Marunouchi"));
    let fields = p.fields.as_ref().unwrap();
    assert_eq!(
        (fields["CODE"].as_str(), fields["NAME"].as_str()),
        ("A", "Marunouchi")
    );
    assert_eq!(p.rings.len(), 2);

    let east = block_on(locate(&index, 35.6801, 139.7699, &options)).unwrap();
    let codes: Vec<_> = east
        .inside
        .iter()
        .map(|p| p.fields.as_ref().unwrap()["CODE"].clone())
        .collect();
    assert_eq!(codes, vec!["B"]);
    // In the courtyard: a candidate, but in neither.
    let courtyard = block_on(locate(&index, 35.6825, 139.7625, &options)).unwrap();
    assert!(courtyard.candidates >= 1);
    assert!(courtyard.inside.is_empty());
}

#[test]
fn locate_refuses_a_file_that_does_not_say_how_it_was_built() {
    let dict = dictionary();
    let refused = block_on(locate(&dict, 35.68, 139.76, &LocateOptions::default()));
    assert!(matches!(refused, Err(Error::NotCoverings)));
    // Unless the caller says.
    let options = LocateOptions {
        precisions: Some(vec![5]),
        scale: Some(1e5),
    };
    assert!(block_on(locate(&dict, 35.68, 139.76, &options))
        .unwrap()
        .inside
        .is_empty());
}

#[test]
fn lookup_prefix_asked_for_nothing_reads_nothing() {
    let dict = dictionary();
    dict.reset_stats();
    let typed = format!("{MARUNOUCHI}5階");
    assert!(block_on(lookup_prefix(&dict, &typed, 0, &[&typed]))
        .unwrap()
        .is_empty());
    assert_eq!(dict.stats().reads, 0);
}

#[test]
fn locate_defers_to_the_file_for_what_the_caller_does_not_really_say() {
    let index = coverings();
    let unsaid = LocateOptions {
        precisions: Some(vec![]),
        scale: Some(f64::NAN),
    };
    let found = block_on(locate(&index, 35.6801, 139.7601, &unsaid)).unwrap();
    assert_eq!(found.inside.len(), 1);
    for scale in [0.0, -1.0, f64::INFINITY] {
        let options = LocateOptions {
            scale: Some(scale),
            ..LocateOptions::default()
        };
        assert_eq!(
            block_on(locate(&index, 35.6801, 139.7601, &options))
                .unwrap()
                .inside
                .len(),
            1
        );
    }
    for precisions in [vec![0], vec![13], vec![5, 13]] {
        let options = LocateOptions {
            precisions: Some(precisions),
            ..LocateOptions::default()
        };
        assert!(matches!(
            block_on(locate(&index, 35.6801, 139.7601, &options)),
            Err(Error::NotCoverings)
        ));
    }
}

#[test]
fn decode_rings_refuses_what_overflows() {
    let text = format!("{},0,{},0", i64::MAX, i64::MAX);
    assert!(matches!(decode_rings(&text, 1.0), Err(Error::Rings)));
}
