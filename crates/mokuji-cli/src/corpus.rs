//! The input format `mokuji build` reads, and the tokenizer it applies.
//!
//! A tab-separated file: the text to index, then the payload to return. It is
//! deliberately the simplest thing that carries both halves, because the
//! interesting inputs arrive through `convert` from an existing tool rather
//! than through this.

use std::collections::BTreeMap;

pub use mokuji_tokenize::{bigram, ja_address, name, Tokenizer};

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
            for (column, field) in attributes.iter_mut().zip(&fields) {
                column.push((*field).to_string());
            }
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
    fn a_key_tokenizer_keeps_the_columns_of_the_lines_it_keeps() {
        let corpus = parse("b\tP\t7\nb\tQ\t8\na\tR\t9\n", Tokenizer::Identity, 1, false).unwrap();
        assert_eq!(corpus.duplicate_keys, 1);
        assert_eq!(corpus.attributes, vec![vec!["7", "9"]]);
    }

    #[test]
    fn a_line_missing_a_declared_column_is_an_error() {
        assert!(parse("a\tP\t7\n", Tokenizer::SpaceSplit, 2, false).is_err());
        assert!(parse("a\tP\t7\t1\t9\n", Tokenizer::SpaceSplit, 2, false).is_err());
    }
}
