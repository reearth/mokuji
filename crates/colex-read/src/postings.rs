//! Postings decoding (`SPEC.md` §5.4).

use colex_codec::{bitpack, varint};
use colex_format::{Error, PostingsCodec, Result};

/// One term's postings: ascending document IDs with their frequencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Postings {
    /// Document IDs, strictly increasing.
    pub docs: Vec<u64>,
    /// Frequency per document, parallel to [`Postings::docs`].
    ///
    /// All ones when the file does not carry frequencies, so that callers need
    /// no separate case for an index that omits them.
    pub freqs: Vec<u64>,
}

impl Postings {
    /// Number of documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether the term occurs in no documents.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

/// Decodes one postings run.
///
/// `num_docs` bounds the document IDs: an index cannot refer to a document it
/// does not contain, and checking here keeps a corrupt run from producing a
/// payload lookup far outside the file.
pub fn decode(run: &[u8], has_freqs: bool, num_docs: u64) -> Result<Postings> {
    let codec = PostingsCodec::from_byte(*run.first().ok_or(Error::Truncated {
        expected: 1,
        found: 0,
    })?)?;
    if codec != PostingsCodec::DeltaBitpack {
        // Registered but not implemented here. Distinguished from an unknown
        // codec so the message says which of the two it is.
        return Err(Error::Malformed {
            detail: "postings codec is not implemented by this reader",
        });
    }

    let (count, n) = varint::decode(&run[1..])?;
    let count = usize::try_from(count).map_err(|_| Error::Malformed {
        detail: "postings count overflows",
    })?;
    let (values, mut pos) = bitpack::decode(run, 1 + n, count)?;

    let mut docs = Vec::with_capacity(count);
    let mut current: Option<u64> = None;
    for value in values {
        // The first value is a document ID; the rest are gaps less one, which
        // is what turns a run of consecutive documents into a run of zeros.
        let doc = match current {
            None => value,
            Some(previous) => previous
                .checked_add(value)
                .and_then(|d| d.checked_add(1))
                .ok_or(Error::Malformed {
                    detail: "document ID overflows",
                })?,
        };
        if doc >= num_docs {
            return Err(Error::Malformed {
                detail: "postings run refers to a document outside the index",
            });
        }
        current = Some(doc);
        docs.push(doc);
    }

    let freqs = if has_freqs {
        let (raw, next) = bitpack::decode(run, pos, count)?;
        pos = next;
        raw.into_iter().map(|v| v + 1).collect()
    } else {
        vec![1; count]
    };

    if pos != run.len() {
        return Err(Error::Malformed {
            detail: "trailing bytes in postings run",
        });
    }
    Ok(Postings { docs, freqs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_run_from_the_specification_appendix() {
        // The postings for `chuo` in conformance/fixtures/tiny.colex.
        let run = [0x00, 0x02, 0x01, 0x03, 0x00];
        let postings = decode(&run, true, 4).unwrap();
        assert_eq!(postings.docs, [1, 3]);
        assert_eq!(postings.freqs, [1, 1]);
    }

    #[test]
    fn a_single_document_run_costs_four_bytes() {
        let run = [0x00, 0x01, 0x00, 0x00];
        let postings = decode(&run, true, 4).unwrap();
        assert_eq!(postings.docs, [0]);
        assert_eq!(postings.freqs, [1]);
    }

    #[test]
    fn supplies_ones_when_the_file_omits_frequencies() {
        let run = [0x00, 0x02, 0x01, 0x03];
        let postings = decode(&run, false, 4).unwrap();
        assert_eq!(postings.freqs, [1, 1]);
    }

    #[test]
    fn rejects_a_document_outside_the_index() {
        let run = [0x00, 0x01, 0x03, 0x05, 0x00];
        assert!(matches!(
            decode(&run, true, 4),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let run = [0x00, 0x01, 0x00, 0x00, 0xff];
        assert!(matches!(
            decode(&run, true, 4),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_an_unimplemented_codec() {
        let run = [0x01, 0x00];
        assert!(matches!(
            decode(&run, true, 4),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_an_unknown_codec() {
        let run = [0xff, 0x00];
        assert!(matches!(
            decode(&run, true, 4),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_an_empty_run() {
        assert!(matches!(decode(&[], true, 4), Err(Error::Truncated { .. })));
    }
}
