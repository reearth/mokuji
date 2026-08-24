//! Reading a COLex file (`SPEC.md` §4, §5).

use std::collections::BTreeMap;

use colex_codec::{block, varint};
use colex_format::{
    parse_section_table, unzigzag, Error, Flags, Header, PayloadCodec, Result, SectionCodec,
    SectionEntry, SectionId,
};

use crate::term::{self, find_in_block, TermEntry, TermIndex};
use crate::{postings, RangeReader};

/// How much of the file to fetch when opening it.
///
/// The resident set is meant to be fetched in one range read (`SPEC.md` §4),
/// but its size is only known after the section table has been parsed — which
/// is itself part of it. Reading a prefix optimistically resolves that: for any
/// file whose resident set fits, opening costs exactly one read. The value is a
/// reader's choice, not a property of the format.
const OPEN_PREFIX: u32 = 64 * 1024;

/// How wide a single read may grow when gathering the resident set.
///
/// Beyond this, reading the gaps between sections would cost more than the
/// reads it saves, and the reader falls back to fetching each on its own.
const MAX_RESIDENT_SPAN: u64 = 8 * 1024 * 1024;

/// One document's payload, as the file writes it down (`SPEC.md` §5.5).
///
/// What it means is still the application's business. This says only how it was
/// stored, which is the difference between an encoding and a meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// Bytes, as stored.
    Opaque(Vec<u8>),
    /// A row of signed integers, each standing for `value * 10^exponent`.
    Ints {
        /// The values, already un-delta'd.
        values: Vec<i64>,
        /// The exponent the file declares. Recorded, not interpreted.
        scale_exponent: i8,
    },
}

impl Payload {
    /// The bytes, for an opaque payload.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Payload::Opaque(bytes) => Some(bytes),
            Payload::Ints { .. } => None,
        }
    }

    /// The values, for an integer payload.
    pub fn values(&self) -> Option<&[i64]> {
        match self {
            Payload::Ints { values, .. } => Some(values),
            Payload::Opaque(_) => None,
        }
    }
}

/// One match: a document and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Document ID.
    pub doc: u64,
    /// The document's payload, empty and opaque if the file carries none.
    pub payload: Payload,
}

/// An open COLex file.
///
/// Everything resident is held here; everything else is fetched per query.
#[derive(Debug)]
pub struct Index<R> {
    reader: R,
    header: Header,
    sections: Vec<SectionEntry>,
    meta: Vec<u8>,
    term_index: Vec<u8>,
    term_block_offsets: Vec<u8>,
    payload_offsets: Vec<u8>,
    term_blocks_codec: SectionCodec,
    payload_codec: SectionCodec,
}

impl<R: RangeReader> Index<R> {
    /// Opens a file, loading the resident set.
    pub async fn open(reader: R) -> Result<Self> {
        let prefix_len = OPEN_PREFIX.min(u32::try_from(reader.file_len()).unwrap_or(u32::MAX));
        let prefix = reader.read_range(0, prefix_len).await?;

        let header = Header::parse(&prefix)?;
        if header.flags.contains(Flags::TERMDICT_MONOLITHIC) {
            return Err(Error::Malformed {
                detail: "monolithic term dictionaries are not readable over range reads",
            });
        }

        let table = fetch(
            &reader,
            &prefix,
            header.section_table_offset,
            u64::from(header.section_table_len),
        )
        .await?;
        let sections = parse_section_table(&table, header.section_count)?;
        for section in &sections {
            section.check_bounds(reader.file_len())?;
        }

        let mut required = vec![
            SectionId::META,
            SectionId::TERM_INDEX,
            SectionId::TERM_BLOCK_OFFSETS,
        ];
        if header.flags.contains(Flags::HAS_PAYLOAD) {
            required.push(SectionId::PAYLOAD_OFFSETS);
        }
        let wanted = required
            .iter()
            .map(|&id| {
                let section = find(&sections, id)?;
                Ok((id, section.offset, section.length, section.codec))
            })
            .collect::<Result<Vec<_>>>()?;

        // The recommended layout puts the resident sections together at the
        // head of the file, so when the opening prefix does not reach far
        // enough the remedy is to extend it once rather than to fetch each
        // section on its own. Extending rather than re-reading also keeps a
        // section that straddles the boundary from costing a read of its own.
        let span_end = wanted
            .iter()
            .map(|(_, o, l, _)| o + l)
            .max()
            .expect("non-empty");
        let mut head = prefix;
        let held = head.len() as u64;
        if span_end > held && span_end - held <= MAX_RESIDENT_SPAN {
            let len = u32::try_from(span_end - held).expect("bounded above");
            let tail = reader.read_range(held, len).await?;
            head.extend_from_slice(&tail);
        }

        let mut resident = BTreeMap::new();
        for (id, offset, length, codec) in wanted {
            // A file whose resident sections are scattered too widely to gather
            // is still readable; it just costs a read each.
            let bytes = fetch(&reader, &head, offset, length).await?;
            // A resident section is read whole, so the whole of it is one unit.
            resident.insert(id, decode_unit(codec, bytes)?);
        }

        // Postings are addressed one run at a time, and a run is a handful of
        // bytes; compressing each would grow the section and would break the
        // single request that covers a contiguous range of them.
        if find(&sections, SectionId::POSTINGS)?.codec != SectionCodec::Raw {
            return Err(Error::Malformed {
                detail: "the postings section must be stored raw",
            });
        }
        let term_blocks_codec = find(&sections, SectionId::TERM_BLOCKS)?.codec;
        let payload_codec = if header.flags.contains(Flags::HAS_PAYLOAD) {
            find(&sections, SectionId::PAYLOAD)?.codec
        } else {
            SectionCodec::Raw
        };

        Ok(Index {
            header,
            meta: resident.remove(&SectionId::META).expect("required above"),
            term_index: resident
                .remove(&SectionId::TERM_INDEX)
                .expect("required above"),
            term_block_offsets: resident
                .remove(&SectionId::TERM_BLOCK_OFFSETS)
                .expect("required above"),
            payload_offsets: resident
                .remove(&SectionId::PAYLOAD_OFFSETS)
                .unwrap_or_default(),
            term_blocks_codec,
            payload_codec,
            sections,
            reader,
        })
    }

    /// The fixed header, including the global statistics BM25 needs.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The section table.
    pub fn sections(&self) -> &[SectionEntry] {
        &self.sections
    }

    /// The metadata section, as canonical CBOR.
    ///
    /// Returned unparsed: provenance is checked by `colex verify`, and a reader
    /// that only answers queries has no reason to interpret it.
    pub fn meta(&self) -> &[u8] {
        &self.meta
    }

    /// The resident term index.
    pub fn term_index(&self) -> Result<TermIndex<'_>> {
        TermIndex::new(&self.term_index)
    }

    /// Reads one term block whole, at a cost of one range read.
    pub async fn read_term_block(&self, ordinal: u32) -> Result<Vec<u8>> {
        let block_count = TermIndex::new(&self.term_index)?.block_count();
        let (offset, len) = self.term_block_bounds(ordinal, block_count)?;
        let bytes = self.reader.read_range(offset, len).await?;
        decode_unit(self.term_blocks_codec, bytes)
    }

    /// Reads one document's payload.
    ///
    /// Each call reads a block. A query goes through [`Index::search`] instead,
    /// which shares a block between documents adjacent in ID order.
    pub async fn read_payload(&self, doc: u64) -> Result<Payload> {
        self.payload(doc, &mut PayloadCache::default()).await
    }

    /// Looks a term up, at a cost of one range read.
    pub async fn lookup(&self, term: &[u8]) -> Result<Option<TermEntry>> {
        let index = TermIndex::new(&self.term_index)?;
        let Some(ordinal) = index.locate(term)? else {
            // The term sorts before every block, so no block need be read.
            return Ok(None);
        };
        let block = self.read_term_block(ordinal).await?;
        find_in_block(&block, term)
    }

    /// The postings a dictionary entry implies by carrying no run.
    ///
    /// `SPEC.md` §5.3.4: the term names exactly one document, whose ID is the
    /// term's own, at frequency one.
    fn implied(&self, entry: &TermEntry) -> Result<postings::Postings> {
        if entry.doc_freq != 1 {
            return Err(Error::Malformed {
                detail: "an entry with implied postings must have a doc_freq of 1",
            });
        }
        if entry.term_id >= self.header.num_docs {
            return Err(Error::Malformed {
                detail: "implied postings name a document outside the index",
            });
        }
        Ok(postings::Postings {
            docs: vec![entry.term_id],
            freqs: vec![1],
        })
    }

    /// Reads and decodes one term's postings, or no read at all when the
    /// dictionary entry already said everything (`SPEC.md` §5.3.4).
    pub async fn postings(&self, entry: &TermEntry) -> Result<postings::Postings> {
        if entry.postings_len == 0 {
            return self.implied(entry);
        }
        let len = u32::try_from(entry.postings_len).map_err(|_| Error::Malformed {
            detail: "postings run is implausibly long",
        })?;
        let run = self.reader.read_range(entry.postings_offset, len).await?;
        let decoded = postings::decode(
            &run,
            self.header.flags.contains(Flags::HAS_FREQS),
            self.header.num_docs,
        )?;
        if decoded.len() as u64 != entry.doc_freq {
            return Err(Error::Malformed {
                detail: "postings run disagrees with the dictionary's doc_freq",
            });
        }
        Ok(decoded)
    }

    /// Terms beginning with `prefix`, in order, at most `limit` of them.
    ///
    /// This is the query a hierarchical key is for — an address, a path, a
    /// statute number. It is not a feature layered on top of the dictionary but
    /// a consequence of its shape: terms are sorted, so everything sharing a
    /// prefix is contiguous, and contiguous terms occupy contiguous blocks.
    ///
    /// Cost is one read per block the range spans, which is a function of how
    /// many terms match rather than of how long the prefix is.
    pub async fn prefix(&self, prefix: &[u8], limit: usize) -> Result<Vec<(Vec<u8>, TermEntry)>> {
        let index = TermIndex::new(&self.term_index)?;
        let boundaries = index.keys()?;
        // A prefix sorts at or before the first term it matches, so the block
        // that would contain the prefix itself is where the range starts.
        let start = index.locate(prefix)?.unwrap_or(0);

        let mut out = Vec::new();
        for ordinal in start..index.block_count() {
            // Whether the range continues past this block is answerable from
            // the resident index alone: a block whose first term does not share
            // the prefix holds nothing that does. Only the first block has to
            // be read on faith.
            if ordinal > start && !boundaries[ordinal as usize].starts_with(prefix) {
                break;
            }

            let block = self.read_term_block(ordinal).await?;
            for (key, entry) in term::decode_block(&block)? {
                if key.starts_with(prefix) {
                    out.push((key, entry));
                    if out.len() == limit {
                        return Ok(out);
                    }
                } else if key.as_slice() > prefix && !out.is_empty() {
                    // Terms ascend, so the first non-matching key after a match
                    // ends the range; nothing later can rejoin it.
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// Reads the postings for a contiguous run of dictionary entries in one go.
    ///
    /// Postings are written in term order, so entries adjacent in the
    /// dictionary have adjacent runs. A prefix match is exactly such a run,
    /// which is why completing a prefix costs a constant number of reads rather
    /// than one per match.
    pub async fn postings_span(&self, entries: &[TermEntry]) -> Result<Vec<postings::Postings>> {
        // Entries whose postings are implied contribute no bytes, so an index
        // of a dictionary — where every entry is implied — spends no read here
        // at all.
        let stored: Vec<&TermEntry> = entries.iter().filter(|e| e.postings_len > 0).collect();
        for pair in stored.windows(2) {
            if pair[1].postings_offset < pair[0].postings_offset {
                return Err(Error::Malformed {
                    detail: "postings runs are not in term order",
                });
            }
        }
        let span = match (stored.first(), stored.last()) {
            (Some(first), Some(last)) => {
                let start = first.postings_offset;
                let end = last.postings_offset + last.postings_len;
                let len = u32::try_from(end - start).map_err(|_| Error::Malformed {
                    detail: "postings span is implausibly long",
                })?;
                Some((start, self.reader.read_range(start, len).await?))
            }
            _ => None,
        };

        let has_freqs = self.header.flags.contains(Flags::HAS_FREQS);
        entries
            .iter()
            .map(|entry| {
                if entry.postings_len == 0 {
                    return self.implied(entry);
                }
                let (start, bytes) = span.as_ref().expect("a stored entry implies a span");
                let at = (entry.postings_offset - start) as usize;
                let run =
                    bytes
                        .get(at..at + entry.postings_len as usize)
                        .ok_or(Error::Malformed {
                            detail: "a postings run lies outside its span",
                        })?;
                postings::decode(run, has_freqs, self.header.num_docs)
            })
            .collect()
    }

    /// Fetches payload for documents given in ascending order.
    ///
    /// Blocks are shared between adjacent documents, so this costs one read per
    /// distinct block rather than one per document.
    pub async fn payload_span(&self, docs: &[u64]) -> Result<Vec<Payload>> {
        let mut cache = PayloadCache::default();
        let mut out = Vec::with_capacity(docs.len());
        for &doc in docs {
            out.push(self.payload(doc, &mut cache).await?);
        }
        Ok(out)
    }

    /// Intersects `terms` and returns the matching documents with their payload.
    ///
    /// Costs at most `2k + m` reads for `k` terms and `m` results (`SPEC.md`
    /// §4). Documents sharing a payload block share a read, which is what makes
    /// the payload term smaller than the result count in practice.
    pub async fn search(&self, terms: &[&[u8]]) -> Result<Vec<Match>> {
        let mut matched: Option<Vec<u64>> = None;
        for term in terms {
            let Some(entry) = self.lookup(term).await? else {
                return Ok(Vec::new());
            };
            let docs = self.postings(&entry).await?.docs;
            matched = Some(match matched {
                None => docs,
                Some(previous) => intersect(&previous, &docs),
            });
            if matched.as_ref().is_some_and(Vec::is_empty) {
                // Nothing can rejoin an empty intersection, so the remaining
                // terms are not worth a read.
                return Ok(Vec::new());
            }
        }

        let matched = matched.unwrap_or_default();
        let mut cache = PayloadCache::default();
        let mut out = Vec::with_capacity(matched.len());
        for doc in matched {
            out.push(Match {
                doc,
                payload: self.payload(doc, &mut cache).await?,
            });
        }
        Ok(out)
    }

    /// Fetches one document's payload, reusing a block already read.
    async fn payload(&self, doc: u64, cache: &mut PayloadCache) -> Result<Payload> {
        if !self.header.flags.contains(Flags::HAS_PAYLOAD) {
            return Ok(Payload::Opaque(Vec::new()));
        }
        let section = &self.payload_offsets;
        let head = section.get(0..12).ok_or(Error::Malformed {
            detail: "payload offsets section is too short",
        })?;
        let codec = PayloadCodec::from_byte(head[0])?;
        let values_per_doc = head[1];
        let scale_exponent = head[2] as i8;
        if head[3] != 0 {
            return Err(Error::Malformed {
                detail: "reserved payload offset bytes are non-zero",
            });
        }
        let per_block = u64::from(u32::from_le_bytes(head[4..8].try_into().expect("length")));
        let block_count = u32::from_le_bytes(head[8..12].try_into().expect("length"));
        if per_block == 0 {
            return Err(Error::Malformed {
                detail: "payload block size is zero",
            });
        }

        let ordinal = doc / per_block;
        let index = doc % per_block;
        if ordinal >= u64::from(block_count) {
            return Err(Error::Malformed {
                detail: "document has no payload block",
            });
        }

        // Results are visited in document ID order and payload is stored in
        // that order, so documents sharing a block share a read. This is the
        // difference between m and m' in SPEC.md §4.
        if cache.ordinal != Some(ordinal) {
            let (offset, len) = bounds(section, 12, ordinal as usize)?;
            let bytes = self.reader.read_range(offset, len).await?;
            cache.block = decode_unit(self.payload_codec, bytes)?;
            cache.ordinal = Some(ordinal);
        }
        match codec {
            PayloadCodec::Opaque => Ok(Payload::Opaque(opaque_entry(&cache.block, index)?)),
            PayloadCodec::DeltaInts => Ok(Payload::Ints {
                values: int_entry(&cache.block, index, values_per_doc)?,
                scale_exponent,
            }),
        }
    }

    fn term_block_bounds(&self, ordinal: u32, block_count: u32) -> Result<(u64, u32)> {
        if ordinal >= block_count {
            return Err(Error::Malformed {
                detail: "term block ordinal is out of range",
            });
        }
        if self.term_block_offsets.len() as u64 != 8 * (u64::from(block_count) + 1) {
            return Err(Error::Malformed {
                detail: "term block offsets do not match the number of blocks",
            });
        }
        bounds(&self.term_block_offsets, 0, ordinal as usize)
    }
}

/// The block most recently read, so that documents adjacent in ID order cost
/// one read between them rather than one each.
///
/// A single slot rather than a map: results are visited in document ID order
/// and payload is stored in that order, so a block is never revisited.
#[derive(Debug, Default)]
struct PayloadCache {
    ordinal: Option<u64>,
    block: Vec<u8>,
}

/// Reads an offset pair from a table of `u64` boundaries.
fn bounds(table: &[u8], base: usize, index: usize) -> Result<(u64, u32)> {
    let at = |i: usize| -> Result<u64> {
        let start = base + i * 8;
        let raw = table.get(start..start + 8).ok_or(Error::Malformed {
            detail: "offset table is too short",
        })?;
        Ok(u64::from_le_bytes(raw.try_into().expect("checked length")))
    };
    let start = at(index)?;
    let end = at(index + 1)?;
    let len = end.checked_sub(start).ok_or(Error::Malformed {
        detail: "offset table is not increasing",
    })?;
    let len = u32::try_from(len).map_err(|_| Error::Malformed {
        detail: "block is implausibly long",
    })?;
    Ok((start, len))
}

/// Reads one entry from an opaque payload block.
fn opaque_entry(block: &[u8], index: u64) -> Result<Vec<u8>> {
    let (count, mut pos) = varint::decode(block)?;
    if index >= count {
        return Err(Error::Malformed {
            detail: "payload block holds fewer entries than expected",
        });
    }
    for i in 0..count {
        let (len, n) = varint::decode(&block[pos.min(block.len())..])?;
        pos += n;
        let len = usize::try_from(len).map_err(|_| Error::Malformed {
            detail: "payload entry is implausibly long",
        })?;
        let value = block.get(pos..pos + len).ok_or(Error::Truncated {
            expected: pos + len,
            found: block.len(),
        })?;
        if i == index {
            return Ok(value.to_vec());
        }
        pos += len;
    }
    unreachable!("index was bounds-checked against count")
}

/// Reads one entry from a delta-coded integer payload block.
///
/// Every entry up to the wanted one has to be decoded, because each is a
/// difference from the one before it. Blocks are sized so that is cheap, and a
/// block is the unit a reader fetches anyway.
fn int_entry(block: &[u8], index: u64, values_per_doc: u8) -> Result<Vec<i64>> {
    let (count, mut pos) = varint::decode(block)?;
    if index >= count {
        return Err(Error::Malformed {
            detail: "payload block holds fewer entries than expected",
        });
    }

    let width = usize::from(values_per_doc);
    let mut current = vec![0i64; width];
    for entry in 0..=index {
        for slot in current.iter_mut().take(width) {
            let (raw, n) = varint::decode(&block[pos.min(block.len())..])?;
            pos += n;
            let delta = unzigzag(raw);
            *slot = if entry == 0 {
                delta
            } else {
                slot.wrapping_add(delta)
            };
        }
    }
    Ok(current)
}

fn intersect(a: &[u64], b: &[u64]) -> Vec<u64> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Decodes one addressed unit according to its section's codec.
fn decode_unit(codec: SectionCodec, bytes: Vec<u8>) -> Result<Vec<u8>> {
    match codec {
        SectionCodec::Raw => Ok(bytes),
        SectionCodec::Deflate => block::decompress(&bytes),
        SectionCodec::Zstd => Err(Error::Malformed {
            detail: "zstd is registered but not implemented by this reader",
        }),
    }
}

fn find(sections: &[SectionEntry], id: SectionId) -> Result<&SectionEntry> {
    sections
        .iter()
        .find(|s| s.id == id)
        .ok_or(Error::Malformed {
            detail: "a required section is missing",
        })
}

/// Serves a range from the opening prefix when it falls inside, or fetches it.
///
/// This is what makes opening cost one read: every resident section of a
/// typical file lies within the prefix already in hand.
async fn fetch<R: RangeReader>(
    reader: &R,
    prefix: &[u8],
    offset: u64,
    length: u64,
) -> Result<Vec<u8>> {
    let end = offset.checked_add(length).ok_or(Error::OutOfBounds {
        offset,
        len: length,
        file_len: reader.file_len(),
    })?;
    if end <= prefix.len() as u64 {
        return Ok(prefix[offset as usize..end as usize].to_vec());
    }
    let length = u32::try_from(length).map_err(|_| Error::Malformed {
        detail: "section is implausibly long",
    })?;
    reader.read_range(offset, length).await
}
