//! Questions that take more than one lookup, answered from the primitives of
//! [`MokujiIndex`]: a prefix that falls back to the longest term a query
//! begins with, and the polygons holding a point, from a file of geohash
//! coverings. The same as `js/src/query.ts`.

use std::collections::{BTreeMap, BTreeSet};

use crate::meta::Value;
use crate::read::{Payload, RangeReader, TermEntry};
use crate::{Error, MokujiIndex, Result};

/// How a hit of [`lookup_prefix`] relates to what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixMatch {
    /// The term asked for.
    Exact,
    /// A term that begins with what was asked.
    Prefix,
    /// The longest term a reading begins with: what was asked lies within it.
    Within,
}

/// One document [`lookup_prefix`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixHit {
    /// The term, as the file stores it.
    pub term: String,
    /// The document's ID.
    pub doc: u64,
    /// What the file returns for it.
    pub payload: Payload,
    /// How the term relates to what was asked.
    pub r#match: PrefixMatch,
}

/// The terms that begin with `normalized`, as many as `limit` documents, or,
/// when none does, the longest term that one of `readings` begins with.
///
/// The fallback is for keys that a query runs past: an address typed with its
/// block and lot after the town, when the file keys towns, finds the town.
/// Every prefix of every reading, from two characters to one fewer than the
/// reading, is looked up at once; prefixes of one string fall in one or two
/// dictionary blocks, read together. `normalized` and `readings` are already
/// normalized, as the file's tokenizer does ([`MokujiIndex::normalize`]).
pub async fn lookup_prefix<R: RangeReader>(
    index: &MokujiIndex<R>,
    normalized: &str,
    limit: usize,
    readings: &[&str],
) -> Result<Vec<PrefixHit>> {
    // Asked for nothing, it reads nothing, the fallback included.
    if limit == 0 {
        return Ok(Vec::new());
    }
    let reader = index.index();
    let mut entries: Vec<(Vec<u8>, TermEntry)> =
        reader.prefix(normalized.as_bytes(), limit).await?;
    let mut within = false;
    if entries.is_empty() && !readings.is_empty() {
        let mut prefixes: Vec<String> = Vec::new();
        for reading in readings {
            let chars: Vec<char> = reading.chars().collect();
            for n in 2..chars.len() {
                let p: String = chars[..n].iter().collect();
                if !prefixes.contains(&p) {
                    prefixes.push(p);
                }
            }
        }
        let keys: Vec<&[u8]> = prefixes.iter().map(|p| p.as_bytes()).collect();
        let found = reader.lookup_many(&keys).await?;
        let mut longest: Option<usize> = None;
        for (i, entry) in found.iter().enumerate() {
            if entry.is_some()
                && longest.is_none_or(|l| prefixes[i].chars().count() > prefixes[l].chars().count())
            {
                longest = Some(i);
            }
        }
        if let Some(l) = longest {
            let entry = found[l].clone().expect("found above");
            entries = vec![(prefixes[l].clone().into_bytes(), entry)];
            within = true;
        }
    }
    let terms: Vec<TermEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
    let postings = reader.postings_span(&terms).await?;
    let mut hits: Vec<(String, u64, PrefixMatch)> = Vec::new();
    for ((key, _), run) in entries.iter().zip(&postings) {
        let term = String::from_utf8_lossy(key).into_owned();
        let r#match = if within {
            PrefixMatch::Within
        } else if term == normalized {
            PrefixMatch::Exact
        } else {
            PrefixMatch::Prefix
        };
        for &doc in &run.docs {
            hits.push((term.clone(), doc, r#match));
        }
    }
    hits.truncate(limit);
    let docs: Vec<u64> = hits.iter().map(|(_, doc, _)| *doc).collect();
    let payloads = reader.payload_span(&docs).await?;
    Ok(hits
        .into_iter()
        .zip(payloads)
        .map(|((term, doc, r#match), payload)| PrefixHit {
            term,
            doc,
            payload,
            r#match,
        })
        .collect())
}

// --- geohash coverings -------------------------------------------------------
//
// A file in the shape demo/reverse builds: each polygon is a document, keyed
// by the geohash cells that cover it, coarse in its interior and fine along
// its boundary, with its label and rings as payload (`label|rings`). The
// format has no opinion about geohashes; the file says how it was built in
// its application-defined `attrs` (SPEC.md §6), and this reads them.

const BASE32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// The geohash cell of `precision` characters holding (lat, lon).
pub fn geohash(lat: f64, lon: f64, precision: usize) -> String {
    let (mut lon_lo, mut lon_hi, mut lat_lo, mut lat_hi) = (-180.0, 180.0, -90.0, 90.0);
    let mut out = String::with_capacity(precision);
    let mut code = 0usize;
    for i in 0..precision * 5 {
        if i % 2 == 0 {
            let mid = (lon_lo + lon_hi) / 2.0;
            code = code * 2 + usize::from(lon >= mid);
            if lon >= mid {
                lon_lo = mid;
            } else {
                lon_hi = mid;
            }
        } else {
            let mid = (lat_lo + lat_hi) / 2.0;
            code = code * 2 + usize::from(lat >= mid);
            if lat >= mid {
                lat_lo = mid;
            } else {
                lat_hi = mid;
            }
        }
        if i % 5 == 4 {
            out.push(BASE32[code] as char);
            code = 0;
        }
    }
    out
}

fn attr<'a, R: RangeReader>(index: &'a MokujiIndex<R>, name: &str) -> Option<&'a str> {
    index
        .meta()
        .attrs
        .as_ref()?
        .get(name)
        .and_then(Value::as_text)
}

/// The geohash precisions the file is keyed at (`geohash-precisions`, such as
/// `5,6,7`), or `None` when it does not say.
pub fn precisions_of<R: RangeReader>(index: &MokujiIndex<R>) -> Option<Vec<usize>> {
    attr(index, "geohash-precisions")?
        .split(',')
        .map(|p| {
            let p = p.trim();
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok().filter(|p| (1..=12).contains(p))
        })
        .collect()
}

/// Integers per degree the rings are stored in (`geometry`, such as
/// `delta-int-1e5`), or `None` when the file does not say.
pub fn scale_of<R: RangeReader>(index: &MokujiIndex<R>) -> Option<f64> {
    let exponent = attr(index, "geometry")?.strip_prefix("delta-int-1e")?;
    if exponent.is_empty() || exponent.len() > 2 || !exponent.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(10f64.powi(exponent.parse().ok()?))
}

/// The names of the fields a label is cut into at U+001F (`payload-fields`),
/// or `None` when a label is one name.
pub fn fields_of<R: RangeReader>(index: &MokujiIndex<R>) -> Option<Vec<String>> {
    let declared = attr(index, "payload-fields").filter(|d| !d.is_empty())?;
    Some(declared.split(',').map(|f| f.trim().to_string()).collect())
}

/// A polygon as `[lon, lat]` rings, each closed.
pub type Rings = Vec<Vec<[f64; 2]>>;

/// Rings as `geometry=delta-int-1e…` writes them: `;` between rings, each the
/// first vertex then the difference from the vertex before, in integers of
/// 1/`scale` degree, without the closing vertex, which is put back.
pub fn decode_rings(text: &str, scale: f64) -> Result<Rings> {
    text.split(';')
        .map(|part| {
            let values: Vec<&str> = part.split(',').collect();
            let mut ring = Vec::new();
            let (mut x, mut y) = (0i64, 0i64);
            for pair in values.chunks_exact(2) {
                let parse = |v: &str| v.trim().parse::<i64>().map_err(|_| Error::Rings);
                x = x.checked_add(parse(pair[0])?).ok_or(Error::Rings)?;
                y = y.checked_add(parse(pair[1])?).ok_or(Error::Rings)?;
                ring.push([x as f64 / scale, y as f64 / scale]);
            }
            if let Some(&first) = ring.first() {
                ring.push(first);
            }
            Ok(ring)
        })
        .collect()
}

/// Whether (x, y) is inside, even-odd over every ring, so a hole is outside.
pub fn contains(rings: &Rings, x: f64, y: f64) -> bool {
    let mut inside = false;
    for ring in rings {
        for edge in ring.windows(2) {
            let ([x1, y1], [x2, y2]) = (edge[0], edge[1]);
            if (y1 > y) != (y2 > y) && x < x1 + (y - y1) * (x2 - x1) / (y2 - y1) {
                inside = !inside;
            }
        }
    }
    inside
}

/// One document of a file of geohash coverings.
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    /// The document's ID.
    pub doc: u64,
    /// The payload before its rings.
    pub label: String,
    /// The label cut at U+001F and named by `payload-fields`, or `None` when
    /// the file names no fields.
    pub fields: Option<BTreeMap<String, String>>,
    /// Its rings.
    pub rings: Rings,
}

/// What [`locate`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    /// The point's cell, at the finest precision the file is keyed at.
    pub cell: String,
    /// How many polygons the cells named, before the point was tested.
    pub candidates: usize,
    /// The ones holding the point.
    pub inside: Vec<Polygon>,
}

/// How a file was built, when the file itself does not say. `None`, no
/// precisions, or a scale that is not a positive finite number defers to the
/// file; precisions outside `1..=12` are [`Error::NotCoverings`].
#[derive(Debug, Clone, Default)]
pub struct LocateOptions {
    /// The geohash precisions the file is keyed at.
    pub precisions: Option<Vec<usize>>,
    /// Integers per degree the rings are stored in.
    pub scale: Option<f64>,
}

/// The polygons holding (lat, lon), from a file of geohash coverings.
///
/// Every precision is looked up, not only the finest that matches, since a
/// point can be in one polygon's coarse interior cell and another's fine
/// boundary cell at once. The cells are prefixes of one string, so they fall
/// in neighbouring dictionary blocks, read together; the candidates' rings
/// come with their payload, and an even-odd test settles them. A file that
/// does not say its precisions and geometry is [`Error::NotCoverings`],
/// unless `options` say them.
pub async fn locate<R: RangeReader>(
    index: &MokujiIndex<R>,
    lat: f64,
    lon: f64,
    options: &LocateOptions,
) -> Result<Located> {
    // What the caller says counts only when it says something: no
    // precisions, or a scale that is not a positive number, defers to the
    // file. Precisions it does say have to be ones a geohash has.
    let precisions = match options.precisions.as_ref().filter(|p| !p.is_empty()) {
        Some(given) if given.iter().all(|p| (1..=12).contains(p)) => given.clone(),
        Some(_) => return Err(Error::NotCoverings),
        None => precisions_of(index).ok_or(Error::NotCoverings)?,
    };
    let scale = options
        .scale
        .filter(|s| s.is_finite() && *s > 0.0)
        .or_else(|| scale_of(index))
        .ok_or(Error::NotCoverings)?;
    let names = fields_of(index);
    let finest = *precisions.iter().max().expect("not empty");
    let cell = geohash(lat, lon, finest);
    let cells: Vec<&[u8]> = precisions.iter().map(|&p| &cell.as_bytes()[..p]).collect();
    let reader = index.index();
    let mut docs = BTreeSet::new();
    for entry in reader.lookup_many(&cells).await?.into_iter().flatten() {
        // One entry at a time: the cells' runs need not be near each other,
        // and one span from the first to the last would read what lies
        // between.
        docs.extend(reader.postings(&entry).await?.docs);
    }
    let docs: Vec<u64> = docs.into_iter().collect();
    let payloads = reader.payload_span(&docs).await?;
    let mut inside = Vec::new();
    for (&doc, payload) in docs.iter().zip(payloads) {
        let payload = payload.to_string();
        let (label, geometry) = payload.split_once('|').unwrap_or((&payload, ""));
        let rings = decode_rings(geometry, scale)?;
        if contains(&rings, lon, lat) {
            let fields = names.as_ref().map(|names| {
                let values: Vec<&str> = label.split('\u{1f}').collect();
                names
                    .iter()
                    .enumerate()
                    .map(|(j, name)| {
                        (
                            name.clone(),
                            values.get(j).copied().unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            });
            inside.push(Polygon {
                doc,
                label: label.to_string(),
                fields,
                rings,
            });
        }
    }
    Ok(Located {
        cell,
        candidates: docs.len(),
        inside,
    })
}
