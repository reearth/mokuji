//! The input format `mokuji build` reads, and the tokenizer it applies.
//!
//! A tab-separated file: the text to index, then the payload to return. It is
//! deliberately the simplest thing that carries both halves, because the
//! interesting inputs arrive through `convert` from an existing tool rather
//! than through this.

use std::collections::BTreeMap;

/// How the text of a line becomes terms.
///
/// The specification does not say how to tokenize; it says the choice must be
/// recorded so that the derivation is repeatable. These are the identifiers
/// from `TOKENIZERS.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tokenizer {
    /// Split on single spaces, discarding empties.
    #[default]
    SpaceSplit,
    /// The whole line is one term.
    ///
    /// What a dictionary wants rather than a document collection: an address is
    /// looked up whole or by prefix, not matched by its parts.
    Identity,
    /// The whole line is one term, after Japanese address normalization.
    ///
    /// See `TOKENIZERS.md`. Registered rather than specified: what it does is
    /// pinned down so that an index is reproducible, not so that every index
    /// has to use it.
    JaAddress,
    /// Character bigrams for scripts without word spacing, whole words for the
    /// rest.
    ///
    /// The analyzer that needs no dictionary, which is the only kind a reader
    /// with no server can be relied on to have. See `TOKENIZERS.md`.
    Bigram,
}

impl Tokenizer {
    /// The registered identifier, recorded in the metadata.
    pub fn id(self) -> &'static str {
        match self {
            Tokenizer::SpaceSplit => "space-split",
            Tokenizer::Identity => "identity",
            Tokenizer::JaAddress => "ja-address",
            Tokenizer::Bigram => "bigram",
        }
    }

    /// Applies this tokenizer to a query.
    ///
    /// A query has to go through the same transformation the terms did, or it
    /// will not match them. This is what the recorded tokenizer identifier is
    /// for: a reader can look up what was done and do it again, without the
    /// format having to specify what that was.
    pub fn normalize_query(self, query: &str) -> String {
        match self {
            // A prefix is a prefix of a term, so it goes through whatever
            // rewriting the terms went through — but not through splitting,
            // which would destroy the thing being prefixed.
            Tokenizer::SpaceSplit | Tokenizer::Identity | Tokenizer::Bigram => query.to_string(),
            Tokenizer::JaAddress => ja_address(query),
        }
    }

    /// Whether this tokenizer makes the whole line a key.
    ///
    /// A key names one record, so two lines that produce the same key are a
    /// duplicate rather than two documents that happen to share a term.
    pub fn is_key(self) -> bool {
        matches!(self, Tokenizer::Identity | Tokenizer::JaAddress)
    }

    /// Parses a registered identifier.
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "space-split" => Some(Tokenizer::SpaceSplit),
            "identity" => Some(Tokenizer::Identity),
            "ja-address" => Some(Tokenizer::JaAddress),
            "bigram" => Some(Tokenizer::Bigram),
            _ => None,
        }
    }

    /// Splits `text` into terms.
    ///
    /// Returns owned strings because a tokenizer may rewrite as well as split;
    /// `ja-address` does.
    pub fn split(self, text: &str) -> Vec<String> {
        match self {
            Tokenizer::SpaceSplit => text
                .split(' ')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
            Tokenizer::Identity => vec![text.to_string()],
            Tokenizer::JaAddress => vec![ja_address(text)],
            Tokenizer::Bigram => bigram(text),
        }
    }
}

/// The `bigram` tokenizer, as registered in `TOKENIZERS.md`.
///
/// Runs of letters and digits are split on everything else. A run that is all
/// ASCII becomes one lowercased term; any other run becomes its character
/// bigrams, or the single character if that is all there is.
///
/// Less precise than morphological analysis, and the index is larger. It is
/// here because it needs no dictionary, and a dictionary is the one thing a
/// reader with no server cannot be assumed to have.
pub fn bigram(text: &str) -> Vec<String> {
    use unicode_normalization::UnicodeNormalization;

    let composed: String = text.nfkc().collect();
    let mut out = Vec::new();
    for run in composed.split(|c: char| !c.is_alphanumeric()) {
        if run.is_empty() {
            continue;
        }
        // Lowercased either way, so that a Latin letter matches the same
        // whether it stands alone or sits inside a run of kanji.
        let run = run.to_lowercase();
        if run.is_ascii() {
            out.push(run);
            continue;
        }
        let chars: Vec<char> = run.chars().collect();
        if chars.len() == 1 {
            out.push(chars[0].to_string());
            continue;
        }
        for pair in chars.windows(2) {
            out.push(pair.iter().collect());
        }
    }
    out
}

/// Japanese address normalization, as registered in `TOKENIZERS.md`.
///
/// Compatibility composition, then Japanese numerals before `丁目` rewritten as
/// decimal. The registry gives the full rule and the reasoning; the short
/// version is that the Address Base Registry writes chome both ways — `一丁目`
/// in one municipality and `１丁目` in the next — so without this the same
/// concept sorts in two different places, and neither is where a person types.
pub fn ja_address(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    let composed: String = text.nfkc().collect();
    let text = composed.trim();

    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        while i < chars.len() && numeral_value(chars[i]).is_some() {
            i += 1;
        }
        if i > start && chars[i..].starts_with(&['丁', '目']) {
            match parse_numeral(&chars[start..i]) {
                Some(value) => out.push_str(&value.to_string()),
                // Unparseable, so left exactly as it was rather than guessed at.
                None => out.extend(&chars[start..i]),
            }
        } else if i > start {
            out.extend(&chars[start..i]);
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// The value of a single Japanese numeral character, if it is one.
fn numeral_value(c: char) -> Option<u64> {
    Some(match c {
        '〇' | '零' => 0,
        '一' => 1,
        '二' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '七' => 7,
        '八' => 8,
        '九' => 9,
        '十' => 10,
        '百' => 100,
        '千' => 1000,
        _ => return None,
    })
}

/// Reads a Japanese numeral, positional and multiplicative forms alike.
///
/// `十二` is twelve and `二十` is twenty, so a multiplier applies to the digit
/// before it, or to one when there is none.
fn parse_numeral(chars: &[char]) -> Option<u64> {
    let mut total = 0u64;
    let mut digit: Option<u64> = None;
    for &c in chars {
        match numeral_value(c)? {
            unit @ (10 | 100 | 1000) => {
                total = total.checked_add(digit.unwrap_or(1).checked_mul(unit)?)?;
                digit = None;
            }
            value => {
                // Two bare digits in a row are positional: 一二 is not a
                // numeral this rule recognizes, so it is left alone.
                if digit.is_some() {
                    return None;
                }
                digit = Some(value);
            }
        }
    }
    total.checked_add(digit.unwrap_or(0))
}

/// Term to document to the count of occurrences.
pub type Postings = BTreeMap<Vec<u8>, BTreeMap<u64, u64>>;

/// Term to document to the positions it occupies there.
pub type Positions = BTreeMap<Vec<u8>, BTreeMap<u64, Vec<u64>>>;

/// A parsed corpus, with its postings already inverted.
pub struct Corpus {
    /// Payloads in the order they appeared, which fixes the document IDs.
    pub payloads: Vec<Vec<u8>>,
    /// Term to document to frequency, sorted, ready to stream to the builder.
    pub postings: Postings,
    /// Where each term occurs in each document, when it was asked for.
    ///
    /// Only built on request: a position per token occurrence is far more than
    /// the counts, and nothing but a phrase query wants them.
    pub positions: Option<Positions>,
    /// One column of raw attribute fields per declared column, per document.
    ///
    /// Kept as text: how many decimals a column carries is a build parameter,
    /// not something the corpus states.
    pub attributes: Vec<Vec<String>>,
    /// Lines dropped because their key was already taken.
    ///
    /// Only a key tokenizer produces these. Reported rather than silent: it is
    /// a statement about the data, and the caller should know how much of it
    /// was set aside.
    pub duplicate_keys: usize,
}

/// Parses a tab-separated corpus.
///
/// A line without a tab is an error rather than a document with empty payload:
/// silently indexing a malformed file is how a corpus and its index drift
/// apart.
pub fn parse(
    text: &str,
    tokenizer: Tokenizer,
    columns: usize,
    with_positions: bool,
) -> Result<Corpus, String> {
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut attributes: Vec<Vec<String>> = vec![Vec::new(); columns];
    let mut postings: Postings = BTreeMap::new();
    let mut duplicate_keys = 0usize;
    let mut positions: Option<Positions> = with_positions.then(BTreeMap::new);

    for (line_number, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        // With attribute columns declared, a line is exactly text, payload and
        // one field each; without them the payload is whatever follows the
        // first tab, tabs included.
        let (body, payload, fields) = if columns == 0 {
            let (body, payload) = line
                .split_once('\t')
                .ok_or_else(|| format!("line {} has no tab separator", line_number + 1))?;
            (body, payload, Vec::new())
        } else {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != columns + 2 {
                return Err(format!(
                    "line {} has {} fields; {} declared attribute column(s) need {}",
                    line_number + 1,
                    parts.len(),
                    columns,
                    columns + 2
                ));
            }
            (parts[0], parts[1], parts[2..].to_vec())
        };

        let doc = payloads.len() as u64;
        let tokens = tokenizer.split(body);

        if tokenizer.is_key() {
            // One line, one key, one record. A second line with the same key
            // would give the key two documents, which costs the whole index the
            // equivalence between term and document IDs that lets the
            // dictionary state its own postings.
            let Some(key) = tokens.into_iter().next().filter(|k| !k.is_empty()) else {
                continue;
            };
            if postings.contains_key(key.as_bytes()) {
                duplicate_keys += 1;
                continue;
            }
            postings.entry(key.into_bytes()).or_default().insert(doc, 1);
            payloads.push(payload.as_bytes().to_vec());
            continue;
        }

        for (at, token) in tokens.into_iter().enumerate() {
            if token.is_empty() {
                continue;
            }
            let term = token.into_bytes();
            if let Some(positions) = positions.as_mut() {
                positions
                    .entry(term.clone())
                    .or_default()
                    .entry(doc)
                    .or_default()
                    .push(at as u64);
            }
            *postings.entry(term).or_default().entry(doc).or_default() += 1;
        }
        payloads.push(payload.as_bytes().to_vec());
        for (column, field) in attributes.iter_mut().zip(&fields) {
            column.push((*field).to_string());
        }
    }

    if payloads.is_empty() {
        return Err("the corpus is empty".to_string());
    }
    Ok(Corpus {
        payloads,
        postings,
        positions,
        attributes,
        duplicate_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_split(text: &str) -> Result<Corpus, String> {
        parse(text, Tokenizer::SpaceSplit, 0, false)
    }

    #[test]
    fn inverts_a_corpus() {
        let corpus = parse_split("tokyo chuo\t1\nosaka chuo\t2\n").unwrap();
        assert_eq!(corpus.payloads.len(), 2);
        assert_eq!(corpus.postings[b"chuo".as_slice()], [(0, 1), (1, 1)].into());
        assert_eq!(corpus.postings[b"tokyo".as_slice()], [(0, 1)].into());
    }

    #[test]
    fn counts_repeated_terms() {
        let corpus = parse_split("a a a\tp\n").unwrap();
        assert_eq!(corpus.postings[b"a".as_slice()][&0], 3);
    }

    #[test]
    fn skips_blank_lines_but_not_malformed_ones() {
        assert!(parse_split("a\tp\n\nb\tq\n").is_ok());
        assert!(parse_split("a\tp\nno tab here\n").is_err());
    }

    #[test]
    fn rejects_an_empty_corpus() {
        assert!(parse_split("").is_err());
    }

    #[test]
    fn drops_empty_tokens() {
        assert_eq!(Tokenizer::SpaceSplit.split("a  b "), ["a", "b"]);
    }

    #[test]
    fn a_key_tokenizer_drops_lines_whose_key_is_taken() {
        // The two forms the Address Base Registry uses for the same chome.
        let corpus = parse(
            "丸の内一丁目\tA\n丸の内１丁目\tB\n",
            Tokenizer::JaAddress,
            0,
            false,
        )
        .unwrap();
        assert_eq!(corpus.duplicate_keys, 1);
        assert_eq!(corpus.payloads, [b"A".to_vec()]);
        assert_eq!(corpus.postings.len(), 1);
        assert_eq!(corpus.postings["丸の内1丁目".as_bytes()].len(), 1);
    }

    #[test]
    fn a_splitting_tokenizer_still_shares_terms_between_documents() {
        let corpus = parse(
            "tokyo chuo\t1\ntokyo kita\t2\n",
            Tokenizer::SpaceSplit,
            0,
            false,
        )
        .unwrap();
        assert_eq!(corpus.duplicate_keys, 0);
        assert_eq!(corpus.postings[b"tokyo".as_slice()].len(), 2);
    }

    #[test]
    fn identity_keeps_the_line_whole() {
        let corpus = parse(
            "東京都千代田区丸の内\t35.68,139.76\n",
            Tokenizer::Identity,
            0,
            false,
        )
        .unwrap();
        assert_eq!(corpus.postings.len(), 1);
        assert!(corpus
            .postings
            .contains_key("東京都千代田区丸の内".as_bytes()));
    }

    #[test]
    fn ja_address_rewrites_chome_to_decimal() {
        // The two forms the registry actually uses, brought together.
        assert_eq!(
            ja_address("東京都千代田区丸の内一丁目"),
            "東京都千代田区丸の内1丁目"
        );
        assert_eq!(
            ja_address("東京都千代田区丸の内１丁目"),
            "東京都千代田区丸の内1丁目"
        );
        assert_eq!(ja_address("札幌市北区北十一丁目"), "札幌市北区北11丁目");
        assert_eq!(ja_address("二十丁目"), "20丁目");
        assert_eq!(ja_address("百丁目"), "100丁目");
    }

    #[test]
    fn ja_address_leaves_numerals_that_are_not_chome_alone() {
        // 一番町 is a place name, not a numbered sequence.
        assert_eq!(ja_address("千代田区一番町"), "千代田区一番町");
        assert_eq!(ja_address("京都市下京区五条"), "京都市下京区五条");
        assert_eq!(ja_address("四谷"), "四谷");
    }

    #[test]
    fn ja_address_composes_compatibility_forms() {
        assert_eq!(ja_address("  東京都　"), "東京都");
        assert_eq!(ja_address("ﾏﾙﾉｳﾁ"), "マルノウチ");
    }

    #[test]
    fn ja_address_leaves_unparseable_runs_untouched() {
        // Two bare digits in sequence are positional notation, which this rule
        // does not claim to read.
        assert_eq!(ja_address("一二丁目"), "一二丁目");
    }

    #[test]
    fn ja_address_sorts_chome_in_order() {
        let mut terms: Vec<String> = ["三丁目", "十丁目", "一丁目", "二丁目"]
            .iter()
            .map(|t| ja_address(t))
            .collect();
        terms.sort();
        // Not perfect — 10 still sorts before 2 — but 1, 10, 2, 3 beats the
        // arbitrary order the kanji give. See TOKENIZERS.md.
        assert_eq!(terms, ["10丁目", "1丁目", "2丁目", "3丁目"]);
    }

    #[test]
    fn a_query_goes_through_the_same_normalization_the_terms_did() {
        // Without this the index built from 一丁目 is unreachable by anyone
        // typing 一丁目, which is everyone.
        assert_eq!(
            Tokenizer::JaAddress.normalize_query("東京都千代田区丸の内一丁目"),
            "東京都千代田区丸の内1丁目"
        );
        assert_eq!(Tokenizer::Identity.normalize_query("一丁目"), "一丁目");
    }

    #[test]
    fn attribute_columns_come_from_extra_fields() {
        let corpus = parse(
            "a b\tP\t7\t-2\nc\tQ\t8\t3\n",
            Tokenizer::SpaceSplit,
            2,
            false,
        )
        .unwrap();
        assert_eq!(corpus.attributes, vec![vec!["7", "8"], vec!["-2", "3"]]);
    }

    #[test]
    fn a_line_missing_a_declared_column_is_an_error() {
        assert!(parse("a\tP\t7\n", Tokenizer::SpaceSplit, 2, false).is_err());
        assert!(parse("a\tP\t7\t1\t9\n", Tokenizer::SpaceSplit, 2, false).is_err());
    }

    #[test]
    fn bigram_splits_japanese_into_pairs() {
        assert_eq!(bigram("建築物"), ["建築", "築物"]);
        assert_eq!(bigram("都市モデル"), ["都市", "市モ", "モデ", "デル"]);
        assert_eq!(bigram("柱"), ["柱"]);
    }

    #[test]
    fn bigram_keeps_ascii_runs_whole() {
        assert_eq!(bigram("CityGML 3D"), ["citygml", "3d"]);
        // A mixed run is not all ASCII, so it goes to bigrams rather than being
        // split at the script boundary — which is what keeps the rule simple
        // enough to reimplement, at the cost of a bigram straddling it.
        assert_eq!(bigram("3D都市"), ["3d", "d都", "都市"]);
    }

    #[test]
    fn bigram_splits_on_everything_that_is_not_a_letter_or_digit() {
        assert_eq!(
            bigram("建築物、および 工作物"),
            ["建築", "築物", "およ", "よび", "工作", "作物"]
        );
    }

    #[test]
    fn bigram_composes_compatibility_forms() {
        assert_eq!(bigram("ﾓﾃﾞﾙ"), bigram("モデル"));
    }

    #[test]
    fn identifiers_round_trip() {
        for t in [
            Tokenizer::SpaceSplit,
            Tokenizer::Identity,
            Tokenizer::JaAddress,
            Tokenizer::Bigram,
        ] {
            assert_eq!(Tokenizer::parse(t.id()), Some(t));
        }
        assert_eq!(Tokenizer::parse("nope"), None);
    }
}
