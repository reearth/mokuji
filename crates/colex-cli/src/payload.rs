//! Rendering payload for a person to read.
//!
//! The format states how a payload was written down and nothing about what it
//! means (`SPEC.md` §5.5), so this is presentation and belongs to the tool
//! rather than to the reader.

use colex_read::Payload;

use crate::info::Error;

/// Parses a payload column as a row of decimal numbers, scaled by `decimals`.
///
/// String arithmetic rather than a float: a coordinate written with ten decimal
/// places does not survive a round trip through `f64` unchanged, and an index
/// that is meant to be rebuilt byte-for-byte cannot afford that.
pub fn parse_values(text: &str, decimals: u8) -> Result<Vec<i64>, Error> {
    text.split(',')
        .map(|field| scale(field.trim(), decimals))
        .collect()
}

fn scale(field: &str, decimals: u8) -> Result<i64, Error> {
    let bad = || Error::Corpus(format!("'{field}' is not a decimal number"));
    let (negative, digits) = match field.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, field.strip_prefix('+').unwrap_or(field)),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty() {
        return Err(bad());
    }
    if !whole
        .bytes()
        .chain(fraction.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }

    let width = usize::from(decimals);
    let mut value: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse::<i128>().map_err(|_| bad())?
    };
    for i in 0..width {
        value = value * 10 + i128::from(fraction.as_bytes().get(i).map_or(0, |b| b - b'0'));
    }
    // Half up on the first digit dropped, so that scaling down is rounding
    // rather than truncation.
    if fraction.as_bytes().get(width).is_some_and(|&b| b >= b'5') {
        value += 1;
    }

    let value = if negative { -value } else { value };
    i64::try_from(value).map_err(|_| Error::Corpus(format!("'{field}' does not fit in 64 bits")))
}

/// Renders a payload as text.
///
/// Delegates, so that the tool, the tests, and the reader all agree on what a
/// stored value looks like written out.
pub fn render(payload: &Payload) -> String {
    payload.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(values: &[i64], exponent: i8) -> Payload {
        Payload::Ints {
            values: values.to_vec(),
            scale_exponent: exponent,
        }
    }

    #[test]
    fn renders_scaled_integers() {
        assert_eq!(
            render(&ints(&[139_767_201, 35_681_560], -6)),
            "139.767201,35.681560"
        );
        assert_eq!(render(&ints(&[-1_500_000], -6)), "-1.500000");
        assert_eq!(render(&ints(&[42], 0)), "42");
        assert_eq!(render(&ints(&[42], 2)), "4200");
    }

    #[test]
    fn renders_values_smaller_than_one_unit() {
        assert_eq!(render(&ints(&[7], -3)), "0.007");
        assert_eq!(render(&ints(&[-7], -3)), "-0.007");
    }

    #[test]
    fn parses_and_scales() {
        assert_eq!(
            parse_values("139.767201,35.68156", 6).unwrap(),
            [139_767_201, 35_681_560]
        );
        assert_eq!(parse_values("-1.5", 6).unwrap(), [-1_500_000]);
        assert_eq!(parse_values("42", 0).unwrap(), [42]);
        assert_eq!(parse_values(" 7 ", 3).unwrap(), [7000]);
    }

    #[test]
    fn rounds_rather_than_truncates() {
        // The registry publishes ten decimal places; asking for six should not
        // silently drop the seventh's worth of position.
        assert_eq!(parse_values("139.5547759", 6).unwrap(), [139_554_776]);
        assert_eq!(parse_values("139.5547754", 6).unwrap(), [139_554_775]);
    }

    #[test]
    fn round_trips_through_rendering() {
        let values = parse_values("139.767201,35.681560", 6).unwrap();
        assert_eq!(render(&ints(&values, -6)), "139.767201,35.681560");
    }

    #[test]
    fn rejects_what_is_not_a_number() {
        assert!(parse_values("north", 6).is_err());
        assert!(parse_values("1.2.3", 6).is_err());
        assert!(parse_values("", 6).is_err());
    }

    #[test]
    fn renders_opaque_text_and_bytes() {
        assert_eq!(
            render(&Payload::Opaque(b"35.68,139.76".to_vec())),
            "35.68,139.76"
        );
        assert_eq!(render(&Payload::Opaque(vec![0xff, 0xfe])), "<2 bytes>");
    }
}
