//! Front-coded keys (`SPEC.md` §5.3.1).
//!
//! A key is stored as the number of leading bytes it shares with its
//! predecessor plus the rest. Keys are therefore decoded in order, which is why
//! the format searches across blocks in the resident term index and only scans
//! within one block.

use colex_format::{Error, Result};

use crate::varint;

/// Decodes the next key into `key`, which must hold the previous key.
///
/// Decoding in place is what makes a block scan allocation-free: the buffer is
/// truncated to the shared prefix and extended with the suffix.
pub fn decode_into(buf: &[u8], pos: usize, key: &mut Vec<u8>) -> Result<usize> {
    let (shared, pos) = varint::decode(&buf[pos.min(buf.len())..])
        .map(|(v, n)| (v, pos + n))
        .map_err(|_| Error::Truncated {
            expected: pos + 1,
            found: buf.len(),
        })?;
    let (len, pos) = varint::decode(&buf[pos.min(buf.len())..]).map(|(v, n)| (v, pos + n))?;

    let shared = usize::try_from(shared).map_err(|_| Error::Malformed {
        detail: "shared prefix length overflows",
    })?;
    let len = usize::try_from(len).map_err(|_| Error::Malformed {
        detail: "key suffix length overflows",
    })?;
    if shared > key.len() {
        return Err(Error::Malformed {
            detail: "shared prefix is longer than the preceding key",
        });
    }
    let suffix = buf.get(pos..pos + len).ok_or(Error::Truncated {
        expected: pos + len,
        found: buf.len(),
    })?;

    key.truncate(shared);
    key.extend_from_slice(suffix);
    Ok(pos + len)
}

/// Appends `key`, coded against `previous`.
pub fn encode(key: &[u8], previous: &[u8], out: &mut Vec<u8>) {
    let shared = key.iter().zip(previous).take_while(|(a, b)| a == b).count();
    varint::encode(shared as u64, out);
    varint::encode((key.len() - shared) as u64, out);
    out.extend_from_slice(&key[shared..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(keys: &[&str]) {
        let mut buf = Vec::new();
        let mut previous: Vec<u8> = Vec::new();
        for key in keys {
            encode(key.as_bytes(), &previous, &mut buf);
            previous = key.as_bytes().to_vec();
        }

        let mut decoded = Vec::new();
        let mut key = Vec::new();
        let mut pos = 0;
        while pos < buf.len() {
            pos = decode_into(&buf, pos, &mut key).unwrap();
            decoded.push(String::from_utf8(key.clone()).unwrap());
        }
        assert_eq!(decoded, keys);
    }

    #[test]
    fn round_trips_a_run_of_keys() {
        round_trip(&["chiyoda", "chuo", "osaka", "shibuya", "tokyo"]);
    }

    #[test]
    fn round_trips_edge_shapes() {
        round_trip(&[""]);
        round_trip(&["", "a"]);
        round_trip(&["a", "a"]);
        round_trip(&["prefix", "prefixed"]);
    }

    /// The shared prefix is what the encoding is for; a two term block of
    /// address components already pays for it.
    #[test]
    fn shares_the_common_prefix() {
        let mut buf = Vec::new();
        encode(b"chuo", b"chiyoda", &mut buf);
        assert_eq!(buf, b"\x02\x02uo");
    }

    #[test]
    fn rejects_a_shared_prefix_longer_than_the_previous_key() {
        let mut key = b"ab".to_vec();
        assert!(matches!(
            decode_into(b"\x05\x00", 0, &mut key),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_truncation() {
        let mut key = Vec::new();
        assert!(decode_into(b"\x00\x05ab", 0, &mut key).is_err());
        assert!(decode_into(b"\x00", 0, &mut key).is_err());
        assert!(decode_into(b"", 0, &mut key).is_err());
    }
}
