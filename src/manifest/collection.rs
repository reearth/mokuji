//! Querying a collection as the concatenation of its segments.

use crate::meta::Meta;
use crate::read::{Index, Payload, RangeReader};

use crate::manifest::{Error, Manifest, Result};

/// One document of a collection: which segment it is in, and its ID there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hit {
    /// Position of the segment in the manifest.
    pub segment: usize,
    /// The document's ID within that segment.
    pub doc: u64,
}

/// What completing a prefix across a collection found, and how.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    /// In collection order: segments in manifest order, documents in each
    /// segment's order.
    pub hits: Vec<Hit>,
    /// Segments consulted, and for each whether its completions section
    /// answered without a read.
    pub consulted: Vec<(usize, bool)>,
}

struct Member<R> {
    index: Index<R>,
    deletes: Vec<u64>,
}

impl<R> Member<R> {
    fn live(&self, doc: u64) -> bool {
        self.deletes.binary_search(&doc).is_err()
    }
}

/// A collection, open: every segment's resident set held, deletes in hand.
pub struct Collection<R> {
    members: Vec<Member<R>>,
}

impl<R: RangeReader> Collection<R> {
    /// Opens every segment the manifest lists, one reader each, in order.
    ///
    /// Checks what the manifest claims about each file that can be checked
    /// without reading all of it — its length, and that its deletes name
    /// documents it has — and that every segment was tokenized the same way,
    /// since one query has to mean the same thing in all of them.
    pub async fn open(manifest: &Manifest, readers: Vec<R>) -> Result<Self> {
        if readers.len() != manifest.segments.len() {
            return Err(Error::Manifest(format!(
                "{} segments listed and {} readers given",
                manifest.segments.len(),
                readers.len()
            )));
        }
        let mut members = Vec::with_capacity(readers.len());
        let mut tokenizer: Option<crate::meta::Tokenizer> = None;
        for (segment, reader) in manifest.segments.iter().zip(readers) {
            let fail = |detail: String| Error::Segment {
                path: segment.path.clone(),
                detail,
            };
            if reader.file_len() != segment.length {
                return Err(fail(format!(
                    "is {} bytes where the manifest says {}",
                    reader.file_len(),
                    segment.length
                )));
            }
            let index = Index::open(reader).await?;
            if let Some(&last) = segment.deletes.last() {
                if last >= index.header().num_docs {
                    return Err(fail(format!(
                        "deletes document {last} of {}",
                        index.header().num_docs
                    )));
                }
            }
            let meta = Meta::parse(index.meta())?;
            match &tokenizer {
                None => tokenizer = Some(meta.tokenizer),
                Some(first) if *first != meta.tokenizer => {
                    return Err(fail(format!(
                        "was tokenized by '{}', and the first segment by '{}'",
                        meta.tokenizer.id, first.id
                    )))
                }
                Some(_) => {}
            }
            members.push(Member {
                index,
                deletes: segment.deletes.clone(),
            });
        }
        Ok(Collection { members })
    }

    /// Number of segments.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the collection has no segments; never, once opened.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// One segment, open.
    pub fn segment(&self, i: usize) -> &Index<R> {
        &self.members[i].index
    }

    /// Whether a document is still in the collection.
    pub fn is_live(&self, hit: Hit) -> bool {
        self.members[hit.segment].live(hit.doc)
    }

    /// Documents holding every term, in collection order.
    ///
    /// Each segment is asked on its own, so the cost is the sum of what each
    /// costs (`SPEC.md` §4) — and the segments are independent, so a reader
    /// that asks them at once pays in round trips only for the slowest.
    pub async fn search(&self, terms: &[&[u8]]) -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        for (segment, member) in self.members.iter().enumerate() {
            let mut matched: Option<Vec<u64>> = None;
            for term in terms {
                let Some(entry) = member.index.lookup(term).await? else {
                    matched = Some(Vec::new());
                    break;
                };
                let docs = member.index.postings(&entry).await?.docs;
                matched = Some(match matched {
                    None => docs,
                    Some(previous) => previous
                        .into_iter()
                        .filter(|d| docs.binary_search(d).is_ok())
                        .collect(),
                });
                if matched.as_ref().is_some_and(Vec::is_empty) {
                    break;
                }
            }
            hits.extend(
                matched
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&doc| member.live(doc))
                    .map(|doc| Hit { segment, doc }),
            );
        }
        Ok(hits)
    }

    /// The first `limit` live documents holding a term that begins with
    /// `prefix`, in collection order.
    ///
    /// Segments are consulted in order until `limit` are found. In each, the
    /// completions section answers first. If deletes have taken some of what
    /// it gives, the segment is asked for everything its entry lists — an entry
    /// that lists more than a page shows has room for deletes, at no read —
    /// and then, if that is still short, for as many again as it has deletes,
    /// which is enough however they fall and is read from its dictionary. A
    /// delete that exhausts an entry's room makes that prefix cost what it
    /// would without the section, and stay correct, until the collection is
    /// compacted.
    pub async fn complete(&self, prefix: &[u8], limit: usize) -> Result<Completion> {
        let mut out = Completion::default();
        for segment in 0..self.members.len() {
            let need = limit - out.hits.len();
            if need == 0 {
                break;
            }
            let found = self.complete_in(segment, prefix, need).await?;
            out.hits.extend(found.hits);
            out.consulted.extend(found.consulted);
        }
        Ok(out)
    }

    /// Every segment's own first `limit` live documents for `prefix`, one list
    /// per segment in manifest order.
    ///
    /// What a reader needs to rank across segments. Collection order puts every
    /// document of an earlier segment before any of a later one, so a place
    /// added in a later segment is never among the first `limit` of a prefix
    /// the base already fills. Ranking the lists against each other needs a
    /// score the segments share, and scores are outside the format (`SPEC.md`
    /// §1) — so this returns the lists and leaves the ranking to the caller. It
    /// costs every segment's reads rather than stopping once `limit` are found;
    /// the segments are independent, so they can be asked at once.
    pub async fn candidates(&self, prefix: &[u8], limit: usize) -> Result<Vec<Completion>> {
        let mut out = Vec::with_capacity(self.members.len());
        for segment in 0..self.members.len() {
            out.push(self.complete_in(segment, prefix, limit).await?);
        }
        Ok(out)
    }

    async fn complete_in(&self, segment: usize, prefix: &[u8], need: usize) -> Result<Completion> {
        let member = &self.members[segment];
        // Asked for what is needed; then, if deletes took some of it, for all
        // the segment's entry lists, which answers at no read when the entry
        // lists more than is needed; then for as many again as there are
        // deletes, which is enough however they fall.
        let listed = member
            .index
            .completions()
            .map_or(0, |c| c.docs_per_entry as usize);
        let mut from_table = true;
        let mut live = Vec::new();
        for ask in [need, listed.max(need), need + member.deletes.len()] {
            let (docs, table) = member.index.complete(prefix, ask).await?;
            from_table &= table;
            live = docs.iter().copied().filter(|&d| member.live(d)).collect();
            if live.len() >= need || docs.len() < ask {
                break;
            }
        }
        live.truncate(need);
        Ok(Completion {
            hits: live.into_iter().map(|doc| Hit { segment, doc }).collect(),
            consulted: vec![(segment, from_table)],
        })
    }

    /// Payload for hits, one span per segment so that neighbours share a read.
    pub async fn payloads(&self, hits: &[Hit]) -> Result<Vec<Payload>> {
        let mut out: Vec<Option<Payload>> = vec![None; hits.len()];
        for (segment, member) in self.members.iter().enumerate() {
            let mut mine: Vec<(usize, u64)> = hits
                .iter()
                .enumerate()
                .filter(|(_, h)| h.segment == segment)
                .map(|(i, h)| (i, h.doc))
                .collect();
            if mine.is_empty() {
                continue;
            }
            mine.sort_by_key(|&(_, doc)| doc);
            let docs: Vec<u64> = mine.iter().map(|&(_, d)| d).collect();
            let payloads = member.index.payload_span(&docs).await?;
            for ((i, _), payload) in mine.into_iter().zip(payloads) {
                out[i] = Some(payload);
            }
        }
        Ok(out
            .into_iter()
            .map(|p| p.expect("every hit is in a segment"))
            .collect())
    }
}
