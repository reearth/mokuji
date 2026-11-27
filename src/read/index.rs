//! Reading a Mokuji file (`SPEC.md` §4, §5).

use std::collections::BTreeMap;

use crate::codec::{bitpack, block, varint};
use crate::format::{
    parse_section_table, unzigzag, Error, Flags, Header, PayloadCodec, Result, SectionCodec,
    SectionEntry, SectionId,
};

use crate::read::term::{self, find_in_block, TermEntry, TermIndex};
use crate::read::{postings, Completions, RangeReader};

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

/// One column of per-document numbers (`SPEC.md` §5.8).
///
/// What the numbers mean is the application's business; the file says only how
/// they were written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// What the column is called.
    pub name: String,
    /// A stored `v` stands for `v * 10^exponent`. Recorded, not interpreted.
    pub scale_exponent: i8,
    /// One value per document, in document ID order.
    pub values: Vec<i64>,
}

/// Decodes the document attributes section.
fn decode_columns(bytes: &[u8], num_docs: u64) -> Result<Vec<Column>> {
    let (count, mut pos) = varint::decode(bytes)?;
    let count = usize::try_from(count).map_err(|_| Error::Malformed {
        detail: "attribute column count overflows",
    })?;

    let mut headers = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let (len, n) = varint::decode(&bytes[pos.min(bytes.len())..])?;
        pos += n;
        let len = usize::try_from(len).map_err(|_| Error::Malformed {
            detail: "attribute column name is implausibly long",
        })?;
        let name = bytes.get(pos..pos + len).ok_or(Error::Truncated {
            expected: pos + len,
            found: bytes.len(),
        })?;
        let name = std::str::from_utf8(name).map_err(|_| Error::Malformed {
            detail: "attribute column name is not valid UTF-8",
        })?;
        pos += len;
        let exponent = *bytes.get(pos).ok_or(Error::Truncated {
            expected: pos + 1,
            found: bytes.len(),
        })? as i8;
        pos += 1;
        if headers.iter().any(|(n, _): &(String, i8)| n == name) {
            return Err(Error::Malformed {
                detail: "two attribute columns share a name",
            });
        }
        headers.push((name.to_string(), exponent));
    }

    let docs = usize::try_from(num_docs).map_err(|_| Error::Malformed {
        detail: "document count overflows",
    })?;
    let mut columns = Vec::with_capacity(headers.len());
    for (name, scale_exponent) in headers {
        let (packed, next) = bitpack::decode(bytes, pos, docs)?;
        pos = next;
        columns.push(Column {
            name,
            scale_exponent,
            values: packed.into_iter().map(unzigzag).collect(),
        });
    }
    if pos != bytes.len() {
        return Err(Error::Malformed {
            detail: "trailing bytes in the document attributes section",
        });
    }
    Ok(columns)
}

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

impl std::fmt::Display for Payload {
    /// Renders a payload as text.
    ///
    /// Presentation, not meaning: an integer payload is written out against the
    /// scale the file declares, keeping the zeros that scale claims, and an
    /// opaque one is shown as text when it is text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Payload::Opaque(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => f.write_str(text),
                Err(_) => write!(f, "<{} bytes>", bytes.len()),
            },
            Payload::Ints {
                values,
                scale_exponent,
            } => {
                for (i, &value) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write_scaled(f, value, *scale_exponent)?;
                }
                Ok(())
            }
        }
    }
}

/// Writes `value * 10^exponent` in full.
fn write_scaled(f: &mut std::fmt::Formatter<'_>, value: i64, exponent: i8) -> std::fmt::Result {
    match exponent {
        0 => write!(f, "{value}"),
        e if e > 0 => {
            write!(f, "{value}")?;
            for _ in 0..e {
                f.write_str("0")?;
            }
            Ok(())
        }
        e => {
            let digits = e.unsigned_abs() as u32;
            let unit = 10i128.pow(digits);
            let value = i128::from(value);
            if value < 0 {
                f.write_str("-")?;
            }
            let magnitude = value.abs();
            write!(
                f,
                "{}.{:0width$}",
                magnitude / unit,
                magnitude % unit,
                width = digits as usize
            )
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

/// How much of the file to hold rather than fetch.
///
/// The format supports both without changing: blocks are independently
/// decodable and the offsets that locate them are resident either way. Which to
/// use is a deployment question — see `SPEC.md` §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// Hold the whole term dictionary, compressed, and decompress one block per
    /// lookup instead of fetching it.
    ///
    /// Costs one read at open and the dictionary's stored size in memory —
    /// 864 KB for 1,744,962 Tokyo addresses — and makes a term lookup cost no
    /// range read at all.
    pub resident_dictionary: bool,
    /// Read neighbouring blocks in one request (`SPEC.md` §4 allows it: the
    /// count can only fall). On by default, as in the other readers; the
    /// conformance tests turn it off to check the count the cases state,
    /// block by block.
    pub coalesce: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            resident_dictionary: false,
            coalesce: true,
        }
    }
}

/// An open Mokuji file.
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
    doc_lengths: Option<Vec<u64>>,
    columns: Vec<Column>,
    completions: Option<Completions>,
    term_blocks_codec: SectionCodec,
    payload_codec: SectionCodec,
    /// The whole term blocks section, if it is being held rather than fetched.
    term_blocks: Option<(u64, Vec<u8>)>,
    coalesce: bool,
}

impl<R: RangeReader> Index<R> {
    /// Opens a file, loading the resident set.
    pub async fn open(reader: R) -> Result<Self> {
        Self::open_with(reader, OpenOptions::default()).await
    }

    /// Opens a file, choosing how much of it to hold.
    pub async fn open_with(reader: R, options: OpenOptions) -> Result<Self> {
        let prefix_len = OPEN_PREFIX.min(u32::try_from(reader.file_len()).unwrap_or(u32::MAX));
        let prefix = reader.read_range(0, prefix_len).await?;

        let header = Header::parse(&prefix)?;

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
        let mut wanted = required
            .iter()
            .map(|&id| {
                let section = find(&sections, id)?;
                Ok((id, section.offset, section.length, section.codec))
            })
            .collect::<Result<Vec<_>>>()?;
        // Resident when present: whether a prefix has an entry has to be known
        // before anything is read for it (SPEC.md §4.1.1).
        if let Some(s) = sections.iter().find(|s| s.id == SectionId::COMPLETIONS) {
            wanted.push((s.id, s.offset, s.length, s.codec));
        }

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

        if header.flags.contains(Flags::HAS_POSITIONS) && !header.flags.contains(Flags::HAS_FREQS) {
            return Err(Error::Malformed {
                detail: "positions need frequencies, which say how many belong to each document",
            });
        }

        // Postings are addressed one run at a time, and a run is a handful of
        // bytes; compressing each would grow the section and would break the
        // single request that covers a contiguous range of them.
        if find(&sections, SectionId::POSTINGS)?.codec != SectionCodec::Raw {
            return Err(Error::Malformed {
                detail: "the postings section must be stored raw",
            });
        }
        // Optional, and read whole: scoring wants many documents' lengths at
        // once, so fetching them one at a time would cost more than holding
        // them all.
        let doc_lengths = match sections.iter().find(|s| s.id == SectionId::DOC_LENGTHS) {
            Some(section) => {
                let bytes = decode_unit(
                    section.codec,
                    fetch(&reader, &head, section.offset, section.length).await?,
                )?;
                let count = usize::try_from(header.num_docs).map_err(|_| Error::Malformed {
                    detail: "document count overflows",
                })?;
                let (values, pos) = bitpack::decode(&bytes, 0, count)?;
                if pos != bytes.len() {
                    return Err(Error::Malformed {
                        detail: "trailing bytes in the document lengths section",
                    });
                }
                Some(values)
            }
            None => None,
        };

        // Read whole for the same reason as the lengths: they exist to be
        // consulted about many documents at once.
        let columns = match sections.iter().find(|s| s.id == SectionId::DOC_ATTRIBUTES) {
            Some(section) => {
                let bytes = decode_unit(
                    section.codec,
                    fetch(&reader, &head, section.offset, section.length).await?,
                )?;
                decode_columns(&bytes, header.num_docs)?
            }
            None => Vec::new(),
        };

        let completions = match resident.remove(&SectionId::COMPLETIONS) {
            Some(bytes) => Some(Completions::decode(&bytes, header.num_docs)?),
            None => None,
        };

        let term_blocks_codec = find(&sections, SectionId::TERM_BLOCKS)?.codec;
        let payload_codec = if header.flags.contains(Flags::HAS_PAYLOAD) {
            find(&sections, SectionId::PAYLOAD)?.codec
        } else {
            SectionCodec::Raw
        };

        let term_blocks = if options.resident_dictionary {
            let section = find(&sections, SectionId::TERM_BLOCKS)?;
            let len = u32::try_from(section.length).map_err(|_| Error::Malformed {
                detail: "the term dictionary is too large to hold",
            })?;
            // Held as stored. Decompressing it here would cost twenty times the
            // memory to save a millisecond a lookup.
            Some((
                section.offset,
                reader.read_range(section.offset, len).await?,
            ))
        } else {
            None
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
            term_blocks,
            coalesce: options.coalesce,
            doc_lengths,
            columns,
            completions,
            sections,
            reader,
        })
    }

    /// Whether neighbouring blocks are read in one request.
    pub fn coalesce(&self) -> bool {
        self.coalesce
    }

    /// Turns reading neighbouring blocks in one request on or off.
    pub fn set_coalesce(&mut self, on: bool) {
        self.coalesce = on;
    }

    /// What the file is read through, so a caller can ask a `CountingReader`
    /// what was read.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// The fixed header, including the global statistics BM25 needs.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The section table.
    pub fn sections(&self) -> &[SectionEntry] {
        &self.sections
    }

    /// The length in tokens of every document, if the file carries them.
    ///
    /// `SPEC.md` §5.7. The header gives the count and the total, which is
    /// enough for the mean; this is what a scorer needs to normalize by the
    /// length of the document actually in front of it.
    pub fn doc_lengths(&self) -> Option<&[u64]> {
        self.doc_lengths.as_deref()
    }

    /// The length in tokens of one document.
    pub fn doc_length(&self, doc: u64) -> Option<u64> {
        self.doc_lengths
            .as_ref()
            .and_then(|lengths| lengths.get(doc as usize).copied())
    }

    /// What decoding a term block needs to know about the file.
    pub fn block_shape(&self) -> term::BlockShape {
        term::BlockShape {
            positions: self.has_positions(),
            freqs: self.header.flags.contains(Flags::HAS_FREQS),
            num_docs: self.header.num_docs,
        }
    }

    /// Whether the file carries positions (`SPEC.md` §5.6a).
    pub fn has_positions(&self) -> bool {
        self.header.flags.contains(Flags::HAS_POSITIONS)
    }

    /// Where each of a term's occurrences sits, per document.
    ///
    /// One further range read, and only for a query that tests a phrase. The
    /// returned lists are parallel to `postings.docs`.
    pub async fn positions(
        &self,
        entry: &TermEntry,
        postings: &postings::Postings,
    ) -> Result<Vec<Vec<u64>>> {
        if !self.has_positions() || entry.positions_len == 0 {
            return Ok(Vec::new());
        }
        let len = u32::try_from(entry.positions_len).map_err(|_| Error::Malformed {
            detail: "positions run is implausibly long",
        })?;
        let run = self.reader.read_range(entry.positions_offset, len).await?;

        // A run is decoded from the front: the frequencies say how many values
        // belong to each document, and they come from the postings.
        let mut pos = 0;
        let mut out = Vec::with_capacity(postings.docs.len().min(1024));
        for &freq in &postings.freqs {
            let count = usize::try_from(freq).map_err(|_| Error::Malformed {
                detail: "term frequency overflows",
            })?;
            let (values, next) = bitpack::decode(&run, pos, count)?;
            pos = next;

            let mut positions = Vec::with_capacity(count.min(1024));
            let mut current: Option<u64> = None;
            for value in values {
                // The first is a position; the rest are gaps less one, so that
                // adjacent occurrences pack to nothing.
                let at = match current {
                    None => value,
                    Some(previous) => previous
                        .checked_add(value)
                        .and_then(|v| v.checked_add(1))
                        .ok_or(Error::Malformed {
                            detail: "position overflows",
                        })?,
                };
                current = Some(at);
                positions.push(at);
            }
            out.push(positions);
        }
        if pos != run.len() {
            return Err(Error::Malformed {
                detail: "trailing bytes in positions run",
            });
        }
        Ok(out)
    }

    /// The document attribute columns the file carries (`SPEC.md` §5.8).
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// One attribute column by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The metadata section, as canonical CBOR.
    ///
    /// Returned unparsed: provenance is checked by `mokuji verify`, and a reader
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
        let bytes = match &self.term_blocks {
            Some((start, held)) => {
                // The offset comes from a table in the file, so it may point
                // before the section it is supposed to lie in.
                let outside = || Error::Malformed {
                    detail: "term block lies outside the section",
                };
                let at = offset
                    .checked_sub(*start)
                    .and_then(|at| usize::try_from(at).ok())
                    .ok_or_else(outside)?;
                let end = at.checked_add(len as usize).ok_or_else(outside)?;
                held.get(at..end).ok_or_else(outside)?.to_vec()
            }
            None => self.reader.read_range(offset, len).await?,
        };
        decode_unit(self.term_blocks_codec, bytes)
    }

    /// Reads several term blocks, decoded, keyed by ordinal: a request for
    /// each run of neighbouring blocks, or none when the dictionary is held.
    pub async fn read_term_blocks(&self, ordinals: &[u32]) -> Result<BTreeMap<u32, Vec<u8>>> {
        let mut out = BTreeMap::new();
        if self.term_blocks.is_some() {
            for &ordinal in ordinals {
                if let std::collections::btree_map::Entry::Vacant(slot) = out.entry(ordinal) {
                    slot.insert(self.read_term_block(ordinal).await?);
                }
            }
            return Ok(out);
        }
        let block_count = TermIndex::new(&self.term_index)?.block_count();
        let wanted: Vec<u64> = ordinals.iter().map(|&o| u64::from(o)).collect();
        let raw = self
            .block_runs(&wanted, |o| self.term_block_bounds(o as u32, block_count))
            .await?;
        for (ordinal, bytes) in raw {
            out.insert(ordinal as u32, decode_unit(self.term_blocks_codec, bytes)?);
        }
        Ok(out)
    }

    /// Reads blocks as stored, a request for each run of neighbours.
    ///
    /// Blocks are laid end to end, so ordinals that follow one another are
    /// one contiguous range of the file, and reading them as one costs a
    /// single request where reading them apart costs one each.
    async fn block_runs(
        &self,
        ordinals: &[u64],
        bounds: impl Fn(u64) -> Result<(u64, u32)>,
    ) -> Result<BTreeMap<u64, Vec<u8>>> {
        let mut sorted = ordinals.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut runs: Vec<Vec<u64>> = Vec::new();
        for ordinal in sorted {
            match runs.last_mut() {
                Some(run) if self.coalesce && run.last() == Some(&(ordinal - 1)) => {
                    run.push(ordinal);
                }
                _ => runs.push(vec![ordinal]),
            }
        }
        let disorder = || Error::Malformed {
            detail: "blocks are not laid out in order",
        };
        let mut out = BTreeMap::new();
        for run in runs {
            let (start, _) = bounds(run[0])?;
            let (last_offset, last_len) = bounds(*run.last().expect("not empty"))?;
            let end = last_offset
                .checked_add(u64::from(last_len))
                .ok_or_else(disorder)?;
            let len = end.checked_sub(start).ok_or_else(disorder)?;
            let len = u32::try_from(len).map_err(|_| Error::Malformed {
                detail: "a run of blocks is implausibly long",
            })?;
            let bytes = self.reader.read_range(start, len).await?;
            for ordinal in run {
                let (offset, len) = bounds(ordinal)?;
                let at = offset.checked_sub(start).ok_or_else(disorder)? as usize;
                let block = bytes.get(at..at + len as usize).ok_or_else(disorder)?;
                out.insert(ordinal, block.to_vec());
            }
        }
        Ok(out)
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
        find_in_block(&block, term, self.block_shape())
    }

    /// Looks several terms up, at most one range read per run of neighbouring
    /// dictionary blocks they fall in (one per block with coalescing off),
    /// and none for a held dictionary.
    ///
    /// Worth having separately from `lookup` because a term entry carries
    /// `doc_freq`, and a reader that wants to know how expensive a term is
    /// before paying for its postings has to read the block either way. Terms
    /// that sort near each other — the n-grams of one word, the geohash cell
    /// and its parent — share a block and so share the read.
    pub async fn lookup_many(&self, terms: &[&[u8]]) -> Result<Vec<Option<TermEntry>>> {
        let index = TermIndex::new(&self.term_index)?;
        let mut ordinals = Vec::with_capacity(terms.len());
        for term in terms {
            ordinals.push(index.locate(term)?);
        }

        let wanted: Vec<u32> = ordinals.iter().flatten().copied().collect();
        let blocks = self.read_term_blocks(&wanted).await?;

        let mut found = Vec::with_capacity(terms.len());
        for (term, ordinal) in terms.iter().zip(&ordinals) {
            found.push(match ordinal {
                Some(ordinal) => find_in_block(&blocks[ordinal], term, self.block_shape())?,
                // The term sorts before every block, so no block was read.
                None => None,
            });
        }
        Ok(found)
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

    /// The postings the dictionary entry states itself, implied or inline,
    /// or `None` if they are in a run.
    fn stated(&self, entry: &TermEntry) -> Result<Option<postings::Postings>> {
        match entry.postings_len {
            term::IMPLIED => self.implied(entry).map(Some),
            term::INLINE => {
                let inline = entry.inline.clone().ok_or(Error::Malformed {
                    detail: "an inline entry carries no postings",
                })?;
                if inline.len() as u64 != entry.doc_freq {
                    return Err(Error::Malformed {
                        detail: "inline postings disagree with the dictionary's doc_freq",
                    });
                }
                Ok(Some(inline))
            }
            _ => Ok(None),
        }
    }

    /// Reads and decodes one term's postings, or no read at all when the
    /// dictionary entry already said everything (`SPEC.md` §5.3.4).
    pub async fn postings(&self, entry: &TermEntry) -> Result<postings::Postings> {
        if let Some(stated) = self.stated(entry)? {
            return Ok(stated);
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
        if nothing_begins_with(&boundaries, prefix) {
            return Ok(Vec::new());
        }
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
            for (key, entry) in term::decode_block(&block, self.block_shape())? {
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

    /// Terms beginning with each of several prefixes, every one of them.
    ///
    /// Which blocks each prefix spans is answerable from the resident index,
    /// so all of them are known before any is read, and they are fetched
    /// together: a request for each run of neighbouring blocks, however many
    /// prefixes fall in it. This is what a query over an area needs — the
    /// cells covering it are many short prefixes, mostly in blocks next to
    /// one another — where [`Index::prefix`], which walks one prefix a block at
    /// a time, would make a request per block per prefix.
    pub async fn prefix_many(&self, prefixes: &[&[u8]]) -> Result<Vec<Vec<(Vec<u8>, TermEntry)>>> {
        let index = TermIndex::new(&self.term_index)?;
        let boundaries = index.keys()?;
        let mut spans = Vec::with_capacity(prefixes.len());
        let mut ordinals = Vec::new();
        for prefix in prefixes {
            if nothing_begins_with(&boundaries, prefix) {
                spans.push(None);
                continue;
            }
            let start = index.locate(prefix)?.unwrap_or(0);
            let mut end = start;
            while (end + 1) < index.block_count()
                && boundaries[end as usize + 1].starts_with(prefix)
            {
                end += 1;
            }
            ordinals.extend(start..=end);
            spans.push(Some((start, end)));
        }
        let blocks = self.read_term_blocks(&ordinals).await?;
        let mut out = Vec::with_capacity(prefixes.len());
        for (prefix, span) in prefixes.iter().zip(spans) {
            let mut found = Vec::new();
            if let Some((start, end)) = span {
                for ordinal in start..=end {
                    for (key, entry) in term::decode_block(&blocks[&ordinal], self.block_shape())? {
                        if key.starts_with(prefix) {
                            found.push((key, entry));
                        }
                    }
                }
            }
            out.push(found);
        }
        Ok(out)
    }

    /// Terms from `low` to `high`, both inclusive, in order, at most `limit`
    /// of them; a `limit` of zero means all, as for [`Index::prefix`].
    ///
    /// The same walk as `prefix`, with a different test for where it stops:
    /// the dictionary is sorted, so a range of keys is a run of neighbouring
    /// entries, and whether the run continues past a block is answerable from
    /// the resident index alone. A range that ends before the first term
    /// costs no read.
    pub async fn range(
        &self,
        low: &[u8],
        high: &[u8],
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, TermEntry)>> {
        let index = TermIndex::new(&self.term_index)?;
        let boundaries = index.keys()?;
        let mut out = Vec::new();
        match boundaries.first() {
            Some(first) if high >= first.as_slice() => {}
            _ => return Ok(out),
        }
        let start = index.locate(low)?.unwrap_or(0);
        for ordinal in start..index.block_count() {
            if ordinal > start && boundaries[ordinal as usize].as_slice() > high {
                break;
            }
            let block = self.read_term_block(ordinal).await?;
            for (key, entry) in term::decode_block(&block, self.block_shape())? {
                if key.as_slice() > high {
                    return Ok(out);
                }
                if key.as_slice() >= low {
                    out.push((key, entry));
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// The completions section, if the file carries one (`SPEC.md` §5.3.5).
    pub fn completions(&self) -> Option<&Completions> {
        self.completions.as_ref()
    }

    /// The first `limit` documents holding a term that begins with `prefix`,
    /// and whether the completions section answered.
    ///
    /// An entry answers when there is one and it lists enough; that costs no
    /// read at all. Otherwise every term of the prefix is walked and its runs
    /// united, and the documents come back in document order — which is the
    /// order a writer that ranks by numbering would have listed them in, and
    /// no ranking otherwise. Where the file declares coverage, a prefix with no
    /// entry is known to be cheap, so the walk is bounded (§4.1.1).
    pub async fn complete(&self, prefix: &[u8], limit: usize) -> Result<(Vec<u64>, bool)> {
        if let Some(c) = &self.completions {
            if let Some(docs) = c.get(prefix) {
                // An entry lists `docs_per_entry` documents, or every document
                // the prefix has if there are fewer. Either way it answers a
                // limit it reaches, and the second answers any limit.
                if limit <= docs.len() || (docs.len() as u64) < c.docs_per_entry {
                    return Ok((docs[..limit.min(docs.len())].to_vec(), true));
                }
            }
        }
        let entries: Vec<TermEntry> = self
            .prefix(prefix, usize::MAX)
            .await?
            .into_iter()
            .map(|(_, entry)| entry)
            .collect();
        if entries.is_empty() {
            return Ok((Vec::new(), false));
        }
        let mut docs: Vec<u64> = self
            .postings_span(&entries)
            .await?
            .into_iter()
            .flat_map(|p| p.docs)
            .collect();
        docs.sort_unstable();
        docs.dedup();
        docs.truncate(limit);
        Ok((docs, false))
    }

    /// Reads the postings for a contiguous run of dictionary entries in one go.
    ///
    /// Postings are written in term order, so entries adjacent in the
    /// dictionary have adjacent runs. A prefix match is exactly such a run,
    /// which is why completing a prefix costs a constant number of reads rather
    /// than one per match.
    pub async fn postings_span(&self, entries: &[TermEntry]) -> Result<Vec<postings::Postings>> {
        // Entries that state their own postings, implied or inline, contribute
        // no bytes, so an index of a dictionary — where every entry is
        // implied — spends no read here at all.
        let stored: Vec<&TermEntry> = entries.iter().filter(|e| e.run_len() > 0).collect();
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
                if let Some(stated) = self.stated(entry)? {
                    return Ok(stated);
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

    /// Fetches payload for documents, in the order given.
    ///
    /// Blocks are shared between documents, so this costs at most one read per
    /// run of neighbouring blocks the documents fall in (one per distinct
    /// block with coalescing off), rather than one per document.
    pub async fn payload_span(&self, docs: &[u64]) -> Result<Vec<Payload>> {
        if !self.header.flags.contains(Flags::HAS_PAYLOAD) {
            return Ok(vec![Payload::Opaque(Vec::new()); docs.len()]);
        }
        let layout = self.payload_layout()?;
        let mut ordinals = Vec::with_capacity(docs.len());
        for &doc in docs {
            ordinals.push(layout.ordinal(doc)?);
        }
        // Every block the documents fall in, a request for each run of
        // neighbours, so the order the documents come in costs nothing.
        let raw = self
            .block_runs(&ordinals, |o| bounds(&self.payload_offsets, 12, o as usize))
            .await?;
        let mut blocks = BTreeMap::new();
        for (ordinal, bytes) in raw {
            blocks.insert(ordinal, decode_unit(self.payload_codec, bytes)?);
        }
        docs.iter()
            .zip(ordinals)
            .map(|(&doc, ordinal)| layout.entry(&blocks[&ordinal], doc))
            .collect()
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
        let layout = self.payload_layout()?;
        let ordinal = layout.ordinal(doc)?;

        // Results are visited in document ID order and payload is stored in
        // that order, so documents sharing a block share a read. This is the
        // difference between m and m' in SPEC.md §4.
        if cache.ordinal != Some(ordinal) {
            let (offset, len) = bounds(&self.payload_offsets, 12, ordinal as usize)?;
            let bytes = self.reader.read_range(offset, len).await?;
            cache.block = decode_unit(self.payload_codec, bytes)?;
            cache.ordinal = Some(ordinal);
        }
        layout.entry(&cache.block, doc)
    }

    /// The payload offsets section's header.
    fn payload_layout(&self) -> Result<PayloadLayout> {
        let head = self.payload_offsets.get(0..12).ok_or(Error::Malformed {
            detail: "payload offsets section is too short",
        })?;
        let codec = PayloadCodec::from_byte(head[0])?;
        if head[3] != 0 {
            return Err(Error::Malformed {
                detail: "reserved payload offset bytes are non-zero",
            });
        }
        let per_block = u64::from(u32::from_le_bytes(head[4..8].try_into().expect("length")));
        if per_block == 0 {
            return Err(Error::Malformed {
                detail: "payload block size is zero",
            });
        }
        Ok(PayloadLayout {
            codec,
            values_per_doc: head[1],
            scale_exponent: head[2] as i8,
            per_block,
            block_count: u32::from_le_bytes(head[8..12].try_into().expect("length")),
        })
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

/// How payload is laid out, from the payload offsets section's header.
struct PayloadLayout {
    codec: PayloadCodec,
    values_per_doc: u8,
    scale_exponent: i8,
    per_block: u64,
    block_count: u32,
}

impl PayloadLayout {
    /// The block a document's payload is in.
    fn ordinal(&self, doc: u64) -> Result<u64> {
        let ordinal = doc / self.per_block;
        if ordinal >= u64::from(self.block_count) {
            return Err(Error::Malformed {
                detail: "document has no payload block",
            });
        }
        Ok(ordinal)
    }

    /// A document's payload, from its block, decoded.
    fn entry(&self, block: &[u8], doc: u64) -> Result<Payload> {
        let index = doc % self.per_block;
        match self.codec {
            PayloadCodec::Opaque => Ok(Payload::Opaque(opaque_entry(block, index)?)),
            PayloadCodec::DeltaInts => Ok(Payload::Ints {
                values: int_entry(block, index, self.values_per_doc)?,
                scale_exponent: self.scale_exponent,
            }),
        }
    }
}

/// Whether, from the resident index alone, no term can begin with `prefix`.
/// The first boundary key is the smallest term; when it sorts after the
/// prefix without beginning with it, every term does, and none can begin with
/// it either.
fn nothing_begins_with(boundaries: &[Vec<u8>], prefix: &[u8]) -> bool {
    match boundaries.first() {
        None => true,
        Some(first) => prefix < first.as_slice() && !first.starts_with(prefix),
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
