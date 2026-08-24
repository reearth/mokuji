//! The input format `colex build` reads, and the tokenizer it applies.
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
}

impl Tokenizer {
    /// The registered identifier, recorded in the metadata.
    pub fn id(self) -> &'static str {
        match self {
            Tokenizer::SpaceSplit => "space-split",
            Tokenizer::Identity => "identity",
        }
    }

    /// Parses a registered identifier.
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "space-split" => Some(Tokenizer::SpaceSplit),
            "identity" => Some(Tokenizer::Identity),
            _ => None,
        }
    }

    /// Splits `text` into terms.
    pub fn split(self, text: &str) -> Vec<&str> {
        match self {
            Tokenizer::SpaceSplit => text.split(' ').filter(|t| !t.is_empty()).collect(),
            Tokenizer::Identity => vec![text],
        }
    }
}

/// A parsed corpus, with its postings already inverted.
pub struct Corpus {
    /// Payloads in the order they appeared, which fixes the document IDs.
    pub payloads: Vec<Vec<u8>>,
    /// Term to document to frequency, sorted, ready to stream to the builder.
    pub postings: BTreeMap<Vec<u8>, BTreeMap<u64, u64>>,
}

/// Parses a tab-separated corpus.
///
/// A line without a tab is an error rather than a document with empty payload:
/// silently indexing a malformed file is how a corpus and its index drift
/// apart.
pub fn parse(text: &str, tokenizer: Tokenizer) -> Result<Corpus, String> {
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut postings: BTreeMap<Vec<u8>, BTreeMap<u64, u64>> = BTreeMap::new();

    for (line_number, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (body, payload) = line
            .split_once('\t')
            .ok_or_else(|| format!("line {} has no tab separator", line_number + 1))?;

        let doc = payloads.len() as u64;
        for token in tokenizer.split(body) {
            *postings
                .entry(token.as_bytes().to_vec())
                .or_default()
                .entry(doc)
                .or_default() += 1;
        }
        payloads.push(payload.as_bytes().to_vec());
    }

    if payloads.is_empty() {
        return Err("the corpus is empty".to_string());
    }
    Ok(Corpus { payloads, postings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_split(text: &str) -> Result<Corpus, String> {
        parse(text, Tokenizer::SpaceSplit)
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
    fn identity_keeps_the_line_whole() {
        let corpus = parse("東京都千代田区丸の内\t35.68,139.76\n", Tokenizer::Identity).unwrap();
        assert_eq!(corpus.postings.len(), 1);
        assert!(corpus
            .postings
            .contains_key("東京都千代田区丸の内".as_bytes()));
    }

    #[test]
    fn identifiers_round_trip() {
        for t in [Tokenizer::SpaceSplit, Tokenizer::Identity] {
            assert_eq!(Tokenizer::parse(t.id()), Some(t));
        }
        assert_eq!(Tokenizer::parse("nope"), None);
    }
}
