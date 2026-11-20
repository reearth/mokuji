//! The completions section (`SPEC.md` §5.3.5).

use crate::codec::{front_coded, varint};
use crate::format::{Error, Result};

/// The documents to show first for each expensive prefix, decided when the file
/// was written.
///
/// Resident: a reader has to know whether a prefix has an entry before it reads
/// anything, since that is what tells it which of the two costs in §4.1.1 it is
/// about to pay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completions {
    /// Every prefix at least this many terms begin with has an entry; zero if
    /// the file promises nothing on this count.
    pub min_terms: u64,
    /// Every prefix whose terms' runs total at least this many bytes has an
    /// entry; zero if the file promises nothing on this count.
    pub max_span: u64,
    /// How many documents an entry lists at most.
    pub docs_per_entry: u64,
    prefixes: Vec<Vec<u8>>,
    docs: Vec<Vec<u64>>,
}

impl Completions {
    /// Decodes the section, checking what a reader is required to check.
    pub fn decode(bytes: &[u8], num_docs: u64) -> Result<Self> {
        let mut pos = 0;
        let mut next = |bytes: &[u8]| -> Result<u64> {
            let (value, n) = varint::decode(bytes.get(pos..).unwrap_or_default())?;
            pos += n;
            Ok(value)
        };
        let min_terms = next(bytes)?;
        let max_span = next(bytes)?;
        let docs_per_entry = next(bytes)?;
        let count = next(bytes)?;
        if docs_per_entry == 0 {
            return Err(Error::Malformed {
                detail: "a completion entry lists no documents",
            });
        }

        // An entry costs at least three bytes, so the section's own length
        // bounds how many there can be before any are decoded.
        let bound = bytes.len() / 3;
        let mut prefixes: Vec<Vec<u8>> = Vec::with_capacity((count as usize).min(bound));
        let mut docs: Vec<Vec<u64>> = Vec::with_capacity((count as usize).min(bound));
        let mut key = Vec::new();
        for _ in 0..count {
            pos = front_coded::decode_into(bytes, pos, &mut key)?;
            if key.is_empty() {
                return Err(Error::Malformed {
                    detail: "a completion prefix is empty",
                });
            }
            if prefixes
                .last()
                .is_some_and(|last| key.as_slice() <= last.as_slice())
            {
                return Err(Error::Malformed {
                    detail: "completion prefixes are not strictly increasing",
                });
            }
            let (n, used) = varint::decode(bytes.get(pos..).unwrap_or_default())?;
            pos += used;
            if n == 0 || n > docs_per_entry {
                return Err(Error::Malformed {
                    detail: "a completion entry lists no documents, or more than it may",
                });
            }
            let mut listed = Vec::with_capacity((n as usize).min(bound));
            let mut seen = std::collections::HashSet::with_capacity((n as usize).min(bound));
            let mut last = 0u64;
            for _ in 0..n {
                let (value, used) = varint::decode(bytes.get(pos..).unwrap_or_default())?;
                pos += used;
                let delta = (value >> 1) as i64 ^ -((value & 1) as i64);
                let doc = last.wrapping_add(delta as u64);
                if doc >= num_docs {
                    return Err(Error::Malformed {
                        detail: "a completion lists a document outside the index",
                    });
                }
                if !seen.insert(doc) {
                    return Err(Error::Malformed {
                        detail: "a completion lists a document twice",
                    });
                }
                listed.push(doc);
                last = doc;
            }
            prefixes.push(key.clone());
            docs.push(listed);
        }
        if pos != bytes.len() {
            return Err(Error::Malformed {
                detail: "trailing bytes in the completions section",
            });
        }
        Ok(Completions {
            min_terms,
            max_span,
            docs_per_entry,
            prefixes,
            docs,
        })
    }

    /// The documents listed for exactly this prefix, in the order the writer
    /// chose, or `None` if it has no entry.
    pub fn get(&self, prefix: &[u8]) -> Option<&[u64]> {
        let at = self
            .prefixes
            .binary_search_by(|p| p.as_slice().cmp(prefix))
            .ok()?;
        Some(&self.docs[at])
    }

    /// Every entry, in prefix order.
    pub fn entries(&self) -> impl Iterator<Item = (&[u8], &[u64])> {
        self.prefixes
            .iter()
            .zip(&self.docs)
            .map(|(p, d)| (p.as_slice(), d.as_slice()))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.prefixes.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    /// Whether a prefix with no entry is known to be cheap, which is what makes
    /// the bound in `SPEC.md` §4.1.1 a number rather than a hope.
    pub fn covers(&self) -> bool {
        self.min_terms != 0 || self.max_span != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// min_terms 2, max_span 0, docs_per_entry 2, then the entries given.
    fn section(entries: &[(&[u8], &[u64])]) -> Vec<u8> {
        let mut out = vec![2, 0, 2, entries.len() as u8];
        let mut previous: &[u8] = b"";
        for (prefix, docs) in entries {
            front_coded::encode(prefix, previous, &mut out);
            previous = prefix;
            out.push(docs.len() as u8);
            let mut last = 0i64;
            for &doc in *docs {
                let delta = doc as i64 - last;
                out.push(((delta << 1) ^ (delta >> 63)) as u8);
                last = doc as i64;
            }
        }
        out
    }

    #[test]
    fn reads_what_it_was_given() {
        let c = Completions::decode(&section(&[(b"a", &[3, 1]), (b"ab", &[1])]), 4).unwrap();
        assert_eq!(c.get(b"a"), Some([3u64, 1].as_slice()));
        assert_eq!(c.get(b"ab"), Some([1u64].as_slice()));
        assert_eq!(c.get(b"b"), None);
        assert!(c.covers());
    }

    #[test]
    fn rejects_what_a_reader_must_reject() {
        let bad: [(&str, Vec<u8>); 6] = [
            ("empty prefix", section(&[(b"", &[0])])),
            ("out of order", section(&[(b"b", &[0]), (b"a", &[0])])),
            ("repeated prefix", section(&[(b"a", &[0]), (b"a", &[1])])),
            ("outside the index", section(&[(b"a", &[4])])),
            ("listed twice", section(&[(b"a", &[1, 1])])),
            ("more than it may", section(&[(b"a", &[0, 1, 2])])),
        ];
        for (why, bytes) in bad {
            assert!(Completions::decode(&bytes, 4).is_err(), "{why}");
        }
        let mut trailing = section(&[(b"a", &[0])]);
        trailing.push(0);
        assert!(Completions::decode(&trailing, 4).is_err());
        assert!(
            Completions::decode(&[1, 0, 0, 0], 4).is_err(),
            "no documents per entry"
        );
    }
}
