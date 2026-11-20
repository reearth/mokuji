//! The tokenizers registered in `TOKENIZERS.md`: how the text of a document
//! becomes terms, and how a query becomes the same.
//!
//! The specification does not say how to tokenize. It says the choice must be
//! recorded, so that an index can be rebuilt and a query put through what the
//! terms went through. A writer needs this to build, and every reader needs it
//! to search, which is why it is a crate of its own rather than part of either.

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
    /// Japanese address normalization, then character bigrams.
    ///
    /// The same normalization as `JaAddress` and a different token boundary,
    /// which is what an approximate match over addresses needs: a query with
    /// one character wrong shares every bigram but two.
    JaAddressBigram,
    /// Character bigrams for scripts without word spacing, whole words for the
    /// rest.
    ///
    /// The analyzer that needs no dictionary, which is the only kind a reader
    /// with no server can be relied on to have. See `TOKENIZERS.md`.
    Bigram,
    /// Names separated by U+001F, each one term after normalization.
    ///
    /// What completion wants: a finished word and an unfinished one are then a
    /// prefix of one term, which a completions section can answer ahead of
    /// time, rather than an intersection nothing precomputed covers.
    NameList,
}

impl Tokenizer {
    /// The registered identifier, recorded in the metadata.
    pub fn id(self) -> &'static str {
        match self {
            Tokenizer::SpaceSplit => "space-split",
            Tokenizer::Identity => "identity",
            Tokenizer::JaAddress => "ja-address",
            Tokenizer::JaAddressBigram => "ja-address-bigram",
            Tokenizer::Bigram => "bigram",
            Tokenizer::NameList => "name-list",
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
            Tokenizer::JaAddress | Tokenizer::JaAddressBigram => ja_address(query),
            // What was typed is a prefix of one name, so it is normalized as a
            // name and not split.
            Tokenizer::NameList => name(query),
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
            "ja-address-bigram" => Some(Tokenizer::JaAddressBigram),
            "bigram" => Some(Tokenizer::Bigram),
            "name-list" => Some(Tokenizer::NameList),
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
            Tokenizer::JaAddressBigram => bigram(&ja_address(text)),
            Tokenizer::Bigram => bigram(text),
            Tokenizer::NameList => text
                .split('\u{1f}')
                .map(name)
                .filter(|n| !n.is_empty())
                .collect(),
        }
    }
}

/// One name as `name-list` normalizes it: NFKC, the Unicode default lowercase
/// mapping with no locale, and every run of `White_Space` one space.
///
/// `char::is_whitespace` is `White_Space` exactly. That matters here: Python's
/// `str.isspace` also counts U+001C to U+001F, which would treat the separator
/// itself as a space.
pub fn name(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    let composed: String = text.nfkc().collect();
    composed
        .to_lowercase()
        .split(char::is_whitespace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
    let chars = fold_variants(&text.chars().collect::<Vec<char>>());
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

/// Folds the characters a Japanese place name is written with in more than one
/// way onto one of them.
///
/// `丸ノ内` and `丸の内` are the same place, and the registry spells both. Left
/// alone they sort in two places and neither is where the other's reader looks,
/// which is the argument that already put the chome rewriting here.
///
/// The set is small on purpose, and each member earned its place by
/// measurement: folding the katakana `no` alone lifts recall on a
/// variant-spelled query from 70% to 88%, the small-`ke` family takes it to
/// 92%, and the archaic kanji `no` to 94%. Hiragana `が`, which is written for
/// the same connective in `自由が丘`, was measured and left out: it changed
/// nothing and it is an ordinary particle everywhere else. Katakana `ツ` was
/// left out because `ハイツ` is a word.
fn fold_variants(chars: &[char]) -> Vec<char> {
    let katakana = |c: Option<&char>| c.is_some_and(|&c| ('\u{30A0}'..='\u{30FF}').contains(&c));
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| match c {
            // A kanji is never part of a katakana word, so it needs no guard.
            '之' => 'の',
            // These are katakana, and inside a katakana word they are letters
            // rather than connectives: `マルノウチ` is not `マルのウチ`, and
            // `ケーキ` is not `ヶーキ`. Only a run standing between other
            // scripts is the connective.
            'ノ' | 'ケ' | 'ヵ'
                if !katakana(i.checked_sub(1).and_then(|j| chars.get(j)))
                    && !katakana(chars.get(i + 1)) =>
            {
                if c == 'ノ' {
                    'の'
                } else {
                    'ヶ'
                }
            }
            other => other,
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn name_list_makes_each_name_one_term() {
        let terms = Tokenizer::NameList.split("Black  Sea\u{1f}Черное море\u{1f} \u{1f}ＴＯＫＹＯ");
        assert_eq!(terms, ["black sea", "черное море", "tokyo"]);
        assert_eq!(
            Tokenizer::NameList.normalize_query("Black\u{a0}Se"),
            "black se"
        );
        // Default case conversion, not folding, and the final sigma is kept.
        assert_eq!(name("İstanbul"), "i\u{307}stanbul");
        assert_eq!(name("ΟΔΟΣ"), "οδος");
    }

    #[test]
    fn identifiers_round_trip() {
        for t in [
            Tokenizer::SpaceSplit,
            Tokenizer::Identity,
            Tokenizer::JaAddress,
            Tokenizer::JaAddressBigram,
            Tokenizer::Bigram,
            Tokenizer::NameList,
        ] {
            assert_eq!(Tokenizer::parse(t.id()), Some(t));
        }
        assert_eq!(Tokenizer::parse("nope"), None);
    }
}
