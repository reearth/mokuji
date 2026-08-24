//! Bit-packed groups (`SPEC.md` §5.4.1).
//!
//! A sequence is split into groups of up to [`GROUP_LEN`] values, each carrying
//! the smallest bit width that represents every value in it. Width is chosen
//! per group rather than per sequence so that a run of equal values — which is
//! what delta-encoded consecutive document IDs become — collapses to width
//! zero and occupies no bytes at all.

use colex_format::{Error, Result};

/// Maximum number of values in one group.
pub const GROUP_LEN: usize = 128;

/// Decodes one group of `count` values, returning them and the position after.
///
/// Rejects a width larger than necessary and non-zero padding, because
/// `SPEC.md` §3.4 requires that an input have exactly one valid encoding. A
/// reader that accepted a wider-than-needed field would make a byte-identical
/// rebuild unverifiable.
pub fn decode_group(buf: &[u8], pos: usize, count: usize) -> Result<(Vec<u64>, usize)> {
    debug_assert!(count <= GROUP_LEN);
    let width = *buf.get(pos).ok_or(Error::Truncated {
        expected: pos + 1,
        found: buf.len(),
    })?;
    if width > 64 {
        return Err(Error::Malformed {
            detail: "bit width exceeds 64",
        });
    }

    let bits = count * width as usize;
    let nbytes = bits.div_ceil(8);
    let start = pos + 1;
    let packed = buf.get(start..start + nbytes).ok_or(Error::Truncated {
        expected: start + nbytes,
        found: buf.len(),
    })?;

    // The count is bounded by the caller here, but the group is decoded from
    // file bytes either way, so the reservation stays bounded.
    let mut values = Vec::with_capacity(count.min(GROUP_LEN));
    let mut widest = 0;
    for i in 0..count {
        let value = get_bits(packed, i * width as usize, width);
        widest = widest.max(u8::try_from(64 - value.leading_zeros()).expect("at most 64"));
        values.push(value);
    }
    if widest != width {
        return Err(Error::Malformed {
            detail: "bit width is not the smallest that fits the group",
        });
    }
    if bits % 8 != 0 && packed[nbytes - 1] >> (bits % 8) != 0 {
        return Err(Error::Malformed {
            detail: "padding bits are not zero",
        });
    }

    Ok((values, start + nbytes))
}

/// Decodes `count` values across as many groups as they occupy.
pub fn decode(buf: &[u8], mut pos: usize, count: usize) -> Result<(Vec<u64>, usize)> {
    // Reserve a bounded amount: `count` comes from the file, so trusting it
    // would let a varint ask for gigabytes. The vector grows as bytes arrive.
    let mut values = Vec::with_capacity(count.min(GROUP_LEN));
    while values.len() < count {
        let (group, next) = decode_group(buf, pos, (count - values.len()).min(GROUP_LEN))?;
        values.extend(group);
        pos = next;
    }
    Ok((values, pos))
}

/// Appends one group of at most [`GROUP_LEN`] values.
pub fn encode_group(values: &[u64], out: &mut Vec<u8>) {
    debug_assert!(values.len() <= GROUP_LEN);
    let width = values
        .iter()
        .map(|&v| u8::try_from(64 - v.leading_zeros()).expect("at most 64"))
        .max()
        .unwrap_or(0);
    out.push(width);
    if width == 0 {
        return;
    }

    let nbytes = (values.len() * width as usize).div_ceil(8);
    let base = out.len();
    out.resize(base + nbytes, 0);
    for (i, &value) in values.iter().enumerate() {
        set_bits(&mut out[base..], i * width as usize, width, value);
    }
}

/// Appends `values` as groups of at most [`GROUP_LEN`].
pub fn encode(values: &[u64], out: &mut Vec<u8>) {
    for group in values.chunks(GROUP_LEN) {
        encode_group(group, out);
    }
}

/// Reads `width` bits starting at bit `offset`, least significant bit first.
fn get_bits(bytes: &[u8], offset: usize, width: u8) -> u64 {
    if width == 0 {
        return 0;
    }
    // At most 64 bits plus a 7 bit misalignment, so at most 9 bytes, which is
    // why the accumulator is a u128.
    let first = offset / 8;
    let last = (offset + width as usize).div_ceil(8);
    let mut acc: u128 = 0;
    for (i, &byte) in bytes[first..last].iter().enumerate() {
        acc |= u128::from(byte) << (8 * i);
    }
    let mask = if width == 64 {
        u128::from(u64::MAX)
    } else {
        (1u128 << width) - 1
    };
    ((acc >> (offset % 8)) & mask) as u64
}

fn set_bits(bytes: &mut [u8], offset: usize, width: u8, value: u64) {
    for i in 0..width as usize {
        if value >> i & 1 == 1 {
            let bit = offset + i;
            bytes[bit / 8] |= 1 << (bit % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(values: &[u64]) {
        let mut buf = Vec::new();
        encode(values, &mut buf);
        let (decoded, pos) = decode(&buf, 0, values.len()).unwrap();
        assert_eq!(decoded, values, "buffer was {buf:02x?}");
        assert_eq!(pos, buf.len());
    }

    #[test]
    fn round_trips_widths() {
        round_trip(&[]);
        round_trip(&[0]);
        round_trip(&[1]);
        round_trip(&[0, 0, 0, 0]);
        round_trip(&[1, 1, 1]);
        round_trip(&[7, 3, 0, 5]);
        round_trip(&[u64::MAX]);
        round_trip(&[u64::MAX, 0, u64::MAX]);
        for shift in 0..64 {
            round_trip(&[1u64 << shift, 0, 1]);
        }
    }

    #[test]
    fn round_trips_across_group_boundaries() {
        let values: Vec<u64> = (0..GROUP_LEN as u64 * 2 + 5).map(|v| v % 7).collect();
        round_trip(&values);
    }

    /// A group of equal values costs one byte, which is the case the encoding
    /// exists for: consecutive document IDs delta-encode to zeros.
    #[test]
    fn a_group_of_zeros_occupies_one_byte() {
        let mut buf = Vec::new();
        encode(&[0; GROUP_LEN], &mut buf);
        assert_eq!(buf, vec![0]);
    }

    #[test]
    fn rejects_a_wider_than_necessary_width() {
        // Two values needing three bits, declared as four.
        let buf = [4u8, 0x57];
        assert!(matches!(
            decode_group(&buf, 0, 2),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_non_zero_padding() {
        let mut buf = Vec::new();
        encode_group(&[7, 3], &mut buf);
        let last = buf.len() - 1;
        buf[last] |= 0b1100_0000;
        assert!(matches!(
            decode_group(&buf, 0, 2),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_an_impossible_width() {
        assert!(matches!(
            decode_group(&[65, 0, 0], 0, 1),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_truncation() {
        assert!(matches!(
            decode_group(&[], 0, 1),
            Err(Error::Truncated { .. })
        ));
        assert!(matches!(
            decode_group(&[8, 0x01], 0, 4),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn decodes_from_the_middle_of_a_buffer() {
        let mut buf = vec![0xaa, 0xbb];
        encode(&[5, 6], &mut buf);
        let (values, pos) = decode(&buf, 2, 2).unwrap();
        assert_eq!(values, [5, 6]);
        assert_eq!(pos, buf.len());
    }
}
