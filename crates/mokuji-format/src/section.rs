use crate::{Error, Result, SectionCodec};

/// Size of one section table entry in bytes.
pub const SECTION_ENTRY_LEN: usize = 24;

/// Identifier of a section (`SPEC.md` §5.2).
///
/// Identifiers below `0x8000` are reserved for the specification; those at or
/// above it are application-defined and must be ignored by readers that do not
/// recognize them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionId(pub u16);

impl SectionId {
    /// Provenance, tokenizer, and build parameters, as canonical CBOR.
    pub const META: SectionId = SectionId(0x0001);
    /// FST over term block boundary keys.
    pub const TERM_INDEX: SectionId = SectionId(0x0010);
    /// Front-coded term blocks.
    pub const TERM_BLOCKS: SectionId = SectionId(0x0011);
    /// Term block ordinal to offset and length.
    pub const TERM_BLOCK_OFFSETS: SectionId = SectionId(0x0012);
    /// Postings runs, one per term.
    pub const POSTINGS: SectionId = SectionId(0x0020);
    /// Per-document payload in document ID order.
    pub const PAYLOAD: SectionId = SectionId(0x0030);
    /// Document ID to payload block and index.
    pub const PAYLOAD_OFFSETS: SectionId = SectionId(0x0031);
    /// Length in tokens of each document, in document ID order. Optional.
    pub const DOC_LENGTHS: SectionId = SectionId(0x0032);
    /// Named columns of numbers, one value per document. Optional.
    pub const DOC_ATTRIBUTES: SectionId = SectionId(0x0033);
    /// Where each term occurs within each document. Optional; requires freqs.
    pub const POSITIONS: SectionId = SectionId(0x0060);

    /// Lowest identifier available for application-specific use.
    pub const APPLICATION_MIN: u16 = 0x8000;

    /// Whether this identifier is application-defined rather than specified.
    pub fn is_application(self) -> bool {
        self.0 >= Self::APPLICATION_MIN
    }

    /// A short name for diagnostics, or `None` for an unrecognized identifier.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::META => "meta",
            Self::TERM_INDEX => "term_index",
            Self::TERM_BLOCKS => "term_blocks",
            Self::TERM_BLOCK_OFFSETS => "term_block_offsets",
            Self::POSTINGS => "postings",
            Self::PAYLOAD => "payload",
            Self::PAYLOAD_OFFSETS => "payload_offsets",
            Self::DOC_LENGTHS => "doc_lengths",
            Self::DOC_ATTRIBUTES => "doc_attributes",
            Self::POSITIONS => "positions",
            _ => return None,
        })
    }
}

/// One entry of the section table (`SPEC.md` §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionEntry {
    /// Which section this entry describes.
    pub id: SectionId,
    /// Compression applied to the section as a whole.
    pub codec: SectionCodec,
    /// Absolute offset of the section.
    pub offset: u64,
    /// Length of the section in bytes.
    pub length: u64,
}

impl SectionEntry {
    /// Parses one entry from the start of `buf`.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < SECTION_ENTRY_LEN {
            return Err(Error::Truncated {
                expected: SECTION_ENTRY_LEN,
                found: buf.len(),
            });
        }
        let id = SectionId(u16::from_le_bytes(
            buf[0..2].try_into().expect("checked length"),
        ));
        let codec = SectionCodec::from_byte(buf[2])?;
        if buf[3..8].iter().any(|&b| b != 0) {
            return Err(Error::Malformed {
                detail: "reserved section entry bytes are non-zero",
            });
        }
        let offset = u64::from_le_bytes(buf[8..16].try_into().expect("checked length"));
        let length = u64::from_le_bytes(buf[16..24].try_into().expect("checked length"));
        Ok(SectionEntry {
            id,
            codec,
            offset,
            length,
        })
    }

    /// Encodes one entry.
    pub fn encode(&self) -> [u8; SECTION_ENTRY_LEN] {
        let mut out = [0u8; SECTION_ENTRY_LEN];
        out[0..2].copy_from_slice(&self.id.0.to_le_bytes());
        out[2] = self.codec as u8;
        out[8..16].copy_from_slice(&self.offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.length.to_le_bytes());
        out
    }

    /// Checks that this section lies within a file of `file_len` bytes.
    ///
    /// Section offsets come from untrusted input, so this must be called
    /// before the range is used (`SPEC.md` §8).
    pub fn check_bounds(&self, file_len: u64) -> Result<()> {
        let end = self
            .offset
            .checked_add(self.length)
            .ok_or(Error::OutOfBounds {
                offset: self.offset,
                len: self.length,
                file_len,
            })?;
        if end > file_len {
            return Err(Error::OutOfBounds {
                offset: self.offset,
                len: self.length,
                file_len,
            });
        }
        Ok(())
    }
}

/// Parses a whole section table.
///
/// The table must be sorted by identifier with no duplicates, so that a reader
/// can look up a section without trusting the writer to have been careful.
pub fn parse_section_table(buf: &[u8], count: u32) -> Result<Vec<SectionEntry>> {
    let count = count as usize;
    let needed = count
        .checked_mul(SECTION_ENTRY_LEN)
        .ok_or(Error::Malformed {
            detail: "section count overflows",
        })?;
    if buf.len() < needed {
        return Err(Error::Truncated {
            expected: needed,
            found: buf.len(),
        });
    }

    // Safe to reserve: `needed` was already checked against the buffer.
    let mut entries = Vec::with_capacity(count);
    let mut prev: Option<SectionId> = None;
    for chunk in buf[..needed].chunks_exact(SECTION_ENTRY_LEN) {
        let entry = SectionEntry::parse(chunk)?;
        if let Some(prev) = prev {
            if entry.id <= prev {
                return Err(Error::Malformed {
                    detail: "section table is not sorted by id, or has duplicates",
                });
            }
        }
        prev = Some(entry.id);
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u16, offset: u64) -> SectionEntry {
        SectionEntry {
            id: SectionId(id),
            codec: SectionCodec::Raw,
            offset,
            length: 16,
        }
    }

    #[test]
    fn round_trips() {
        let e = SectionEntry {
            codec: SectionCodec::Deflate,
            ..entry(0x0020, 4096)
        };
        assert_eq!(SectionEntry::parse(&e.encode()).unwrap(), e);
    }

    #[test]
    fn rejects_non_zero_reserved_bytes() {
        let mut buf = entry(0x0001, 0).encode();
        buf[5] = 1;
        assert!(matches!(
            SectionEntry::parse(&buf),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn parses_sorted_table() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&entry(0x0001, 0).encode());
        buf.extend_from_slice(&entry(0x0010, 16).encode());
        let table = parse_section_table(&buf, 2).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table[1].id, SectionId::TERM_INDEX);
    }

    #[test]
    fn rejects_unsorted_table() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&entry(0x0010, 0).encode());
        buf.extend_from_slice(&entry(0x0001, 16).encode());
        assert!(matches!(
            parse_section_table(&buf, 2),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&entry(0x0010, 0).encode());
        buf.extend_from_slice(&entry(0x0010, 16).encode());
        assert!(matches!(
            parse_section_table(&buf, 2),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_table_shorter_than_count() {
        let buf = entry(0x0001, 0).encode();
        assert!(matches!(
            parse_section_table(&buf, 2),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn bounds_check_rejects_overflow() {
        let e = SectionEntry {
            offset: u64::MAX,
            length: 1,
            ..entry(0x0001, 0)
        };
        assert!(matches!(
            e.check_bounds(1024),
            Err(Error::OutOfBounds { .. })
        ));
    }

    #[test]
    fn bounds_check_rejects_past_end() {
        let e = entry(0x0001, 1020);
        assert!(matches!(
            e.check_bounds(1024),
            Err(Error::OutOfBounds { .. })
        ));
    }

    #[test]
    fn bounds_check_accepts_exact_end() {
        assert!(entry(0x0001, 1008).check_bounds(1024).is_ok());
    }

    #[test]
    fn application_ids_are_recognized_as_such() {
        assert!(SectionId(0x8000).is_application());
        assert!(!SectionId::POSTINGS.is_application());
        assert_eq!(SectionId(0x8000).name(), None);
    }
}
