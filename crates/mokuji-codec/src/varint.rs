//! LEB128 variable length integers.
//!
//! Used for the length-prefixed and delta-encoded fields inside blocks, where
//! values are small and a fixed width would dominate the block.

use mokuji_format::{Error, Result};

/// Maximum number of bytes a `u64` can occupy.
pub const MAX_LEN: usize = 10;

/// Appends `value` to `out`, returning the number of bytes written.
pub fn encode(value: u64, out: &mut Vec<u8>) -> usize {
    let mut value = value;
    let mut written = 0;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        written += 1;
        if value == 0 {
            return written;
        }
    }
}

/// Reads a value from the start of `buf`, returning it and its encoded length.
///
/// Rejects encodings longer than [`MAX_LEN`] and encodings whose final byte
/// would overflow a `u64`, so that a malformed file cannot be read as a
/// different value than it encodes.
pub fn decode(buf: &[u8]) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in buf.iter().take(MAX_LEN).enumerate() {
        let payload = u64::from(byte & 0x7f);
        if shift == 63 && payload > 1 {
            return Err(Error::Malformed {
                detail: "varint overflows u64",
            });
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    if buf.len() >= MAX_LEN {
        Err(Error::Malformed {
            detail: "varint is longer than ten bytes",
        })
    } else {
        Err(Error::Truncated {
            expected: buf.len() + 1,
            found: buf.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: u64) {
        let mut buf = Vec::new();
        let written = encode(value, &mut buf);
        assert_eq!(written, buf.len());
        assert_eq!(decode(&buf).unwrap(), (value, buf.len()));
    }

    #[test]
    fn round_trips_boundaries() {
        for value in [0, 1, 127, 128, 300, u32::MAX as u64, u64::MAX - 1, u64::MAX] {
            round_trip(value);
        }
    }

    #[test]
    fn round_trips_powers_of_two() {
        for shift in 0..64 {
            round_trip(1u64 << shift);
        }
    }

    #[test]
    fn uses_minimal_width() {
        let mut buf = Vec::new();
        encode(127, &mut buf);
        assert_eq!(buf.len(), 1);
        buf.clear();
        encode(128, &mut buf);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn decodes_from_the_start_of_a_longer_buffer() {
        let mut buf = Vec::new();
        encode(300, &mut buf);
        buf.extend_from_slice(b"trailing");
        assert_eq!(decode(&buf).unwrap(), (300, 2));
    }

    #[test]
    fn rejects_truncation() {
        assert!(matches!(decode(&[0x80]), Err(Error::Truncated { .. })));
        assert!(matches!(decode(&[]), Err(Error::Truncated { .. })));
    }

    #[test]
    fn rejects_overflow() {
        let buf = [0x80u8, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        assert!(matches!(decode(&buf), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_over_long_encoding() {
        let buf = [0x80u8; MAX_LEN + 1];
        assert!(matches!(decode(&buf), Err(Error::Malformed { .. })));
    }
}
