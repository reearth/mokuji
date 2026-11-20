//! The manifest itself (`MANIFEST.md`).

use serde::{Deserialize, Serialize};

use crate::manifest::{Error, Result};

/// The version of `MANIFEST.md` this crate implements.
pub const VERSION: &str = "0.1";

/// The segments of a collection, in order, and what has been deleted from each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// `MANIFEST.md` version, `major.minor`.
    pub mokuji_manifest: String,
    /// Monotonic, bumped by every change: which state of the collection this is.
    pub generation: u64,
    /// Oldest first. The collection is their concatenation, in this order.
    pub segments: Vec<Segment>,
}

/// One segment of a collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    /// Where the file is, relative to the manifest.
    pub path: String,
    /// `<algorithm>:<lowercase hex>` of the whole file, as `SPEC.md` §6 writes
    /// a source's.
    pub content_hash: String,
    /// The file's length in bytes.
    pub length: u64,
    /// Documents of this segment that are no longer in the collection, by
    /// their ID here, strictly increasing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deletes: Vec<u64>,
    /// Where the segment's documents are, if they are all in one place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
}

/// Where a segment's documents are: the publisher's statement that every one of
/// them lies within these countries.
///
/// A reader asked about a place skips a segment whose region does not include
/// it — which is what makes a regional shard cheaper to open than the planet.
/// A segment with no region may hold documents anywhere, and is never skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    /// ISO 3166-1 alpha-2 codes.
    pub countries: Vec<String>,
}

impl Manifest {
    /// Parses and checks a manifest.
    ///
    /// Unknown keys are ignored, as they are in a file's metadata: a later
    /// version may add one, and a reader of this one still reads the rest.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Manifest =
            serde_json::from_str(text).map_err(|e| Error::Manifest(e.to_string()))?;
        let major = manifest.mokuji_manifest.split('.').next().unwrap_or("");
        if major != VERSION.split('.').next().expect("a major version") {
            return Err(Error::Manifest(format!(
                "version {} is not one this reader implements",
                manifest.mokuji_manifest
            )));
        }
        if manifest.segments.is_empty() {
            return Err(Error::Manifest("a collection has no segments".into()));
        }
        for segment in &manifest.segments {
            let fail = |detail: &str| Error::Segment {
                path: segment.path.clone(),
                detail: detail.into(),
            };
            if segment.path.is_empty() {
                return Err(Error::Manifest("a segment has no path".into()));
            }
            crate::meta::ContentHash::parse(&segment.content_hash)
                .map_err(|_| fail("content_hash is not <algorithm>:<hex>"))?;
            if segment.deletes.windows(2).any(|w| w[1] <= w[0]) {
                return Err(fail("deletes are not strictly increasing"));
            }
            if let Some(region) = &segment.region {
                if region.countries.is_empty() {
                    return Err(fail("a region names no country"));
                }
                let mut seen = std::collections::HashSet::new();
                for code in &region.countries {
                    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
                        return Err(fail("a region's countries are ISO 3166-1 alpha-2 codes"));
                    }
                    if !seen.insert(code) {
                        return Err(fail("a region names a country twice"));
                    }
                }
            }
        }
        Ok(manifest)
    }

    /// The collection within these countries: every segment whose region
    /// includes one of them, and every segment with no region, in the same
    /// order and with the same deletes.
    ///
    /// Restricting keeps collection order, so a restricted collection answers
    /// as the whole one would with the other regions' documents deleted.
    pub fn within(&self, countries: &[&str]) -> Manifest {
        Manifest {
            segments: self
                .segments
                .iter()
                .filter(|s| {
                    s.region
                        .as_ref()
                        .is_none_or(|r| r.countries.iter().any(|c| countries.contains(&c.as_str())))
                })
                .cloned()
                .collect(),
            ..self.clone()
        }
    }

    /// The manifest as JSON, one key order and one layout, so that the same
    /// collection is always written down the same way.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a manifest serializes");
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            mokuji_manifest: VERSION.into(),
            generation: 3,
            segments: vec![
                Segment {
                    path: "base.mokuji".into(),
                    content_hash: format!("sha2-256:{}", "ab".repeat(32)),
                    length: 1000,
                    deletes: vec![2, 7],
                    region: None,
                },
                Segment {
                    path: "delta.mokuji".into(),
                    content_hash: format!("sha2-256:{}", "cd".repeat(32)),
                    length: 100,
                    deletes: vec![],
                    region: Some(Region {
                        countries: vec!["JP".into(), "KR".into()],
                    }),
                },
            ],
        }
    }

    #[test]
    fn round_trips() {
        let manifest = sample();
        assert_eq!(Manifest::parse(&manifest.to_json()).unwrap(), manifest);
    }

    #[test]
    fn leaves_out_an_empty_delete_list() {
        let json = sample().to_json();
        assert_eq!(json.matches("deletes").count(), 1);
    }

    #[test]
    fn ignores_keys_it_does_not_know() {
        let json = sample()
            .to_json()
            .replacen('{', "{\n  \"published\": \"2026-09-30\",", 1);
        assert_eq!(Manifest::parse(&json).unwrap(), sample());
    }

    #[test]
    fn restricting_keeps_what_has_no_region_and_what_includes_the_place() {
        let manifest = sample();
        assert_eq!(manifest.within(&["JP"]).segments.len(), 2);
        let elsewhere = manifest.within(&["FR"]);
        assert_eq!(elsewhere.segments.len(), 1);
        assert_eq!(elsewhere.segments[0].path, "base.mokuji");
        assert_eq!(elsewhere.segments[0].deletes, [2, 7]);
    }

    #[test]
    fn rejects_what_it_cannot_trust() {
        let mut bad = sample();
        bad.segments[0].deletes = vec![7, 2];
        assert!(Manifest::parse(&bad.to_json()).is_err(), "unsorted deletes");

        let mut bad = sample();
        bad.segments[0].deletes = vec![2, 2];
        assert!(Manifest::parse(&bad.to_json()).is_err(), "a delete twice");

        let mut bad = sample();
        bad.segments[1].content_hash = "abcd".into();
        assert!(Manifest::parse(&bad.to_json()).is_err(), "a bare hash");

        let mut bad = sample();
        bad.mokuji_manifest = "1.0".into();
        assert!(
            Manifest::parse(&bad.to_json()).is_err(),
            "a major version ahead"
        );

        let mut bad = sample();
        bad.segments.clear();
        assert!(Manifest::parse(&bad.to_json()).is_err(), "no segments");

        for countries in [vec![], vec!["jp"], vec!["JPN"], vec!["JP", "JP"]] {
            let mut bad = sample();
            bad.segments[1].region = Some(Region {
                countries: countries.iter().map(|c| c.to_string()).collect(),
            });
            assert!(Manifest::parse(&bad.to_json()).is_err(), "{countries:?}");
        }
    }
}
