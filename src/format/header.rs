use crate::format::{Error, Result};

/// Magic bytes at offset 0 of every Mokuji file: `MOKUJI` and two zero bytes.
///
/// The version is in `spec_major` and `spec_minor` alone (`SPEC.md` §5.1).
pub const MAGIC: [u8; 8] = *b"MOKUJI\0\0";

/// The magic of the files written before it lost its version, which a 0.x
/// reader still reads (`SPEC.md` Appendix A). Nothing writes it.
pub const MAGIC_V0: [u8; 8] = *b"MOKUJIv0";

/// Major specification version implemented by this crate.
///
/// A file with a different major version is rejected.
pub const SPEC_MAJOR: u8 = 0;

/// Minor specification version implemented by this crate.
///
/// A file with a *higher* minor version is accepted; unknown sections in it
/// are ignored.
pub const SPEC_MINOR: u8 = 1;

/// Size of the fixed header in bytes.
pub const HEADER_LEN: usize = 64;

/// Header flag bits (`SPEC.md` §5.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(u16);

impl Flags {
    /// A payload section is present.
    pub const HAS_PAYLOAD: Flags = Flags(1 << 1);
    /// Postings runs carry a term frequency per document.
    ///
    /// An index whose terms occur at most once per document — an address
    /// dictionary, typically — omits them entirely rather than storing a run
    /// of ones.
    pub const HAS_FREQS: Flags = Flags(1 << 2);
    /// A positions section is present, and dictionary entries locate it.
    ///
    /// What a phrase query needs and nothing else does. Entries carry two more
    /// varints when this is set, so a reader must know before it parses one.
    pub const HAS_POSITIONS: Flags = Flags(1 << 3);

    // Bit 0 named a monolithic transducer mode that was never specified and
    // has been withdrawn; it stays reserved so nothing collides with a reader
    // written against the earlier draft.
    const KNOWN: u16 = 0b1110;

    /// Builds flags from the raw field, rejecting reserved bits.
    pub fn from_bits(bits: u16) -> Result<Self> {
        if bits & !Self::KNOWN != 0 {
            return Err(Error::Malformed {
                detail: "reserved flag bit set",
            });
        }
        Ok(Flags(bits))
    }

    /// The raw field.
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Whether every bit in `other` is set.
    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns these flags with every bit in `other` set.
    pub fn with(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
}

/// The fixed 64-byte header (`SPEC.md` §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Major specification version of the file.
    pub spec_major: u8,
    /// Minor specification version of the file.
    pub spec_minor: u8,
    /// Layout flags.
    pub flags: Flags,
    /// Number of entries in the section table.
    pub section_count: u32,
    /// Number of documents in the index, `N` in BM25.
    pub num_docs: u64,
    /// Sum of all document lengths in tokens.
    ///
    /// Stored alongside [`Header::num_docs`] rather than as a precomputed
    /// average so that the header carries no floating-point value for
    /// implementations to disagree about.
    pub sum_doclen: u64,
    /// Absolute offset of the section table.
    pub section_table_offset: u64,
    /// Length of the section table in bytes.
    pub section_table_len: u32,
}

impl Header {
    /// Parses the fixed header from the start of `buf`.
    ///
    /// `buf` may be longer than the header; trailing bytes are ignored.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_LEN {
            return Err(Error::Truncated {
                expected: HEADER_LEN,
                found: buf.len(),
            });
        }

        let magic: [u8; 8] = buf[0..8].try_into().expect("checked length");
        if magic != MAGIC && magic != MAGIC_V0 {
            return Err(Error::BadMagic { found: magic });
        }

        let spec_major = buf[8];
        let spec_minor = buf[9];
        // A newer minor version is readable by construction: it may only add
        // sections, which an older reader ignores. A newer major version is
        // not.
        if spec_major != SPEC_MAJOR {
            return Err(Error::UnsupportedVersion {
                major: spec_major,
                minor: spec_minor,
            });
        }

        let flags = Flags::from_bits(u16(&buf[10..12]))?;
        let section_count = u32(&buf[12..16]);
        let num_docs = u64(&buf[16..24]);
        let sum_doclen = u64(&buf[24..32]);
        let section_table_offset = u64(&buf[32..40]);
        let section_table_len = u32(&buf[40..44]);

        if buf[44..HEADER_LEN].iter().any(|&b| b != 0) {
            return Err(Error::Malformed {
                detail: "reserved header bytes are non-zero",
            });
        }

        Ok(Header {
            spec_major,
            spec_minor,
            flags,
            section_count,
            num_docs,
            sum_doclen,
            section_table_offset,
            section_table_len,
        })
    }

    /// Encodes the fixed header.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(&MAGIC);
        out[8] = self.spec_major;
        out[9] = self.spec_minor;
        out[10..12].copy_from_slice(&self.flags.bits().to_le_bytes());
        out[12..16].copy_from_slice(&self.section_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.num_docs.to_le_bytes());
        out[24..32].copy_from_slice(&self.sum_doclen.to_le_bytes());
        out[32..40].copy_from_slice(&self.section_table_offset.to_le_bytes());
        out[40..44].copy_from_slice(&self.section_table_len.to_le_bytes());
        out
    }

    /// Average document length, or `None` for an empty index.
    ///
    /// This is the `avgdl` term of BM25. The format supplies it; how it is
    /// used is outside the specification.
    pub fn avg_doclen(&self) -> Option<f64> {
        if self.num_docs == 0 {
            None
        } else {
            Some(self.sum_doclen as f64 / self.num_docs as f64)
        }
    }
}

fn u16(b: &[u8]) -> u16 {
    u16::from_le_bytes(b.try_into().expect("checked length"))
}

fn u32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b.try_into().expect("checked length"))
}

fn u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b.try_into().expect("checked length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        Header {
            spec_major: SPEC_MAJOR,
            spec_minor: SPEC_MINOR,
            flags: Flags::HAS_PAYLOAD,
            section_count: 7,
            num_docs: 1_480_000,
            sum_doclen: 8_880_000,
            section_table_offset: 64,
            section_table_len: 7 * 24,
        }
    }

    #[test]
    fn round_trips() {
        let h = sample();
        assert_eq!(Header::parse(&h.encode()).unwrap(), h);
    }

    #[test]
    fn ignores_trailing_bytes() {
        let mut buf = sample().encode().to_vec();
        buf.extend_from_slice(&[0xff; 32]);
        assert_eq!(Header::parse(&buf).unwrap(), sample());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = sample().encode();
        buf[0] = b'X';
        assert!(matches!(Header::parse(&buf), Err(Error::BadMagic { .. })));
    }

    #[test]
    fn reads_the_magic_that_named_its_version() {
        let mut buf = sample().encode();
        assert_eq!(&buf[0..8], b"MOKUJI\0\0");
        buf[0..8].copy_from_slice(&MAGIC_V0);
        assert_eq!(Header::parse(&buf).unwrap(), sample());
    }

    #[test]
    fn rejects_future_major_version() {
        let mut buf = sample().encode();
        buf[8] = SPEC_MAJOR + 1;
        assert!(matches!(
            Header::parse(&buf),
            Err(Error::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn accepts_future_minor_version() {
        let mut buf = sample().encode();
        buf[9] = SPEC_MINOR + 9;
        assert_eq!(Header::parse(&buf).unwrap().spec_minor, SPEC_MINOR + 9);
    }

    #[test]
    fn accepts_every_defined_flag() {
        let all = Flags::HAS_PAYLOAD
            .with(Flags::HAS_FREQS)
            .with(Flags::HAS_POSITIONS);
        let mut buf = sample().encode();
        buf[10..12].copy_from_slice(&all.bits().to_le_bytes());
        assert_eq!(Header::parse(&buf).unwrap().flags, all);
    }

    #[test]
    fn rejects_reserved_flag_bits() {
        let mut buf = sample().encode();
        buf[10] = 0xff;
        assert!(matches!(Header::parse(&buf), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_non_zero_reserved_bytes() {
        let mut buf = sample().encode();
        buf[63] = 1;
        assert!(matches!(Header::parse(&buf), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_truncation() {
        let buf = sample().encode();
        assert!(matches!(
            Header::parse(&buf[..HEADER_LEN - 1]),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn avg_doclen_is_none_when_empty() {
        let h = Header {
            num_docs: 0,
            sum_doclen: 0,
            ..sample()
        };
        assert_eq!(h.avg_doclen(), None);
    }
}
