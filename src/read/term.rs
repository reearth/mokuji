//! Term dictionary lookup (`SPEC.md` §5.3).

use crate::codec::{front_coded, varint};
use crate::format::{Error, Result};

use crate::read::postings::Postings;

/// A `postings_len` meaning the term names the document with its own ID
/// (`SPEC.md` §5.3.4).
pub const IMPLIED: u64 = 0;
/// A `postings_len` meaning the postings follow in the entry itself.
pub const INLINE: u64 = 1;

/// What the dictionary knows about one term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermEntry {
    /// Dense sequential ID, carried by position rather than stored.
    pub term_id: u64,
    /// Number of documents containing the term.
    pub doc_freq: u64,
    /// Absolute offset of the term's postings run.
    pub postings_offset: u64,
    /// The `postings_len` field as stored: the run's length in bytes, or
    /// [`IMPLIED`], or [`INLINE`]. [`TermEntry::run_len`] is the number of
    /// bytes of the postings section the term occupies.
    pub postings_len: u64,
    /// The postings, when the entry carries them inline.
    pub inline: Option<Postings>,
    /// Absolute offset of the term's positions run, if the file has one.
    pub positions_offset: u64,
    /// Length of that run in bytes; zero when the file carries no positions.
    pub positions_len: u64,
}

impl TermEntry {
    /// Bytes of the postings section this term's postings occupy: none when
    /// the entry states them itself, implied or inline.
    pub fn run_len(&self) -> u64 {
        match self.postings_len {
            IMPLIED | INLINE => 0,
            len => len,
        }
    }
}

/// What a term block's entries depend on beyond the block itself.
#[derive(Debug, Clone, Copy)]
pub struct BlockShape {
    /// Whether entries carry positions fields (`HAS_POSITIONS`).
    pub positions: bool,
    /// Whether postings carry frequencies (`HAS_FREQS`).
    pub freqs: bool,
    /// The document count, which bounds every document an entry may name.
    pub num_docs: u64,
}

/// The resident index over block boundary keys.
#[derive(Debug)]
pub struct TermIndex<'a> {
    bytes: &'a [u8],
    block_count: u32,
}

impl<'a> TermIndex<'a> {
    /// Wraps the term index section.
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let head = bytes.get(0..4).ok_or(Error::Truncated {
            expected: 4,
            found: bytes.len(),
        })?;
        let block_count = u32::from_le_bytes(head.try_into().expect("checked length"));
        // Each boundary key costs at least two bytes, so a count larger than
        // the section could hold is a lie worth catching before anything acts
        // on it.
        if u64::from(block_count) * 2 > (bytes.len() - 4) as u64 {
            return Err(Error::Malformed {
                detail: "the term index claims more blocks than it could hold",
            });
        }
        Ok(TermIndex { bytes, block_count })
    }

    /// Number of term blocks.
    pub fn block_count(&self) -> u32 {
        self.block_count
    }

    /// Every boundary key, in order.
    ///
    /// Used to check that a block actually begins with the key the index
    /// claims for it, which a query has no reason to verify but a whole-file
    /// walk does.
    pub fn keys(&self) -> Result<Vec<Vec<u8>>> {
        let mut key = Vec::new();
        let mut pos = 4;
        // `block_count` is a u32 from the file. A key costs at least two bytes,
        // so the section's own length is the honest bound on how many there can
        // be, and the reservation is capped besides.
        let mut out = Vec::with_capacity((self.block_count as usize).min(self.bytes.len() / 2));
        for _ in 0..self.block_count {
            pos = front_coded::decode_into(self.bytes, pos, &mut key)?;
            out.push(key.clone());
        }
        Ok(out)
    }

    /// The ordinal of the only block that can contain `term`.
    ///
    /// `None` means `term` sorts before the first block and is therefore
    /// absent. A term that sorts after every boundary key still yields the last
    /// block: the index narrows the search to one block, it does not decide
    /// membership.
    pub fn locate(&self, term: &[u8]) -> Result<Option<u32>> {
        let mut key = Vec::new();
        let mut previous = Vec::new();
        let mut pos = 4;
        let mut found = None;
        for ordinal in 0..self.block_count {
            pos = front_coded::decode_into(self.bytes, pos, &mut key)?;
            // Checked rather than assumed: an unsorted index would make the
            // search below silently return the wrong block.
            if ordinal > 0 && key <= previous {
                return Err(Error::Malformed {
                    detail: "term index boundary keys are not strictly increasing",
                });
            }
            if key.as_slice() <= term {
                found = Some(ordinal);
            }
            previous.clear();
            previous.extend_from_slice(&key);
        }
        Ok(found)
    }
}

/// Reads a term block's entries in order, one at a time.
///
/// Both a lookup and a whole-block decode walk entries the same way, and every
/// entry depends on the ones before it — its key is front-coded against the
/// previous key, its offsets are deltas, and inline postings are predicted from
/// the last document an earlier entry named — so the walk is written once.
struct Entries<'b> {
    block: &'b [u8],
    shape: BlockShape,
    pos: usize,
    count: u64,
    next: u64,
    first_term_id: u64,
    key: Vec<u8>,
    postings_offset: Option<u64>,
    previous_positions: Option<u64>,
    /// One more than the last document an implied or inline entry named.
    expected: u64,
}

impl<'b> Entries<'b> {
    fn new(block: &'b [u8], shape: BlockShape) -> Result<Self> {
        let (count, mut pos) = varint::decode(block)?;
        let (first_term_id, next) = varint::decode(&block[pos..])?;
        pos += next;
        Ok(Entries {
            block,
            shape,
            pos,
            count,
            next: 0,
            first_term_id,
            key: Vec::new(),
            postings_offset: None,
            previous_positions: None,
            expected: first_term_id,
        })
    }

    fn varint(&mut self) -> Result<u64> {
        let (value, n) = varint::decode(self.block.get(self.pos..).unwrap_or(&[]))?;
        self.pos += n;
        Ok(value)
    }

    /// The next entry, its key left in `self.key`; `None` after the last.
    fn next(&mut self) -> Result<Option<TermEntry>> {
        if self.next == self.count {
            return Ok(None);
        }
        let ordinal = self.next;
        self.next += 1;

        // Front coding decodes in place over the previous key, so the previous
        // key is copied out first to check the order against.
        let previous = self.key.clone();
        self.pos = front_coded::decode_into(self.block, self.pos, &mut self.key)?;
        if ordinal > 0 && self.key <= previous {
            return Err(Error::Malformed {
                detail: "terms within a block are not strictly increasing",
            });
        }

        let term_id = self.first_term_id + ordinal;
        let doc_freq = self.varint()?;
        let delta = self.varint()?;
        let offset = match self.postings_offset {
            None => delta,
            Some(previous) => previous.checked_add(delta).ok_or(Error::Malformed {
                detail: "postings offset delta overflows",
            })?,
        };
        self.postings_offset = Some(offset);
        let postings_len = self.varint()?;

        let inline = match postings_len {
            IMPLIED => {
                self.expected = term_id.saturating_add(1);
                None
            }
            INLINE => Some(self.inline(doc_freq)?),
            _ => None,
        };

        let (positions_offset, positions_len) = if self.shape.positions {
            let delta = self.varint()?;
            let at = match self.previous_positions {
                None => delta,
                Some(previous) => previous.checked_add(delta).ok_or(Error::Malformed {
                    detail: "positions offset delta overflows",
                })?,
            };
            self.previous_positions = Some(at);
            (at, self.varint()?)
        } else {
            (0, 0)
        };

        Ok(Some(TermEntry {
            term_id,
            doc_freq,
            postings_offset: offset,
            postings_len,
            inline,
            positions_offset,
            positions_len,
        }))
    }

    /// Inline postings (`SPEC.md` §5.3.4): `doc_freq` documents, the first
    /// predicted from the entry before, then frequencies if the file has them.
    fn inline(&mut self, doc_freq: u64) -> Result<Postings> {
        if doc_freq == 0 {
            return Err(Error::Malformed {
                detail: "inline postings must name at least one document",
            });
        }
        // Bounded by the bytes left: every value costs at least one, so a
        // doc_freq beyond that is a lie, and is caught before it allocates.
        let remaining = (self.block.len() - self.pos.min(self.block.len())) as u64;
        if doc_freq > remaining {
            return Err(Error::Truncated {
                expected: doc_freq as usize,
                found: remaining as usize,
            });
        }
        let mut docs = Vec::with_capacity(doc_freq as usize);
        let first = self.varint()?;
        let doc = unzigzag(first)
            .checked_add(self.expected as i128)
            .filter(|&d| d >= 0)
            .ok_or(Error::Malformed {
                detail: "inline postings name a document before the first",
            })? as u64;
        docs.push(doc);
        for _ in 1..doc_freq {
            let gap = self.varint()?;
            let doc = docs
                .last()
                .expect("one pushed")
                .checked_add(gap)
                .and_then(|d| d.checked_add(1))
                .ok_or(Error::Malformed {
                    detail: "document ID overflows",
                })?;
            docs.push(doc);
        }
        if *docs.last().expect("non-empty") >= self.shape.num_docs {
            return Err(Error::Malformed {
                detail: "inline postings name a document outside the index",
            });
        }
        let freqs = if self.shape.freqs {
            let mut freqs = Vec::with_capacity(docs.len());
            for _ in 0..doc_freq {
                let value = self.varint()?;
                freqs.push(value.checked_add(1).ok_or(Error::Malformed {
                    detail: "frequency overflows",
                })?);
            }
            freqs
        } else {
            vec![1; docs.len()]
        };
        self.expected = docs.last().expect("non-empty") + 1;
        Ok(Postings { docs, freqs })
    }

    fn finish(&self) -> Result<()> {
        if self.pos != self.block.len() {
            return Err(Error::Malformed {
                detail: "trailing bytes in term block",
            });
        }
        Ok(())
    }
}

fn unzigzag(value: u64) -> i128 {
    let half = i128::from(value >> 1);
    if value & 1 == 0 {
        half
    } else {
        -half - 1
    }
}

/// Finds `term` in one term block, which is one range read.
pub fn find_in_block(block: &[u8], term: &[u8], shape: BlockShape) -> Result<Option<TermEntry>> {
    let mut entries = Entries::new(block, shape)?;
    while let Some(entry) = entries.next()? {
        // Terms ascend, so a key past the target settles the question without
        // decoding the rest of the block.
        match entries.key.as_slice().cmp(term) {
            std::cmp::Ordering::Equal => return Ok(Some(entry)),
            std::cmp::Ordering::Greater => return Ok(None),
            std::cmp::Ordering::Less => {}
        }
    }
    Ok(None)
}

/// Decodes every entry of a term block.
///
/// A query stops at the term it wants; this reads the block out in full, which
/// is what a verification pass needs.
pub fn decode_block(block: &[u8], shape: BlockShape) -> Result<Vec<(Vec<u8>, TermEntry)>> {
    let mut entries = Entries::new(block, shape)?;
    let mut out = Vec::with_capacity(entries.count.min(1024) as usize);
    while let Some(entry) = entries.next()? {
        out.push((entries.key.clone(), entry));
    }
    entries.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: BlockShape = BlockShape {
        positions: false,
        freqs: false,
        num_docs: 100,
    };

    fn varint(value: u64, out: &mut Vec<u8>) {
        varint::encode(value, out);
    }

    /// A block of three entries: implied, inline naming two documents, a run.
    fn block() -> Vec<u8> {
        let mut b = Vec::new();
        varint(3, &mut b); // entry_count
        varint(10, &mut b); // first_term_id

        // "a": implied, so document 10, and the next prediction is 11.
        front_coded::encode(b"a", b"", &mut b);
        varint(1, &mut b);
        varint(500, &mut b);
        varint(IMPLIED, &mut b);
        // "b": inline, documents 9 and 12: zigzag(9 - 11) = 3, then 12 - 9 - 1.
        front_coded::encode(b"b", b"a", &mut b);
        varint(2, &mut b);
        varint(0, &mut b);
        varint(INLINE, &mut b);
        varint(3, &mut b);
        varint(2, &mut b);
        // "c": a five-byte run at 500.
        front_coded::encode(b"c", b"b", &mut b);
        varint(4, &mut b);
        varint(0, &mut b);
        varint(5, &mut b);
        b
    }

    #[test]
    fn decodes_implied_inline_and_stored_postings() {
        let entries = decode_block(&block(), SHAPE).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].1.inline, None);
        assert_eq!(entries[0].1.run_len(), 0);
        let inline = entries[1].1.inline.as_ref().unwrap();
        assert_eq!(inline.docs, vec![9, 12]);
        assert_eq!(inline.freqs, vec![1, 1]);
        assert_eq!(entries[1].1.run_len(), 0);
        assert_eq!(entries[2].1.run_len(), 5);
        assert_eq!(entries[2].1.postings_offset, 500);
    }

    #[test]
    fn a_lookup_past_inline_postings_lands_on_the_right_entry() {
        let entry = find_in_block(&block(), b"c", SHAPE).unwrap().unwrap();
        assert_eq!(entry.term_id, 12);
        assert_eq!(entry.doc_freq, 4);
        assert!(find_in_block(&block(), b"bb", SHAPE).unwrap().is_none());
    }

    #[test]
    fn rejects_inline_postings_outside_the_index() {
        let shape = BlockShape {
            num_docs: 12,
            ..SHAPE
        };
        assert!(decode_block(&block(), shape).is_err());
    }

    #[test]
    fn rejects_inline_postings_that_claim_more_than_the_block_holds() {
        let mut b = Vec::new();
        varint(1, &mut b);
        varint(0, &mut b);
        front_coded::encode(b"a", b"", &mut b);
        varint(u64::MAX >> 1, &mut b);
        varint(0, &mut b);
        varint(INLINE, &mut b);
        varint(0, &mut b);
        assert!(decode_block(&b, SHAPE).is_err());
    }

    #[test]
    fn rejects_empty_inline_postings() {
        let mut b = Vec::new();
        varint(1, &mut b);
        varint(0, &mut b);
        front_coded::encode(b"a", b"", &mut b);
        varint(0, &mut b);
        varint(0, &mut b);
        varint(INLINE, &mut b);
        assert!(decode_block(&b, SHAPE).is_err());
    }
}
