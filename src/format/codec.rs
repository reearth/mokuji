use crate::format::{Error, Result};

/// Compression applied to each unit a reader addresses (`SPEC.md` §5.6).
///
/// The unit is a block for a block-addressed section and the whole of a section
/// that is read in one go. Never the file, and never a run of blocks:
/// compressing more than a reader can address would force it to fetch bytes it
/// does not want, which is the thing this format exists to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionCodec {
    /// Stored uncompressed.
    Raw = 0,
    /// Raw deflate, RFC 1951. Required.
    ///
    /// Chosen over a better-compressing algorithm because the format's reach
    /// depends on how easily it can be implemented: every language runtime has
    /// zlib, and browsers decompress raw deflate natively, so a reader in a
    /// page needs no bundled decompressor at all.
    Deflate = 1,
    /// zstd. Optional.
    ///
    /// Worth about 1.02x over deflate on term blocks and 1.4x on payload
    /// (measured; see `demo/addresses`). The gap is small enough that it is not
    /// worth requiring of every implementation, and large enough to be worth
    /// allowing.
    Zstd = 2,
}

impl SectionCodec {
    /// Parses the codec byte.
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => SectionCodec::Raw,
            1 => SectionCodec::Deflate,
            2 => SectionCodec::Zstd,
            _ => {
                return Err(Error::Malformed {
                    detail: "unknown section codec",
                })
            }
        })
    }

    /// Whether every conformant reader is required to implement this codec.
    pub fn is_required(self) -> bool {
        matches!(self, SectionCodec::Raw | SectionCodec::Deflate)
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
        for c in [SectionCodec::Raw, SectionCodec::Deflate, SectionCodec::Zstd] {
            assert_eq!(SectionCodec::from_byte(c as u8).unwrap(), c);
        }
    }

    #[test]
    fn rejects_unknown_section_codec() {
        assert!(SectionCodec::from_byte(3).is_err());
    }

    #[test]
    fn deflate_is_required_and_zstd_is_not() {
        assert!(SectionCodec::Raw.is_required());
        assert!(SectionCodec::Deflate.is_required());
        assert!(!SectionCodec::Zstd.is_required());
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

/// How a document's payload is written down (`SPEC.md` §5.5).
///
/// Not what it means. The format states the encoding and says nothing about the
/// subject, which is the line PMTiles draws around a tile: declare the type,
/// say nothing about the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadCodec {
    /// Bytes, returned as stored.
    Opaque = 0,
    /// A fixed number of signed integers per document, delta-coded against the
    /// previous document in the block.
    DeltaInts = 1,
}

impl PayloadCodec {
    /// Parses the codec byte.
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => PayloadCodec::Opaque,
            1 => PayloadCodec::DeltaInts,
            _ => {
                return Err(Error::Malformed {
                    detail: "unknown payload codec",
                })
            }
        })
    }
}

/// Maps a signed value onto an unsigned one, keeping small negatives small.
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// The inverse of [`zigzag`].
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn payload_codec_round_trips() {
        for c in [PayloadCodec::Opaque, PayloadCodec::DeltaInts] {
            assert_eq!(PayloadCodec::from_byte(c as u8).unwrap(), c);
        }
        assert!(PayloadCodec::from_byte(2).is_err());
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [
            0,
            1,
            -1,
            2,
            -2,
            i64::MAX,
            i64::MIN,
            139_765_575,
            -35_678_505,
        ] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn zigzag_keeps_small_negatives_small() {
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
        assert!(zigzag(-3) < 8);
    }
}
