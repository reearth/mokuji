//! Canonical CBOR, restricted to the subset `SPEC.md` §6 uses.
//!
//! Deterministic encoding as defined by RFC 8949 §4.2: definite lengths,
//! shortest-form integers, and map keys sorted by their encoded bytes. Decoding
//! enforces all three, so a file that is merely valid CBOR but not canonical is
//! rejected rather than silently accepted and re-encoded differently.

use mokuji_format::{Error, Result};

/// A CBOR value, limited to what metadata needs.
///
/// Deliberately narrow. Floats have no canonical form worth relying on, and
/// negative integers, byte strings, and tags have no use here; admitting them
/// would widen what a conformant writer has to reproduce exactly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    /// An unsigned integer.
    Uint(u64),
    /// A UTF-8 text string.
    Text(String),
    /// An array.
    Array(Vec<Value>),
    /// A map, held in encoded-key order.
    Map(Vec<(Value, Value)>),
}

impl Value {
    /// Borrows the text, or `None` if this is not a text value.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The unsigned integer, or `None` if this is not one.
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            Value::Uint(v) => Some(*v),
            _ => None,
        }
    }

    /// Looks a key up in a map value.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries
                .iter()
                .find(|(k, _)| k.as_text() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Builds a map value, sorting its entries into canonical order.
pub fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut entries: Vec<(Value, Value)> = entries
        .into_iter()
        .map(|(k, v)| (Value::Text(k.to_string()), v))
        .collect();
    entries.sort_by_cached_key(|(k, _)| encode(k));
    Value::Map(entries)
}

/// Encodes a value.
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Uint(v) => head(0, *v, out),
        Value::Text(s) => {
            head(3, s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            head(4, items.len() as u64, out);
            items.iter().for_each(|item| write(item, out));
        }
        Value::Map(entries) => {
            head(5, entries.len() as u64, out);
            for (k, v) in entries {
                write(k, out);
                write(v, out);
            }
        }
    }
}

/// Writes a major type and argument in the shortest form that fits.
fn head(major: u8, value: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    match value {
        0..=23 => out.push(major | value as u8),
        24..=0xff => {
            out.push(major | 24);
            out.push(value as u8);
        }
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(value as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
}

/// Decodes one value, which must consume the whole buffer.
pub fn decode(buf: &[u8]) -> Result<Value> {
    let (value, pos) = read(buf, 0)?;
    if pos != buf.len() {
        return Err(Error::Malformed {
            detail: "trailing bytes after the metadata value",
        });
    }
    Ok(value)
}

fn read(buf: &[u8], pos: usize) -> Result<(Value, usize)> {
    let initial = *buf.get(pos).ok_or(Error::Truncated {
        expected: pos + 1,
        found: buf.len(),
    })?;
    let major = initial >> 5;
    let (argument, pos) = read_argument(buf, pos)?;

    match major {
        0 => Ok((Value::Uint(argument), pos)),
        3 => {
            let len = length(argument)?;
            let raw = buf.get(pos..pos + len).ok_or(Error::Truncated {
                expected: pos + len,
                found: buf.len(),
            })?;
            let text = std::str::from_utf8(raw).map_err(|_| Error::Malformed {
                detail: "metadata text is not valid UTF-8",
            })?;
            Ok((Value::Text(text.to_string()), pos + len))
        }
        4 => {
            let len = length(argument)?;
            let mut items = Vec::with_capacity(len.min(64));
            let mut pos = pos;
            for _ in 0..len {
                let (item, next) = read(buf, pos)?;
                items.push(item);
                pos = next;
            }
            Ok((Value::Array(items), pos))
        }
        5 => {
            let len = length(argument)?;
            let mut entries: Vec<(Value, Value)> = Vec::with_capacity(len.min(64));
            let mut pos = pos;
            let mut previous: Option<Vec<u8>> = None;
            for _ in 0..len {
                let (key, next) = read(buf, pos)?;
                let (value, next) = read(buf, next)?;
                // Canonical order is checked, not restored: a reader that
                // silently sorted would accept two encodings of one map and
                // make a byte-identical rebuild unverifiable.
                let encoded = encode(&key);
                if let Some(previous) = &previous {
                    if &encoded <= previous {
                        return Err(Error::Malformed {
                            detail: "metadata map keys are not in canonical order",
                        });
                    }
                }
                previous = Some(encoded);
                entries.push((key, value));
                pos = next;
            }
            Ok((Value::Map(entries), pos))
        }
        _ => Err(Error::Malformed {
            detail: "metadata uses a CBOR major type outside the permitted subset",
        }),
    }
}

/// Reads the argument of a head, rejecting anything but the shortest form.
fn read_argument(buf: &[u8], pos: usize) -> Result<(u64, usize)> {
    let initial = buf[pos] & 0x1f;
    let take = |n: usize| -> Result<u64> {
        let raw = buf.get(pos + 1..pos + 1 + n).ok_or(Error::Truncated {
            expected: pos + 1 + n,
            found: buf.len(),
        })?;
        Ok(raw.iter().fold(0u64, |acc, &b| acc << 8 | u64::from(b)))
    };

    let (value, width) = match initial {
        0..=23 => (u64::from(initial), 0),
        24 => (take(1)?, 1),
        25 => (take(2)?, 2),
        26 => (take(4)?, 4),
        27 => (take(8)?, 8),
        31 => {
            return Err(Error::Malformed {
                detail: "metadata uses an indefinite length",
            })
        }
        _ => {
            return Err(Error::Malformed {
                detail: "metadata uses a reserved CBOR argument",
            })
        }
    };

    // Shortest form: a value that would fit in a narrower head must use it.
    let shortest = match value {
        0..=23 => 0,
        24..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    };
    if width != shortest {
        return Err(Error::Malformed {
            detail: "metadata integer is not in shortest form",
        });
    }
    Ok((value, pos + 1 + width))
}

fn length(argument: u64) -> Result<usize> {
    usize::try_from(argument).map_err(|_| Error::Malformed {
        detail: "metadata length overflows",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: Value) {
        let encoded = encode(&value);
        assert_eq!(decode(&encoded).unwrap(), value, "bytes {encoded:02x?}");
    }

    #[test]
    fn round_trips_the_subset() {
        round_trip(Value::Uint(0));
        round_trip(Value::Uint(23));
        round_trip(Value::Uint(24));
        round_trip(Value::Uint(u64::MAX));
        round_trip(Value::Text(String::new()));
        round_trip(Value::Text("tokyo".into()));
        round_trip(Value::Array(vec![]));
        round_trip(Value::Array(vec![Value::Uint(1), Value::Text("a".into())]));
        round_trip(map([]));
        round_trip(map([("b", Value::Uint(2)), ("a", Value::Uint(1))]));
    }

    #[test]
    fn round_trips_a_long_string() {
        round_trip(Value::Text("x".repeat(300)));
    }

    #[test]
    fn map_builds_in_canonical_order() {
        // Shorter keys sort first, because ordering is by encoded bytes and the
        // length is part of the head.
        let value = map([("uri", Value::Uint(1)), ("content_hash", Value::Uint(2))]);
        let Value::Map(entries) = &value else {
            panic!("expected a map")
        };
        assert_eq!(entries[0].0.as_text(), Some("uri"));
        assert_eq!(entries[1].0.as_text(), Some("content_hash"));
    }

    #[test]
    fn rejects_keys_out_of_canonical_order() {
        // {"b": 1, "a": 2}, which is valid CBOR but not canonical.
        let buf = [0xa2, 0x61, b'b', 0x01, 0x61, b'a', 0x02];
        assert!(matches!(decode(&buf), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_duplicate_keys() {
        let buf = [0xa2, 0x61, b'a', 0x01, 0x61, b'a', 0x02];
        assert!(matches!(decode(&buf), Err(Error::Malformed { .. })));
    }

    #[test]
    fn rejects_a_non_shortest_integer() {
        // 1 encoded in a one-byte argument instead of inline.
        assert!(matches!(
            decode(&[0x18, 0x01]),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_an_indefinite_length() {
        assert!(matches!(
            decode(&[0x9f, 0xff]),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_types_outside_the_subset() {
        // A negative integer, which the subset does not admit.
        assert!(matches!(decode(&[0x20]), Err(Error::Malformed { .. })));
        // A float.
        assert!(matches!(
            decode(&[0xf9, 0x00, 0x00]),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_trailing_bytes() {
        assert!(matches!(
            decode(&[0x01, 0x01]),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_truncation() {
        assert!(matches!(decode(&[0x61]), Err(Error::Truncated { .. })));
        assert!(matches!(decode(&[]), Err(Error::Truncated { .. })));
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(matches!(
            decode(&[0x61, 0xff]),
            Err(Error::Malformed { .. })
        ));
    }
}
