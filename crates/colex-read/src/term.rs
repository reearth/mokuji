//! Term dictionary lookup (`SPEC.md` §5.3).

use colex_codec::{front_coded, varint};
use colex_format::{Error, Result};

/// What the dictionary knows about one term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermEntry {
    /// Dense sequential ID, carried by position rather than stored.
    pub term_id: u64,
    /// Number of documents containing the term.
    pub doc_freq: u64,
    /// Absolute offset of the term's postings run.
    pub postings_offset: u64,
    /// Length of that run in bytes.
    pub postings_len: u64,
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
        let mut out = Vec::with_capacity(self.block_count as usize);
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

/// Finds `term` in one term block, which is one range read.
pub fn find_in_block(block: &[u8], term: &[u8]) -> Result<Option<TermEntry>> {
    let (count, mut pos) = varint::decode(block)?;
    let (first_term_id, next) = varint::decode(&block[pos..])?;
    pos += next;

    let mut key = Vec::new();
    let mut previous = Vec::new();
    let mut postings_offset: Option<u64> = None;
    for i in 0..count {
        pos = front_coded::decode_into(block, pos, &mut key)?;
        if i > 0 && key <= previous {
            return Err(Error::Malformed {
                detail: "terms within a block are not strictly increasing",
            });
        }

        let (doc_freq, n) = varint::decode(&block[pos..])?;
        pos += n;
        let (delta, n) = varint::decode(&block[pos..])?;
        pos += n;
        let offset = match postings_offset {
            None => delta,
            Some(previous) => previous.checked_add(delta).ok_or(Error::Malformed {
                detail: "postings offset delta overflows",
            })?,
        };
        postings_offset = Some(offset);
        let (postings_len, n) = varint::decode(&block[pos..])?;
        pos += n;

        // Terms ascend, so a key past the target settles the question without
        // decoding the rest of the block.
        if key.as_slice() == term {
            return Ok(Some(TermEntry {
                term_id: first_term_id + i,
                doc_freq,
                postings_offset: offset,
                postings_len,
            }));
        }
        if key.as_slice() > term {
            return Ok(None);
        }
        previous.clear();
        previous.extend_from_slice(&key);
    }
    Ok(None)
}

/// Decodes every entry of a term block.
///
/// A query stops at the term it wants; this reads the block out in full, which
/// is what a verification pass needs.
pub fn decode_block(block: &[u8]) -> Result<Vec<(Vec<u8>, TermEntry)>> {
    let (count, mut pos) = varint::decode(block)?;
    let (first_term_id, next) = varint::decode(&block[pos..])?;
    pos += next;

    let mut out = Vec::with_capacity(count.min(1024) as usize);
    let mut key = Vec::new();
    let mut postings_offset: Option<u64> = None;
    for i in 0..count {
        pos = front_coded::decode_into(block, pos, &mut key)?;
        if let Some((previous, _)) = out.last() {
            if &key <= previous {
                return Err(Error::Malformed {
                    detail: "terms within a block are not strictly increasing",
                });
            }
        }

        let (doc_freq, n) = varint::decode(&block[pos..])?;
        pos += n;
        let (delta, n) = varint::decode(&block[pos..])?;
        pos += n;
        let offset = match postings_offset {
            None => delta,
            Some(previous) => previous.checked_add(delta).ok_or(Error::Malformed {
                detail: "postings offset delta overflows",
            })?,
        };
        postings_offset = Some(offset);
        let (postings_len, n) = varint::decode(&block[pos..])?;
        pos += n;

        out.push((
            key.clone(),
            TermEntry {
                term_id: first_term_id + i,
                doc_freq,
                postings_offset: offset,
                postings_len,
            },
        ));
    }
    if pos != block.len() {
        return Err(Error::Malformed {
            detail: "trailing bytes in term block",
        });
    }
    Ok(out)
}
