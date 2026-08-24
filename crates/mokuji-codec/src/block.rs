//! Compression of an addressed unit (`SPEC.md` §5.6).
//!
//! Compression is applied per unit a reader addresses — one block of a
//! block-addressed section, or the whole of a section that is read in one go —
//! never across the file. Compressing more than the reader can address would
//! force it to fetch bytes it does not want, which is the one thing the format
//! exists to avoid.
//!
//! A compressed unit carries its decompressed length in front, so that a reader
//! can bound the allocation before performing it rather than trusting a length
//! discovered mid-stream (`SPEC.md` §8).

use std::io::Write;

use flate2::write::{DeflateDecoder, DeflateEncoder};
use flate2::Compression;
use mokuji_format::{Error, Result};

use crate::varint;

/// Largest unit a reader will decompress.
///
/// A bound is required because the declared length is attacker-controlled. The
/// value is a reader's choice; it is far above any block a sensible writer
/// produces and far below anything that would exhaust memory.
pub const MAX_UNIT: usize = 16 * 1024 * 1024;

/// Compresses one unit with raw deflate (RFC 1951).
///
/// Raw rather than gzip or zlib framing: at block sizes of one or two
/// kilobytes, a per-block header and checksum is pure overhead, and the
/// integrity of the file is a matter for its content hash rather than for
/// every block to re-answer.
pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() / 2 + 16);
    varint::encode(bytes.len() as u64, &mut out);

    let mut encoder = DeflateEncoder::new(out, Compression::best());
    encoder.write_all(bytes).map_err(|_| Error::Malformed {
        detail: "deflate failed",
    })?;
    encoder.finish().map_err(|_| Error::Malformed {
        detail: "deflate failed",
    })
}

/// Decompresses one unit.
pub fn decompress(unit: &[u8]) -> Result<Vec<u8>> {
    let (len, pos) = varint::decode(unit)?;
    let len = usize::try_from(len).map_err(|_| Error::Malformed {
        detail: "decompressed length overflows",
    })?;
    if len > MAX_UNIT {
        return Err(Error::Malformed {
            detail: "decompressed length exceeds what this reader will allocate",
        });
    }

    let mut decoder = DeflateDecoder::new(Vec::with_capacity(len));
    decoder
        .write_all(&unit[pos..])
        .map_err(|_| Error::Malformed {
            detail: "unit is not valid deflate",
        })?;
    let out = decoder.finish().map_err(|_| Error::Malformed {
        detail: "unit is not valid deflate",
    })?;

    // The declared length is checked against the result rather than trusted:
    // they disagree only if the file is corrupt, and a reader that carried on
    // would hand its caller a block of the wrong size.
    if out.len() != len {
        return Err(Error::Malformed {
            detail: "unit does not decompress to its declared length",
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(bytes: &[u8]) {
        assert_eq!(decompress(&compress(bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn round_trips() {
        round_trip(b"");
        round_trip(b"a");
        round_trip(&[0u8; 4096]);
        round_trip("東京都千代田区丸の内一丁目1番地".repeat(64).as_bytes());
    }

    #[test]
    fn round_trips_incompressible_bytes() {
        // A linear congruential sequence: no structure to find, so the output
        // is larger than the input. A writer stores such a unit raw instead.
        let noise: Vec<u8> = (0..4096u32)
            .scan(1u32, |s, _| {
                *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                Some((*s >> 24) as u8)
            })
            .collect();
        round_trip(&noise);
    }

    #[test]
    fn compression_is_deterministic() {
        let bytes = "東京都".repeat(500);
        assert_eq!(
            compress(bytes.as_bytes()).unwrap(),
            compress(bytes.as_bytes()).unwrap()
        );
    }

    #[test]
    fn rejects_a_length_beyond_what_it_will_allocate() {
        let mut unit = Vec::new();
        varint::encode(MAX_UNIT as u64 + 1, &mut unit);
        assert!(matches!(decompress(&unit), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_a_wrong_declared_length() {
        let mut unit = compress(b"tokyo").unwrap();
        unit[0] = 4;
        assert!(matches!(decompress(&unit), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_corrupt_input() {
        let mut unit = compress(&[7u8; 512]).unwrap();
        let last = unit.len() - 1;
        unit[last] ^= 0xff;
        assert!(decompress(&unit).is_err());
    }
}
