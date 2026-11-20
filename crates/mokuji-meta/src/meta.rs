//! The metadata structure itself (`SPEC.md` §6).

use mokuji_format::{Error, Result};

use crate::cbor::{self, Value};

/// A content hash of a source of truth, as `<algorithm>:<lowercase hex>`.
///
/// The algorithm is named rather than assumed so that a file states what it was
/// hashed with, instead of relying on whatever the reader happens to implement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContentHash {
    /// Hash algorithm; `sha2-256` in this version of the specification.
    pub algorithm: String,
    /// Lowercase hexadecimal digest.
    pub hex: String,
}

impl ContentHash {
    /// The algorithm this version of the specification defines.
    pub const SHA2_256: &'static str = "sha2-256";

    /// Builds a `sha2-256` hash from a digest.
    pub fn sha2_256(digest: &[u8]) -> Self {
        ContentHash {
            algorithm: Self::SHA2_256.to_string(),
            hex: digest.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }

    /// Parses the `<algorithm>:<hex>` form.
    pub fn parse(text: &str) -> Result<Self> {
        let (algorithm, hex) = text.split_once(':').ok_or(Error::Malformed {
            detail: "content hash is not in <algorithm>:<hex> form",
        })?;
        if algorithm.is_empty() || hex.is_empty() {
            return Err(Error::Malformed {
                detail: "content hash has an empty algorithm or digest",
            });
        }
        if !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Malformed {
                detail: "content hash digest is not lowercase hexadecimal",
            });
        }
        Ok(ContentHash {
            algorithm: algorithm.to_string(),
            hex: hex.to_string(),
        })
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

/// One source of truth the index was derived from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Source {
    /// Hash of the source content.
    pub content_hash: ContentHash,
    /// Where the source can be obtained, if it is public.
    pub uri: Option<String>,
    /// When the builder obtained it, as an RFC 3339 date or date-time.
    ///
    /// The hash says *which* bytes; this says when they were the ones on
    /// offer. A register republished under the same name is a different
    /// source, and the date is what tells a reader how stale the index might
    /// be without fetching anything.
    pub retrieved: Option<String>,
    /// When the publisher last changed it, as the publisher states it — an
    /// HTTP `Last-Modified`, a release date — in RFC 3339.
    pub modified: Option<String>,
}

/// What produced the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generator {
    /// Implementation name.
    pub name: String,
    /// Implementation version.
    pub version: String,
    /// The full build parameters.
    ///
    /// Required to be sufficient, with the source hashes, to reproduce the file
    /// byte-for-byte (`SPEC.md` §3.4). Anything a writer branches on belongs
    /// here.
    pub params: Value,
}

/// How the source text was turned into terms.
///
/// Recorded rather than specified. Which analyzer is right is a property of the
/// language and the corpus; what provenance actually needs is only that the
/// choice is stated and therefore repeatable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokenizer {
    /// Identifier from the tokenizer registry.
    pub id: String,
    /// Tokenizer parameters.
    pub params: Value,
}

/// The metadata section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// Monotonic generation number.
    ///
    /// A Mokuji file is immutable; a new index is a new generation, not an
    /// edit.
    pub generation: u64,
    /// The sources this index was derived from.
    pub sources: Vec<Source>,
    /// What built it.
    pub generator: Generator,
    /// How it was tokenized.
    pub tokenizer: Tokenizer,
    /// Application-defined additions, or `None` if absent.
    pub attrs: Option<Value>,
    /// The license the index itself is published under, as an SPDX license
    /// expression such as `ODbL-1.0`, or `None` if the file does not say.
    ///
    /// The index's, not its sources'. An index derived from a database under a
    /// share-alike license is usually itself a derivative database, and the
    /// file is the thing being distributed.
    pub license: Option<String>,
    /// What a consumer shows alongside results from this file, or `None`.
    pub attribution: Option<String>,
}

impl Meta {
    /// Encodes the section as canonical CBOR.
    pub fn encode(&self) -> Vec<u8> {
        let sources = Value::Array(
            self.sources
                .iter()
                .map(|source| {
                    let mut entries =
                        vec![("content_hash", Value::Text(source.content_hash.to_string()))];
                    if let Some(uri) = &source.uri {
                        entries.push(("uri", Value::Text(uri.clone())));
                    }
                    if let Some(retrieved) = &source.retrieved {
                        entries.push(("retrieved", Value::Text(retrieved.clone())));
                    }
                    if let Some(modified) = &source.modified {
                        entries.push(("modified", Value::Text(modified.clone())));
                    }
                    cbor::map(entries)
                })
                .collect(),
        );

        let mut entries = vec![
            ("generation", Value::Uint(self.generation)),
            ("source", sources),
            (
                "generator",
                cbor::map([
                    ("name", Value::Text(self.generator.name.clone())),
                    ("version", Value::Text(self.generator.version.clone())),
                    ("params", self.generator.params.clone()),
                ]),
            ),
            (
                "tokenizer",
                cbor::map([
                    ("id", Value::Text(self.tokenizer.id.clone())),
                    ("params", self.tokenizer.params.clone()),
                ]),
            ),
        ];
        if let Some(attrs) = &self.attrs {
            entries.push(("attrs", attrs.clone()));
        }
        if let Some(license) = &self.license {
            entries.push(("license", Value::Text(license.clone())));
        }
        if let Some(attribution) = &self.attribution {
            entries.push(("attribution", Value::Text(attribution.clone())));
        }
        cbor::encode(&cbor::map(entries))
    }

    /// Parses the section.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let value = cbor::decode(bytes)?;
        let generation =
            value
                .get("generation")
                .and_then(Value::as_uint)
                .ok_or(Error::Malformed {
                    detail: "metadata has no generation",
                })?;

        let Some(Value::Array(sources)) = value.get("source") else {
            return Err(Error::Malformed {
                detail: "metadata has no source array",
            });
        };
        let sources = sources
            .iter()
            .map(|source| {
                let hash = source.get("content_hash").and_then(Value::as_text).ok_or(
                    Error::Malformed {
                        detail: "a metadata source has no content hash",
                    },
                )?;
                Ok(Source {
                    content_hash: ContentHash::parse(hash)?,
                    uri: source
                        .get("uri")
                        .and_then(Value::as_text)
                        .map(str::to_string),
                    // Text when present, but not parsed as a date: a reader
                    // that cannot read a timestamp can still use the index.
                    retrieved: optional_text(
                        source,
                        "retrieved",
                        "a metadata source's retrieval time is not text",
                    )?,
                    modified: optional_text(
                        source,
                        "modified",
                        "a metadata source's modification time is not text",
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let generator = value.get("generator").ok_or(Error::Malformed {
            detail: "metadata has no generator",
        })?;
        let tokenizer = value.get("tokenizer").ok_or(Error::Malformed {
            detail: "metadata has no tokenizer",
        })?;
        let text = |v: &Value, key: &'static str, detail: &'static str| -> Result<String> {
            v.get(key)
                .and_then(Value::as_text)
                .map(str::to_string)
                .ok_or(Error::Malformed { detail })
        };

        Ok(Meta {
            generation,
            sources,
            generator: Generator {
                name: text(generator, "name", "metadata generator has no name")?,
                version: text(generator, "version", "metadata generator has no version")?,
                params: generator.get("params").cloned().ok_or(Error::Malformed {
                    detail: "metadata generator has no params",
                })?,
            },
            tokenizer: Tokenizer {
                id: text(tokenizer, "id", "metadata tokenizer has no id")?,
                params: tokenizer.get("params").cloned().ok_or(Error::Malformed {
                    detail: "metadata tokenizer has no params",
                })?,
            },
            attrs: value.get("attrs").cloned(),
            license: optional_text(&value, "license", "metadata license is not text")?,
            attribution: optional_text(&value, "attribution", "metadata attribution is not text")?,
        })
    }
}

/// Whether `text` is an RFC 3339 `full-date` (`2026-11-15`) or `date-time`
/// (`2026-11-15T15:41:42Z`, `2026-11-15T15:41:42.5+09:00`).
///
/// What a writer checks before recording `retrieved` or `modified`. A reader
/// does not: a malformed date is a poor record, not an unreadable index.
pub fn is_rfc3339(text: &str) -> bool {
    fn digits(s: &str, n: usize) -> Option<u32> {
        (s.len() == n && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
    }
    fn date(s: &str) -> bool {
        let mut parts = s.split('-');
        let (Some(y), Some(m), Some(d), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let (Some(y), Some(m), Some(d)) = (digits(y, 4), digits(m, 2), digits(d, 2)) else {
            return false;
        };
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days = match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return false,
        };
        (1..=days).contains(&d)
    }
    fn hms(s: &str) -> bool {
        let mut parts = s.split(':');
        let (Some(h), Some(m), Some(sec), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let (whole, fraction) = sec.split_once('.').unwrap_or((sec, "0"));
        // 60 for a leap second, which RFC 3339 allows.
        matches!((digits(h, 2), digits(m, 2), digits(whole, 2)), (Some(h), Some(m), Some(s)) if h < 24 && m < 60 && s <= 60)
            && !fraction.is_empty()
            && fraction.bytes().all(|b| b.is_ascii_digit())
    }
    fn offset(s: &str) -> bool {
        if s == "Z" || s == "z" {
            return true;
        }
        let Some(rest) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) else {
            return false;
        };
        matches!(rest.split_once(':'), Some((h, m)) if matches!((digits(h, 2), digits(m, 2)), (Some(h), Some(m)) if h < 24 && m < 60))
    }

    let Some((day, time)) = text.split_once(['T', 't']) else {
        return date(text);
    };
    let split = time.find(['Z', 'z', '+', '-']);
    let Some(split) = split else {
        return false;
    };
    date(day) && hms(&time[..split]) && offset(&time[split..])
}

/// A key that may be absent, but is text when present. A license that is a
/// number is a malformed file, not an unlicensed one.
fn optional_text(value: &Value, key: &str, detail: &'static str) -> Result<Option<String>> {
    match value.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_text()
            .map(|t| Some(t.to_string()))
            .ok_or(Error::Malformed { detail }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Meta {
        Meta {
            generation: 1,
            sources: vec![Source {
                content_hash: ContentHash::sha2_256(&[0xab; 32]),
                uri: Some("s3://bucket/addresses.tsv".into()),
                ..Source::default()
            }],
            generator: Generator {
                name: "mokuji".into(),
                version: "0.0.0".into(),
                params: cbor::map([("terms_per_block", Value::Uint(128))]),
            },
            tokenizer: Tokenizer {
                id: "space-split".into(),
                params: cbor::map::<&str>([]),
            },
            attrs: None,
            license: None,
            attribution: None,
        }
    }

    #[test]
    fn round_trips() {
        let meta = sample();
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn round_trips_without_a_uri() {
        let mut meta = sample();
        meta.sources[0].uri = None;
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn round_trips_when_a_source_was_retrieved_and_modified() {
        let mut meta = sample();
        meta.sources[0].retrieved = Some("2026-11-20T03:12:00Z".into());
        meta.sources[0].modified = Some("2026-11-15".into());
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn a_source_date_that_is_not_text_is_malformed() {
        let source = cbor::map([
            (
                "content_hash",
                Value::Text(ContentHash::sha2_256(&[0; 32]).to_string()),
            ),
            ("modified", Value::Uint(20261115)),
        ]);
        let mut keyed: Vec<(String, Value)> = match cbor::decode(&sample().encode()).unwrap() {
            Value::Map(entries) => entries
                .into_iter()
                .map(|(k, v)| (k.as_text().unwrap().to_string(), v))
                .collect(),
            _ => unreachable!(),
        };
        for (key, value) in &mut keyed {
            if key == "source" {
                *value = Value::Array(vec![source.clone()]);
            }
        }
        assert!(Meta::parse(&cbor::encode(&cbor::map(keyed))).is_err());
    }

    #[test]
    fn a_source_date_that_is_not_rfc3339_still_parses() {
        let mut meta = sample();
        meta.sources[0].modified = Some("15 Nov 2026".into());
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn recognizes_rfc3339() {
        for good in [
            "2026-11-15",
            "2024-02-29",
            "2026-11-15T15:41:42Z",
            "2026-11-15t15:41:42z",
            "2026-11-15T15:41:42.123+09:00",
            "2026-12-31T23:59:60-05:30",
        ] {
            assert!(is_rfc3339(good), "{good}");
        }
        for bad in [
            "",
            "2026-11",
            "2026-1-15",
            "2026-13-01",
            "2025-02-29",
            "2026-11-31",
            "2026-11-15T15:41:42",
            "2026-11-15T25:00:00Z",
            "2026-11-15T15:41:42.Z",
            "2026-11-15 15:41:42Z",
            "2026-11-15T15:41:42+0900",
            "Sun, 15 Nov 2026 15:41:42 GMT",
        ] {
            assert!(!is_rfc3339(bad), "{bad}");
        }
    }

    #[test]
    fn round_trips_with_attributes() {
        let mut meta = sample();
        meta.attrs = Some(cbor::map([("region", Value::Text("jp".into()))]));
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn round_trips_a_license_and_its_attribution() {
        let mut meta = sample();
        meta.license = Some("ODbL-1.0".into());
        meta.attribution = Some("© OpenStreetMap contributors".into());
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn a_license_that_is_not_text_is_malformed() {
        let entries = match cbor::decode(&sample().encode()).unwrap() {
            Value::Map(entries) => entries,
            _ => unreachable!(),
        };
        // Rebuilt through `map`, so that the keys stay in canonical order and
        // the only thing wrong is the license.
        let mut keyed: Vec<(String, Value)> = entries
            .into_iter()
            .map(|(k, v)| (k.as_text().unwrap().to_string(), v))
            .collect();
        let valid = cbor::encode(&cbor::map(keyed.clone()));
        assert!(Meta::parse(&valid).is_ok());
        keyed.push(("license".into(), Value::Uint(1)));
        assert!(Meta::parse(&cbor::encode(&cbor::map(keyed))).is_err());
    }

    #[test]
    fn encoding_is_stable() {
        assert_eq!(sample().encode(), sample().encode());
    }

    #[test]
    fn parses_the_hash_form() {
        let hash = ContentHash::parse("sha2-256:0a1b").unwrap();
        assert_eq!(hash.algorithm, "sha2-256");
        assert_eq!(hash.to_string(), "sha2-256:0a1b");
    }

    #[test]
    fn rejects_malformed_hashes() {
        assert!(ContentHash::parse("0a1b").is_err());
        assert!(ContentHash::parse("sha2-256:").is_err());
        assert!(ContentHash::parse(":0a1b").is_err());
        // Uppercase would hash-compare unequal to the same digest in lowercase.
        assert!(ContentHash::parse("sha2-256:0A1B").is_err());
    }

    #[test]
    fn rejects_metadata_missing_required_keys() {
        let without_source = cbor::encode(&cbor::map([("generation", Value::Uint(1))]));
        assert!(Meta::parse(&without_source).is_err());
    }
}
