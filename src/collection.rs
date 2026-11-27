//! A collection asked in what a person types: [`manifest::Collection`] with
//! the segments' tokenizer in front of it, and what a page showing results
//! needs beside them — their payload, their credit, and what they cost.
//!
//! The same shape as `MokujiCollection` in the TypeScript, Python and Go
//! readers.

use std::path::Path;

use crate::manifest::{self, Collection, Hit, Manifest};
use crate::meta::Meta;
use crate::read::{CountingReader, FileReader, OpenOptions, Payload, RangeReader, ReadStats};
use crate::{Error, Result, Tokenizer};

/// What a collection's reads have cost since opening or the last reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CollectionStats {
    /// Reads, summed over the segments.
    pub reads: u32,
    /// Bytes, summed over the segments.
    pub bytes: u64,
    /// The most reads any one segment made: the round trips a reader that
    /// asks the segments at once waits for.
    pub round_trips: u32,
}

/// What [`MokujiCollection::complete`] and [`MokujiCollection::candidates`]
/// return. Its hits are [`manifest::Hit`]s, a segment and a document, not the
/// crate root's [`crate::Hit`], which is a term's document.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionCompletion {
    /// What was typed, after the collection's tokenizer.
    pub normalized: String,
    /// The documents, in collection order.
    pub hits: Vec<Hit>,
    /// Segments consulted, and for each whether its completions section
    /// answered.
    pub consulted: Vec<(usize, bool)>,
    /// The hits' payloads, in the same order.
    pub payloads: Vec<Payload>,
    /// The reads since the last reset when the hits were known, before any
    /// payload.
    pub requests_before_payload: u32,
}

/// What [`MokujiCollection::search`] returns. Its hits are
/// [`manifest::Hit`]s, a segment and a document.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionSearch {
    /// The terms the query became.
    pub terms: Vec<String>,
    /// How many live documents hold every term, of which `hits` are the
    /// first `limit`.
    pub matched: usize,
    /// In collection order: segments in manifest order, documents in each
    /// segment's order.
    pub hits: Vec<Hit>,
    /// The hits' payloads, in the same order.
    pub payloads: Vec<Payload>,
}

/// A collection, open: every segment's resident set held, deletes in hand.
pub struct MokujiCollection<R> {
    inner: Collection<CountingReader<R>>,
    metas: Vec<Meta>,
    tokenizer: Option<Tokenizer>,
    manifest_segments: usize,
}

impl MokujiCollection<FileReader> {
    /// Opens the manifest at `path` and every segment it names, each a path
    /// relative to the manifest's directory. With `countries`, only the
    /// segments whose region includes one of them, and those with none
    /// (`MANIFEST.md` §7); the others are never opened.
    pub async fn open_file(path: impl AsRef<Path>, countries: Option<&[&str]>) -> Result<Self> {
        let path = path.as_ref();
        let listed = Manifest::parse(&std::fs::read_to_string(path)?)?;
        let manifest = match countries {
            Some(codes) => listed.within(codes),
            None => listed.clone(),
        };
        let mut readers = Vec::with_capacity(manifest.segments.len());
        for segment in &manifest.segments {
            readers.push(FileReader::open(Manifest::segment_path(path, segment))?);
        }
        let mut collection = Self::open(&manifest, readers).await?;
        collection.manifest_segments = listed.segments.len();
        Ok(collection)
    }
}

impl<R: RangeReader> MokujiCollection<R> {
    /// Opens every segment the manifest lists, one reader each, in order,
    /// checking each against what the manifest says of it.
    pub async fn open(manifest: &Manifest, readers: Vec<R>) -> Result<Self> {
        Self::open_with(manifest, readers, OpenOptions::default()).await
    }

    /// [`MokujiCollection::open`], opening each segment with `options`.
    pub async fn open_with(
        manifest: &Manifest,
        readers: Vec<R>,
        options: OpenOptions,
    ) -> Result<Self> {
        let readers = readers.into_iter().map(CountingReader::new).collect();
        let inner = Collection::open_with(manifest, readers, options).await?;
        let mut metas = Vec::with_capacity(inner.len());
        for i in 0..inner.len() {
            metas.push(Meta::parse(inner.segment(i).meta())?);
        }
        // The segments were checked to share one tokenizer when they opened.
        let tokenizer = metas
            .first()
            .and_then(|m| Tokenizer::parse(&m.tokenizer.id));
        Ok(MokujiCollection {
            inner,
            metas,
            tokenizer,
            manifest_segments: manifest.segments.len(),
        })
    }

    /// The byte-level collection underneath.
    pub fn inner(&self) -> &Collection<CountingReader<R>> {
        &self.inner
    }

    /// How many segments the manifest lists, before a region left some out.
    pub fn manifest_segments(&self) -> usize {
        self.manifest_segments
    }

    /// What a query goes through before it is looked up: the segments'
    /// tokenizer, without splitting.
    pub fn normalize(&self, text: &str) -> String {
        match self.tokenizer {
            Some(t) => t.normalize_query(text),
            None => text.to_string(),
        }
    }

    /// Each segment's credit, once each, in segment order, for a page that
    /// shows results.
    pub fn attributions(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for meta in &self.metas {
            if let Some(a) = meta.attribution.as_deref().filter(|a| !a.is_empty()) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        out
    }

    /// Reads and bytes summed, and the most reads any one segment made.
    pub fn stats(&self) -> CollectionStats {
        let per: Vec<ReadStats> = (0..self.inner.len())
            .map(|i| self.inner.segment(i).reader().stats())
            .collect();
        CollectionStats {
            reads: per.iter().map(|s| s.reads).sum(),
            bytes: per.iter().map(|s| s.bytes).sum(),
            round_trips: per.iter().map(|s| s.reads).max().unwrap_or(0),
        }
    }

    /// Starts every segment's count from zero.
    pub fn reset_stats(&self) {
        for i in 0..self.inner.len() {
            self.inner.segment(i).reader().reset();
        }
    }

    /// One segment's first `need` live documents for a prefix already
    /// normalized, and whether its completions section answered
    /// (`MANIFEST.md` §3.2).
    pub async fn live_in(
        &self,
        segment: usize,
        normalized: &str,
        need: usize,
    ) -> Result<(Vec<u64>, bool)> {
        Ok(self
            .inner
            .live_in(segment, normalized.as_bytes(), need)
            .await?)
    }

    /// The first `limit` live documents for what was typed, in collection
    /// order, with their payload.
    pub async fn complete(&self, text: &str, limit: usize) -> Result<CollectionCompletion> {
        let normalized = self.normalize(text);
        let found = self.inner.complete(normalized.as_bytes(), limit).await?;
        self.finish(normalized, found.hits, found.consulted).await
    }

    /// Every segment's own first `limit`, for a caller that ranks across
    /// segments by a score of its own (`MANIFEST.md` §3.3), one list after
    /// another in segment order.
    pub async fn candidates(&self, text: &str, limit: usize) -> Result<CollectionCompletion> {
        let normalized = self.normalize(text);
        let mut hits = Vec::new();
        let mut consulted = Vec::new();
        for list in self.inner.candidates(normalized.as_bytes(), limit).await? {
            hits.extend(list.hits);
            consulted.extend(list.consulted);
        }
        self.finish(normalized, hits, consulted).await
    }

    async fn finish(
        &self,
        normalized: String,
        hits: Vec<Hit>,
        consulted: Vec<(usize, bool)>,
    ) -> Result<CollectionCompletion> {
        let requests_before_payload = self.stats().reads;
        let payloads = self.payloads(&hits).await?;
        Ok(CollectionCompletion {
            normalized,
            hits,
            consulted,
            payloads,
            requests_before_payload,
        })
    }

    /// The live documents holding every term of what was typed, split by the
    /// collection's tokenizer, in collection order, unranked: the first
    /// `limit` with their payload.
    ///
    /// Each segment is asked on its own, its terms one at a time in the order
    /// typed, and stops at the first that is missing or that empties the
    /// intersection, so the cost is the sum of what each segment costs.
    pub async fn search(&self, text: &str, limit: usize) -> Result<CollectionSearch> {
        let terms: Vec<String> = match self.tokenizer {
            Some(t) => t.split(text),
            None => vec![text.to_string()],
        };
        let keys: Vec<&[u8]> = terms.iter().map(|t| t.as_bytes()).collect();
        let mut hits = self.inner.search(&keys).await?;
        let matched = hits.len();
        hits.truncate(limit);
        let payloads = self.payloads(&hits).await?;
        Ok(CollectionSearch {
            terms,
            matched,
            hits,
            payloads,
        })
    }

    /// Payload for hits, one span per segment, in the hits' order.
    pub async fn payloads(&self, hits: &[Hit]) -> Result<Vec<Payload>> {
        Ok(self.inner.payloads(hits).await?)
    }
}

impl From<manifest::Error> for Error {
    fn from(e: manifest::Error) -> Self {
        Error::Collection(e)
    }
}
