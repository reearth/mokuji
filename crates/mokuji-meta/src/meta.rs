//! The metadata structure itself (`SPEC.md` §6).

use mokuji_format::{Error, Result};

use crate::cbor::{self, Value};

/// A content hash of a source of truth, as `<algorithm>:<lowercase hex>`.
///
/// The algorithm is named rather than assumed so that a file states what it was
/// hashed with, instead of relying on whatever the reader happens to implement.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Hash of the source content.
    pub content_hash: ContentHash,
    /// Where the source can be obtained, if it is public.
    pub uri: Option<String>,
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
        })
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
    fn round_trips_with_attributes() {
        let mut meta = sample();
        meta.attrs = Some(cbor::map([("region", Value::Text("jp".into()))]));
        assert_eq!(Meta::parse(&meta.encode()).unwrap(), meta);
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
