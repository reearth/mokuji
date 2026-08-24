use crate::{Error, Result};

/// Compression applied to a whole section (`SPEC.md` §5.2).
///
/// Whole-section compression must not be used for a section a reader addresses
/// at block granularity, since decompressing it would require reading all of
/// it and defeat the point of range reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionCodec {
    /// Stored uncompressed.
    Raw = 0,
    /// zstd, self-contained.
    Zstd = 1,
    /// zstd against the shared dictionary section.
    ZstdWithDict = 2,
}

impl SectionCodec {
    /// Parses the codec byte.
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => SectionCodec::Raw,
            1 => SectionCodec::Zstd,
            2 => SectionCodec::ZstdWithDict,
            _ => {
                return Err(Error::Malformed {
                    detail: "unknown section codec",
                })
            }
        })
    }

    /// Whether a reader may address the section at block granularity.
    pub fn is_block_addressable(self) -> bool {
        matches!(self, SectionCodec::Raw)
    }
}

/// Encoding of a single term's postings run (`SPEC.md` §5.4).
///
/// The identifier is carried by the run rather than fixed for the whole file,
/// so a dense term can use a bitmap without every reader having to implement
/// one. [`PostingsCodec::DeltaBitpack`] is mandatory for all readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PostingsCodec {
    /// Delta-encoded document IDs with bit-packing. Required.
    DeltaBitpack = 0,
    /// Roaring bitmap. Optional.
    Roaring = 1,
}

impl PostingsCodec {
    /// Parses the codec byte at the start of a postings run.
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => PostingsCodec::DeltaBitpack,
            1 => PostingsCodec::Roaring,
            _ => {
                return Err(Error::Malformed {
                    detail: "unknown postings codec",
                })
            }
        })
    }

    /// Whether every conformant reader is required to implement this codec.
    pub fn is_required(self) -> bool {
        matches!(self, PostingsCodec::DeltaBitpack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_codec_round_trips() {
        for c in [
            SectionCodec::Raw,
            SectionCodec::Zstd,
            SectionCodec::ZstdWithDict,
        ] {
            assert_eq!(SectionCodec::from_byte(c as u8).unwrap(), c);
        }
    }

    #[test]
    fn rejects_unknown_section_codec() {
        assert!(SectionCodec::from_byte(3).is_err());
    }

    #[test]
    fn only_raw_sections_are_block_addressable() {
        assert!(SectionCodec::Raw.is_block_addressable());
        assert!(!SectionCodec::Zstd.is_block_addressable());
    }

    #[test]
    fn postings_codec_round_trips() {
        for c in [PostingsCodec::DeltaBitpack, PostingsCodec::Roaring] {
            assert_eq!(PostingsCodec::from_byte(c as u8).unwrap(), c);
        }
    }

    #[test]
    fn delta_bitpack_is_the_required_codec() {
        assert!(PostingsCodec::DeltaBitpack.is_required());
        assert!(!PostingsCodec::Roaring.is_required());
    }
}
