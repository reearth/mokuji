//! Reading a COLex file (`SPEC.md` §4, §5).

use std::collections::BTreeMap;

use colex_codec::varint;
use colex_format::{
    parse_section_table, Error, Flags, Header, Result, SectionCodec, SectionEntry, SectionId,
};

use crate::term::{find_in_block, TermEntry, TermIndex};
use crate::{postings, RangeReader};

/// How much of the file to fetch when opening it.
///
/// The resident set is meant to be fetched in one range read (`SPEC.md` §4),
/// but its size is only known after the section table has been parsed — which
/// is itself part of it. Reading a prefix optimistically resolves that: for any
/// file whose resident set fits, opening costs exactly one read. The value is a
/// reader's choice, not a property of the format.
const OPEN_PREFIX: u32 = 64 * 1024;

/// One match: a document and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Document ID.
    pub doc: u64,
    /// The document's payload, or empty if the file carries none.
    ///
    /// Payload bytes are opaque to the format (`SPEC.md` §5.5); interpreting
    /// them is the application's business.
    pub payload: Vec<u8>,
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

        let mut resident = BTreeMap::new();
        let mut required = vec![
            SectionId::META,
            SectionId::TERM_INDEX,
            SectionId::TERM_BLOCK_OFFSETS,
        ];
        if header.flags.contains(Flags::HAS_PAYLOAD) {
            required.push(SectionId::PAYLOAD_OFFSETS);
        }
        for id in required {
            let section = find(&sections, id)?;
            if section.codec != SectionCodec::Raw {
                return Err(Error::Malformed {
                    detail: "a resident section is compressed as a whole",
                });
            }
            resident.insert(
                id,
                fetch(&reader, &prefix, section.offset, section.length).await?,
            );
        }

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

    /// Looks a term up, at a cost of one range read.
    pub async fn lookup(&self, term: &[u8]) -> Result<Option<TermEntry>> {
        let index = TermIndex::new(&self.term_index)?;
        let Some(ordinal) = index.locate(term)? else {
            // The term sorts before every block, so no block need be read.
            return Ok(None);
        };
        let (offset, len) = self.term_block_bounds(ordinal, index.block_count())?;
        let block = self.reader.read_range(offset, len).await?;
        find_in_block(&block, term)
    }

    /// Reads and decodes one term's postings, at a cost of one range read.
    pub async fn postings(&self, entry: &TermEntry) -> Result<postings::Postings> {
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
    async fn payload(&self, doc: u64, cache: &mut PayloadCache) -> Result<Vec<u8>> {
        if !self.header.flags.contains(Flags::HAS_PAYLOAD) {
            return Ok(Vec::new());
        }
        let section = &self.payload_offsets;
        let head = section.get(0..8).ok_or(Error::Malformed {
            detail: "payload offsets section is too short",
        })?;
        let per_block = u64::from(u32::from_le_bytes(head[0..4].try_into().expect("length")));
        let block_count = u32::from_le_bytes(head[4..8].try_into().expect("length"));
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

        if cache.ordinal != Some(ordinal) {
            let (offset, len) = bounds(section, 8, ordinal as usize)?;
            cache.block = self.reader.read_range(offset, len).await?;
            cache.ordinal = Some(ordinal);
        }
        entry_in_payload_block(&cache.block, index)
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

fn entry_in_payload_block(block: &[u8], index: u64) -> Result<Vec<u8>> {
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
