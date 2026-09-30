//! A collection end to end: a base, a segment added after it, a delete, and
//! the file compacting them makes — which has to answer exactly as they did.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BASE: &str = "Black Sea\u{1f}Černé moře\tBlack Sea\n\
                    Black Forest\u{1f}Schwarzwald\tBlack Forest\n\
                    Blackpool\tBlackpool\n\
                    Bled\tBled\n\
                    Tokyo\u{1f}東京\tTokyo\n\
                    Tohoku\u{1f}東北\tTohoku\n";

const DELTA: &str = "Black Mountain\tBlack Mountain\n\
                     Tokyo Tower\u{1f}東京タワー\tTokyo Tower\n";

fn mokuji(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mokuji"))
        .args(args)
        .output()
        .expect("running mokuji")
}

fn ok(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn build(dir: &Path, name: &str, corpus: &str) -> PathBuf {
    let tsv = dir.join(format!("{name}.tsv"));
    let file = dir.join(format!("{name}.mokuji"));
    std::fs::write(&tsv, corpus).unwrap();
    ok(mokuji(&[
        "build",
        tsv.to_str().unwrap(),
        file.to_str().unwrap(),
        "--tokenizer",
        "name-list",
        "--completions",
        "2,0,2",
        "--terms-per-block",
        "2",
        "--docs-per-payload-block",
        "2",
    ]));
    file
}

/// The payloads a command printed, in order, whatever it numbered or scored
/// them by: the last field of each line, since these payloads hold no tab.
fn payloads(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.rsplit_once('\t').expect("id and payload").1.to_string())
        .collect()
}

fn collection(dir: &Path) -> PathBuf {
    let base = build(dir, "base", BASE);
    let delta = build(dir, "delta", DELTA);
    let manifest = dir.join("collection.json");
    let m = manifest.to_str().unwrap();
    ok(mokuji(&["manifest", "new", m, base.to_str().unwrap()]));
    ok(mokuji(&["manifest", "add", m, delta.to_str().unwrap()]));
    manifest
}

#[test]
fn a_collection_is_its_segments_in_order_less_what_was_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = collection(dir.path());
    let m = manifest.to_str().unwrap();

    let before = ok(mokuji(&["complete", m, "black", "-n", "10"]));
    assert_eq!(
        payloads(&before),
        ["Black Sea", "Black Forest", "Blackpool", "Black Mountain"]
    );

    // Black Sea is document 0 of the base, and the first document every `b`
    // entry lists.
    let deleted = ok(mokuji(&["manifest", "delete", m, "base.mokuji", "0"]));
    let said = String::from_utf8_lossy(&deleted.stdout);
    assert!(said.contains("generation 3"), "{said}");
    assert!(
        said.contains("completion entries now list a deleted document"),
        "{said}"
    );

    let after = ok(mokuji(&["complete", m, "black", "-n", "10"]));
    assert_eq!(
        payloads(&after),
        ["Black Forest", "Blackpool", "Black Mountain"]
    );

    // The entry for `bl` lists Black Sea and Black Forest; with Black Sea gone
    // the base is asked again, past its entry, so the answer is still two.
    let bl = ok(mokuji(&["complete", m, "bl", "-n", "2"]));
    assert_eq!(payloads(&bl), ["Black Forest", "Blackpool"]);
    assert!(String::from_utf8_lossy(&bl.stderr).contains("segment 0: the dictionary"));

    let tower = ok(mokuji(&["search", m, "東京タワー"]));
    assert_eq!(payloads(&tower), ["Tokyo Tower"]);

    assert!(ok(mokuji(&["verify", m])).status.success());
}

#[test]
fn a_compacted_collection_answers_exactly_as_the_collection_did() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = collection(dir.path());
    let m = manifest.to_str().unwrap();
    ok(mokuji(&["manifest", "delete", m, "base.mokuji", "0", "4"]));
    ok(mokuji(&["manifest", "delete", m, "delta.mokuji", "0"]));

    let compacted = dir.path().join("compacted.mokuji");
    let c = compacted.to_str().unwrap();
    ok(mokuji(&["compact", m, c]));
    ok(mokuji(&["verify", c]));

    for prefix in [
        "b", "bl", "black", "black s", "t", "to", "tokyo", "東", "東京", "x",
    ] {
        for limit in ["1", "2", "3", "10"] {
            let from_collection = payloads(&ok(mokuji(&["complete", m, prefix, "-n", limit])));
            let from_file = payloads(&ok(mokuji(&["complete", c, prefix, "-n", limit])));
            assert_eq!(from_collection, from_file, "complete {prefix:?} -n {limit}");
        }
    }
    for term in ["tokyo", "black forest", "東京タワー", "black mountain"] {
        let from_collection = payloads(&ok(mokuji(&["search", m, term])));
        let from_file = payloads(&ok(mokuji(&["search", c, term])));
        assert_eq!(from_collection, from_file, "search {term:?}");
    }
}

#[test]
fn a_manifest_that_misdescribes_a_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = collection(dir.path());
    let m = manifest.to_str().unwrap();

    // Rebuilt with different contents under the same name: the hash the
    // manifest recorded no longer matches.
    build(dir.path(), "delta", "Black Rock\tBlack Rock\n");
    let output = mokuji(&["verify", m]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not the"));

    let output = mokuji(&["manifest", "delete", m, "base.mokuji", "99"]);
    assert!(
        !output.status.success(),
        "a document the segment does not have"
    );
}

#[test]
fn compaction_refuses_to_mix_licenses() {
    let dir = tempfile::tempdir().unwrap();
    let base = build(dir.path(), "base", BASE);
    let tsv = dir.path().join("osm.tsv");
    let osm = dir.path().join("osm.mokuji");
    std::fs::write(&tsv, DELTA).unwrap();
    ok(mokuji(&[
        "build",
        tsv.to_str().unwrap(),
        osm.to_str().unwrap(),
        "--tokenizer",
        "name-list",
        "--completions",
        "2,0,2",
        "--terms-per-block",
        "2",
        "--docs-per-payload-block",
        "2",
        "--license",
        "ODbL-1.0",
    ]));
    let manifest = dir.path().join("mixed.json");
    let m = manifest.to_str().unwrap();
    ok(mokuji(&[
        "manifest",
        "new",
        m,
        base.to_str().unwrap(),
        osm.to_str().unwrap(),
    ]));
    let output = mokuji(&[
        "compact",
        m,
        dir.path().join("out.mokuji").to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("license"));
}

mod folding {
    //! The same property, on generated collections: any split into segments,
    //! any deletes, every prefix, every limit.

    use std::path::Path;

    use mokuji_cli::build::{self, Recipe};
    use mokuji_cli::corpus::Tokenizer;
    use mokuji_cli::{collection, compact};
    use mokuji_manifest::Collection;
    use mokuji_read::{block_on::block_on, Index, SliceReader};
    use mokuji_write::{Completions, Options};
    use proptest::prelude::*;

    const NAMES: &[&str] = &[
        "a", "ab", "abc", "abd", "b", "ba", "bab", "東", "東京", "東北",
    ];

    fn segment(dir: &Path, name: &str, docs: &[Vec<usize>]) -> std::path::PathBuf {
        let tsv = dir.join(format!("{name}.tsv"));
        let file = dir.join(format!("{name}.mokuji"));
        let corpus: String = docs
            .iter()
            .enumerate()
            .map(|(i, names)| {
                let text: Vec<&str> = names.iter().map(|&n| NAMES[n]).collect();
                format!("{}\t{name}-{i}\n", text.join("\u{1f}"))
            })
            .collect();
        std::fs::write(&tsv, corpus).unwrap();
        build::run(
            &tsv,
            &file,
            &Recipe::default(),
            Options {
                terms_per_block: 2,
                docs_per_payload_block: 2,
                completions: Some(Completions {
                    min_terms: 2,
                    max_span: 0,
                    docs_per_entry: 2,
                }),
                ..Options::default()
            },
            Tokenizer::NameList,
        )
        .unwrap();
        file
    }

    fn docs() -> impl Strategy<Value = Vec<Vec<usize>>> {
        prop::collection::vec(prop::collection::vec(0..NAMES.len(), 1..3), 1..6)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn compaction_changes_no_answer(
            segments in prop::collection::vec(docs(), 1..4),
            deletes in prop::collection::vec(any::<prop::sample::Index>(), 0..4),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let files: Vec<_> = segments
                .iter()
                .enumerate()
                .map(|(i, d)| segment(dir.path(), &format!("s{i}"), d))
                .collect();
            let manifest = dir.path().join("m.json");
            let paths: Vec<&Path> = files.iter().map(|f| f.as_path()).collect();
            collection::new(&manifest, &paths).unwrap();

            // Deletes, spread over the segments, never all of a collection.
            let total: usize = segments.iter().map(Vec::len).sum();
            let mut picked: Vec<usize> = deletes.iter().map(|i| i.index(total)).collect();
            picked.sort_unstable();
            picked.dedup();
            prop_assume!(picked.len() < total);
            let mut offset = 0;
            for (i, d) in segments.iter().enumerate() {
                let mine: Vec<u64> = picked
                    .iter()
                    .filter(|&&p| p >= offset && p < offset + d.len())
                    .map(|&p| (p - offset) as u64)
                    .collect();
                if !mine.is_empty() {
                    collection::delete(&manifest, &format!("s{i}.mokuji"), &mine).unwrap();
                }
                offset += d.len();
            }

            let compacted = dir.path().join("c.mokuji");
            compact::run(&manifest, &compacted).unwrap();

            let m = collection::read(&manifest).unwrap();
            let readers = collection::open(&manifest, &m).unwrap();
            let col = block_on(Collection::open(&m, readers.iter().collect())).unwrap();
            let file = block_on(Index::open(SliceReader::new(std::fs::read(&compacted).unwrap()))).unwrap();

            let mut prefixes: Vec<Vec<u8>> = NAMES
                .iter()
                .flat_map(|n| (1..=n.len()).map(move |l| n.as_bytes()[..l].to_vec()))
                .collect();
            prefixes.push(b"z".to_vec());
            for prefix in &prefixes {
                for limit in [1usize, 2, 3, 20] {
                    let hits = block_on(col.complete(prefix, limit)).unwrap().hits;
                    let want: Vec<String> = block_on(col.payloads(&hits))
                        .unwrap()
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    let (docs, _) = block_on(file.complete(prefix, limit)).unwrap();
                    let got: Vec<String> = block_on(file.payload_span(&docs))
                        .unwrap()
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    prop_assert_eq!(&got, &want, "prefix {:?} limit {}", prefix, limit);
                }
            }
        }
    }
}
