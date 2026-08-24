//! The input format `colex build` reads, and the tokenizer it applies.
//!
//! A tab-separated file: the text to index, then the payload to return. It is
//! deliberately the simplest thing that carries both halves, because the
//! interesting inputs arrive through `convert` from an existing tool rather
//! than through this.

use std::collections::BTreeMap;

/// The tokenizer identifier recorded in the metadata.
///
/// The specification does not say how to tokenize; it says the choice must be
/// recorded so that the derivation is repeatable. This is the identifier for
/// splitting on single spaces, which is enough for a tab-separated corpus and
/// honest about being nothing more.
pub const TOKENIZER_ID: &str = "space-split";

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
pub fn parse(text: &str) -> Result<Corpus, String> {
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
        for token in tokenize(body) {
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

/// Splits on single spaces, dropping empties.
pub fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    text.split(' ').filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_a_corpus() {
        let corpus = parse("tokyo chuo\t1\nosaka chuo\t2\n").unwrap();
        assert_eq!(corpus.payloads.len(), 2);
        assert_eq!(corpus.postings[b"chuo".as_slice()], [(0, 1), (1, 1)].into());
        assert_eq!(corpus.postings[b"tokyo".as_slice()], [(0, 1)].into());
    }

    #[test]
    fn counts_repeated_terms() {
        let corpus = parse("a a a\tp\n").unwrap();
        assert_eq!(corpus.postings[b"a".as_slice()][&0], 3);
    }

    #[test]
    fn skips_blank_lines_but_not_malformed_ones() {
        assert!(parse("a\tp\n\nb\tq\n").is_ok());
        assert!(parse("a\tp\nno tab here\n").is_err());
    }

    #[test]
    fn rejects_an_empty_corpus() {
        assert!(parse("").is_err());
    }

    #[test]
    fn drops_empty_tokens() {
        assert_eq!(tokenize("a  b ").collect::<Vec<_>>(), ["a", "b"]);
    }
}
