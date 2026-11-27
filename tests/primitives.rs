//! The byte layer's term primitives, how it reads blocks, and the readers a
//! caller plugs in: the checks `go/index_test.go` makes of the Go reader.

use mokuji::read::{block_on::block_on, CountingReader, Index, OpenOptions, SliceReader};
use mokuji::{Blocking, BlockingRangeReader, FormatError, MokujiIndex};

fn load(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("conformance/fixtures")
            .join(format!("{name}.mokuji")),
    )
    .unwrap()
}

const UNCOALESCED: OpenOptions = OpenOptions {
    resident_dictionary: false,
    coalesce: false,
};

fn terms(found: &[(Vec<u8>, mokuji::read::TermEntry)]) -> Vec<String> {
    found
        .iter()
        .map(|(key, _)| String::from_utf8(key.clone()).unwrap())
        .collect()
}

#[test]
fn range_returns_the_terms_between_two_keys_both_inclusive() {
    let reader = CountingReader::new(SliceReader::new(load("dict")));
    let index = block_on(Index::open(&reader)).unwrap();
    let range = |low: &str, high: &str, limit| {
        terms(&block_on(index.range(low.as_bytes(), high.as_bytes(), limit)).unwrap())
    };
    assert_eq!(
        range(
            "東京都千代田区丸の内1丁目1番2号",
            "東京都港区赤坂1丁目1番1号",
            0
        ),
        [
            "東京都千代田区丸の内1丁目1番2号",
            "東京都千代田区丸の内2丁目1番1号",
            "東京都港区赤坂1丁目1番1号"
        ]
    );
    // Neither end has to be a term.
    assert_eq!(
        range(
            "東京都千代田区丸の内1丁目1番1号a",
            "東京都千代田区丸の内3",
            0
        ),
        [
            "東京都千代田区丸の内1丁目1番2号",
            "東京都千代田区丸の内2丁目1番1号"
        ]
    );
    assert_eq!(
        range("東京都", "東京都千代田区丸の内9", 1),
        ["東京都千代田区丸の内1丁目1番1号"]
    );
    // Both ends sort before the first block, so the resident index says the
    // range is empty without reading anything.
    reader.reset();
    assert!(range("北海道", "千葉県", 0).is_empty());
    assert_eq!(reader.reads(), 0);
}

#[test]
fn prefix_many_reads_each_block_once() {
    let reader = CountingReader::new(SliceReader::new(load("dict")));
    let index = block_on(Index::open_with(&reader, UNCOALESCED)).unwrap();
    reader.reset();
    let found = block_on(index.prefix_many(&[
        "東京都千代田区丸の内1".as_bytes(),
        "東京都千代田区丸の内".as_bytes(),
    ]))
    .unwrap();
    assert_eq!((found[0].len(), found[1].len(), reader.reads()), (2, 3, 2));
}

#[test]
fn a_prefix_nothing_can_begin_with_costs_no_read() {
    let reader = CountingReader::new(SliceReader::new(load("tiny")));
    let index = block_on(Index::open(&reader)).unwrap();
    reader.reset();
    assert!(block_on(index.prefix(b"a", 20)).unwrap().is_empty());
    assert!(block_on(index.prefix_many(&[b"a"])).unwrap()[0].is_empty());
    assert_eq!(reader.reads(), 0);
    // A prefix of the first term is not before it in that sense.
    assert_eq!(
        terms(&block_on(index.prefix(b"c", 20)).unwrap()),
        ["chiyoda", "chuo"]
    );
}

#[test]
fn neighbouring_blocks_are_one_read_unless_told_otherwise() {
    let docs: Vec<u64> = (0..5).collect();
    let count = |options| {
        let reader = CountingReader::new(SliceReader::new(load("dict")));
        let index = block_on(Index::open_with(&reader, options)).unwrap();
        reader.reset();
        let payloads = block_on(index.payload_span(&docs)).unwrap();
        (reader.reads(), payloads)
    };
    let (apart, a) = count(UNCOALESCED);
    let (together, b) = count(OpenOptions::default());
    assert_eq!(a, b);
    assert!(apart > 1);
    assert_eq!(together, 1);
}

#[test]
fn payload_comes_back_in_the_order_asked() {
    let index = block_on(Index::open(SliceReader::new(load("dict")))).unwrap();
    let ascending = block_on(index.payload_span(&[0, 3])).unwrap();
    let descending = block_on(index.payload_span(&[3, 0])).unwrap();
    assert_eq!(ascending, [descending[1].clone(), descending[0].clone()]);
}

#[test]
fn a_held_dictionary_answers_lookups_without_a_read() {
    let reader = CountingReader::new(SliceReader::new(load("dict")));
    let options = OpenOptions {
        resident_dictionary: true,
        ..OpenOptions::default()
    };
    let index = block_on(Index::open_with(&reader, options)).unwrap();
    reader.reset();
    let found = block_on(index.lookup_many(&[
        "東京都千代田区丸の内1丁目1番1号".as_bytes(),
        "東京都港区赤坂1丁目1番1号".as_bytes(),
    ]))
    .unwrap();
    assert!(found.iter().all(Option::is_some));
    assert_eq!(reader.reads(), 0);
}

/// A blocking source, as a caller's HTTP client would be.
struct Held(Vec<u8>);

impl BlockingRangeReader for Held {
    fn file_len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_range(&self, offset: u64, len: u32) -> Result<Vec<u8>, FormatError> {
        let start = offset as usize;
        self.0
            .get(start..start + len as usize)
            .map(<[u8]>::to_vec)
            .ok_or(FormatError::Read {
                kind: std::io::ErrorKind::UnexpectedEof,
                detail: "past the end".into(),
            })
    }
}

#[test]
fn a_blocking_source_answers_as_any_other_does() {
    let blocking = block_on(MokujiIndex::open(Blocking(Held(load("dict"))))).unwrap();
    let slice = block_on(MokujiIndex::open(SliceReader::new(load("dict")))).unwrap();
    let a = block_on(blocking.lookup("東京都千代田区丸の内", 10)).unwrap();
    let b = block_on(slice.lookup("東京都千代田区丸の内", 10)).unwrap();
    assert_eq!(a, b);
    assert_eq!(blocking.stats(), slice.stats());
}

#[test]
fn write_params_say_what_the_other_writers_say() {
    use mokuji::meta::Value;
    let options = mokuji::write::Options {
        compress: false,
        ..mokuji::write::Options::default()
    };
    let params = options.params();
    let uint = |key: &str| params.get(key).and_then(Value::as_uint);
    assert_eq!(uint("terms_per_block"), Some(1024));
    assert_eq!(uint("docs_per_payload_block"), Some(64));
    assert_eq!(uint("has_freqs"), Some(1));
    assert_eq!(uint("inline_postings"), Some(8));
    assert_eq!(uint("compress"), Some(0));
    assert_eq!(
        params.get("payload").and_then(Value::as_text),
        Some("opaque")
    );
    assert_eq!(mokuji::write::GENERATOR_NAME, "mokuji");
    assert_eq!(mokuji::write::VERSION, env!("CARGO_PKG_VERSION"));
}
