use mokuji_codec::{bitpack, block, front_coded, varint};
use mokuji_format::{
    zigzag, Error, Flags, Header, PayloadCodec, Result, SectionCodec, SectionEntry, SectionId,
    HEADER_LEN, SECTION_ENTRY_LEN, SPEC_MAJOR, SPEC_MINOR,
};

/// Build parameters.
///
/// These belong in the metadata section's `generator.params`, since `SPEC.md`
/// §3.4 requires that the parameters be sufficient to reproduce the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Terms per dictionary block.
    ///
    /// Trades bytes per lookup against the size of the dictionary, and both
    /// against the size of the resident term index. Larger blocks give the
    /// compressor more to work with and fewer boundaries to record: measured on
    /// 1,744,962 Tokyo addresses, 128 terms a block costs 1.58 bytes a term and
    /// 435 bytes a query, while 1024 costs 0.50 and 725. Bigger blocks also
    /// *reduce* the read count for a query with many results, since more of
    /// them fall in one block.
    pub terms_per_block: usize,
    /// Documents per payload block.
    ///
    /// Larger blocks let more results share a read and make each read bigger.
    pub docs_per_payload_block: u32,
    /// Whether to store term frequencies.
    ///
    /// An address dictionary, where a term occurs at most once per document,
    /// omits them rather than storing a run of ones.
    pub has_freqs: bool,
    /// How payload is written down.
    ///
    /// Not what it means — see `SPEC.md` §5.5. The format states the encoding
    /// and has no opinion about the subject.
    pub payload: PayloadEncoding,
    /// Whether to store where each term occurs in each document.
    ///
    /// What a phrase query needs and nothing else does. The section is larger
    /// than the rest of the file put together, and it costs bytes rather than
    /// reads: a query that tests no phrase never touches it.
    pub positions: bool,
    /// Whether to store the length in tokens of every document.
    ///
    /// What BM25 needs and the header cannot give: the header carries the
    /// document count and the token total, which is enough for the mean but
    /// not for the per-document normalization. Costs about two bytes a
    /// document and is worth nothing to an index whose documents are all one
    /// term long.
    pub doc_lengths: bool,
    /// Whether to compress the sections that can be compressed.
    ///
    /// Each addressed unit is compressed on its own, so a reader still fetches
    /// one block and decompresses one block. A section whose units do not
    /// shrink is stored raw instead, which is decided per section rather than
    /// per block because the codec is recorded per section.
    pub compress: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            terms_per_block: 1024,
            docs_per_payload_block: 64,
            has_freqs: true,
            payload: PayloadEncoding::Opaque,
            positions: false,
            doc_lengths: false,
            compress: true,
        }
    }
}

/// One column of numbers, one value per document (`SPEC.md` §5.8).
///
/// Payload that can be read in bulk and read early: the few numbers a reader
/// needs about many documents before it decides whose payload to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// What the column is called. Meaning is the application's business.
    pub name: String,
    /// A stored `v` stands for `v * 10^exponent`. Recorded, not interpreted.
    pub scale_exponent: i8,
    /// One value per document, in document ID order.
    pub values: Vec<i64>,
}

/// How payload is written down (`SPEC.md` §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadEncoding {
    /// Bytes, stored as given.
    Opaque,
    /// A fixed number of signed integers per document, delta-coded.
    ///
    /// Worth taking whenever documents that sort together hold similar numbers,
    /// which for a hierarchical key they do: addresses that sort together are
    /// near each other on the ground.
    Ints {
        /// How many integers each document carries.
        values_per_doc: u8,
        /// A stored `v` stands for `v * 10^exponent`. Recorded, not
        /// interpreted.
        scale_exponent: i8,
    },
}

/// Builds a Mokuji file from documents and a sorted stream of postings.
///
/// Documents are added first, in document ID order; postings are then pushed in
/// term order, and within a term in document order. Both orderings are checked
/// rather than assumed, since a stream that is merely nearly sorted would
/// produce a file that is subtly wrong rather than obviously broken.
#[derive(Debug)]
pub struct Builder {
    options: Options,
    payloads: Vec<Vec<u8>>,
    values: Vec<Vec<i64>>,
    columns: Vec<Column>,
    sum_doclen: u64,
    doc_lengths: Vec<u64>,
    terms: Vec<Term>,
    open: Option<OpenTerm>,
}

#[derive(Debug)]
struct Term {
    term: Vec<u8>,
    doc_freq: u64,
    run: Vec<u8>,
    positions: Vec<u8>,
}

#[derive(Debug)]
struct OpenTerm {
    term: Vec<u8>,
    docs: Vec<u64>,
    freqs: Vec<u64>,
    positions: Vec<Vec<u64>>,
}

impl Builder {
    /// Starts a build.
    pub fn new(options: Options) -> Result<Self> {
        if options.terms_per_block == 0 || options.docs_per_payload_block == 0 {
            return Err(Error::Malformed {
                detail: "block sizes must be non-zero",
            });
        }
        // A positions run says nothing about where one document's positions end
        // and the next begin; the frequency is that boundary.
        if options.positions && !options.has_freqs {
            return Err(Error::Malformed {
                detail: "positions need frequencies, which say how many belong to each document",
            });
        }
        Ok(Builder {
            options,
            payloads: Vec::new(),
            values: Vec::new(),
            columns: Vec::new(),
            sum_doclen: 0,
            doc_lengths: Vec::new(),
            terms: Vec::new(),
            open: None,
        })
    }

    /// Appends a document whose payload is opaque bytes, returning its ID.
    pub fn add_document(&mut self, payload: impl Into<Vec<u8>>) -> Result<u64> {
        if self.options.payload != PayloadEncoding::Opaque {
            return Err(Error::Malformed {
                detail: "this index encodes payload as integers",
            });
        }
        self.payloads.push(payload.into());
        Ok(self.documents() - 1)
    }

    /// Appends a document whose payload is a fixed row of integers.
    pub fn add_document_values(&mut self, values: &[i64]) -> Result<u64> {
        let PayloadEncoding::Ints { values_per_doc, .. } = self.options.payload else {
            return Err(Error::Malformed {
                detail: "this index encodes payload as opaque bytes",
            });
        };
        if values.len() != usize::from(values_per_doc) {
            return Err(Error::Malformed {
                detail: "a document carries the wrong number of values",
            });
        }
        self.values.push(values.to_vec());
        Ok(self.documents() - 1)
    }

    /// How many documents have been added.
    pub fn documents(&self) -> u64 {
        match self.options.payload {
            PayloadEncoding::Opaque => self.payloads.len() as u64,
            PayloadEncoding::Ints { .. } => self.values.len() as u64,
        }
    }

    /// Adds a column of per-document numbers.
    ///
    /// Checked at [`Builder::finish`] against the document count, since the
    /// documents may not all have been added yet.
    pub fn add_column(&mut self, column: Column) -> Result<()> {
        if self.columns.iter().any(|c| c.name == column.name) {
            return Err(Error::Malformed {
                detail: "two document attribute columns share a name",
            });
        }
        self.columns.push(column);
        Ok(())
    }

    /// Records that `term` occurs `freq` times in `doc`.
    ///
    /// Terms must be pushed in ascending order and, within a term, documents
    /// must ascend too.
    pub fn add_posting(&mut self, term: &[u8], doc: u64, freq: u64) -> Result<()> {
        if freq == 0 {
            return Err(Error::Malformed {
                detail: "a posting with zero frequency is not a posting",
            });
        }
        if doc >= self.documents() {
            return Err(Error::Malformed {
                detail: "posting refers to a document that was never added",
            });
        }

        match &mut self.open {
            Some(open) if open.term == term => {
                if doc <= *open.docs.last().expect("a term is opened with a document") {
                    return Err(Error::Malformed {
                        detail: "documents within a term are not ascending",
                    });
                }
                open.docs.push(doc);
                open.freqs.push(freq);
                open.positions.push(Vec::new());
            }
            Some(open) if term <= open.term.as_slice() => {
                return Err(Error::Malformed {
                    detail: "terms are not ascending",
                });
            }
            _ => {
                self.close_term();
                self.open = Some(OpenTerm {
                    term: term.to_vec(),
                    docs: vec![doc],
                    freqs: vec![freq],
                    positions: vec![Vec::new()],
                });
            }
        }
        self.sum_doclen += freq;
        if self.options.doc_lengths {
            let doc = doc as usize;
            if self.doc_lengths.len() <= doc {
                self.doc_lengths.resize(doc + 1, 0);
            }
            self.doc_lengths[doc] += freq;
        }
        Ok(())
    }

    /// Records that `term` occurs in `doc` at exactly these positions.
    ///
    /// The frequency is the number of positions, so this is `add_posting` with
    /// the detail a phrase query needs. Positions must ascend.
    pub fn add_posting_at(&mut self, term: &[u8], doc: u64, positions: &[u64]) -> Result<()> {
        if positions.is_empty() {
            return Err(Error::Malformed {
                detail: "a posting with no positions is not a posting",
            });
        }
        if positions.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Error::Malformed {
                detail: "positions within a document are not ascending",
            });
        }
        self.add_posting(term, doc, positions.len() as u64)?;
        let open = self.open.as_mut().expect("add_posting opened a term");
        *open.positions.last_mut().expect("one per document") = positions.to_vec();
        Ok(())
    }

    /// Finishes the file, with `meta` as the metadata section.
    ///
    /// `meta` is taken as encoded canonical CBOR rather than built here: it
    /// carries the provenance of a source the writer never sees, so composing
    /// it is the caller's job.
    pub fn finish(mut self, meta: &[u8]) -> Result<Vec<u8>> {
        self.close_term();
        if self.terms.is_empty() {
            return Err(Error::Malformed {
                detail: "an index with no terms cannot be built",
            });
        }

        let term_chunks: Vec<&[Term]> = self.terms.chunks(self.options.terms_per_block).collect();
        let per_block = self.options.docs_per_payload_block as usize;
        let raw_payload_blocks: Vec<Vec<u8>> = match self.options.payload {
            PayloadEncoding::Opaque => self
                .payloads
                .chunks(per_block)
                .map(|group| {
                    let mut block = Vec::new();
                    varint::encode(group.len() as u64, &mut block);
                    for payload in group {
                        varint::encode(payload.len() as u64, &mut block);
                        block.extend_from_slice(payload);
                    }
                    block
                })
                .collect(),
            PayloadEncoding::Ints { .. } => self
                .values
                .chunks(per_block)
                .map(|group| {
                    let mut block = Vec::new();
                    varint::encode(group.len() as u64, &mut block);
                    // Deltas never cross a block boundary: a block is addressed
                    // on its own and must decode on its own.
                    let mut previous: Option<&Vec<i64>> = None;
                    for row in group {
                        for (i, &value) in row.iter().enumerate() {
                            let delta = match previous {
                                None => value,
                                Some(previous) => value.wrapping_sub(previous[i]),
                            };
                            varint::encode(zigzag(delta), &mut block);
                        }
                        previous = Some(row);
                    }
                    block
                })
                .collect(),
        };
        let (payload_blocks, payload_codec) = self.pack(raw_payload_blocks)?;

        let mut term_index = (term_chunks.len() as u32).to_le_bytes().to_vec();
        let mut previous: &[u8] = b"";
        for chunk in &term_chunks {
            let first = chunk[0].term.as_slice();
            front_coded::encode(first, previous, &mut term_index);
            previous = first;
        }
        // The term index depends on nothing but the terms, so compressing it
        // cannot disturb the layout.
        let (term_index, term_index_codec) = self.pack_one(term_index)?;

        // Postings are placed before the term blocks that point into them, so
        // that a postings offset is known before any term block is built. With
        // the opposite order each depends on the size of the other, and once
        // the blocks are compressed that dependency has no fixed point to find:
        // a changed offset changes a varint, which changes what deflate emits,
        // which moves the offset again. Ordering the file to make it writable
        // in one pass is cheaper than iterating toward a layout that may not
        // settle.
        // Lengths are read whole or not at all — scoring touches many
        // documents at once — so the section sits with the resident ones.
        let doc_lengths = if self.options.doc_lengths {
            let mut lengths = self.doc_lengths.clone();
            lengths.resize(self.documents() as usize, 0);
            let mut out = Vec::new();
            bitpack::encode(&lengths, &mut out);
            Some(out)
        } else {
            None
        };

        let attributes = if self.columns.is_empty() {
            None
        } else {
            let mut out = Vec::new();
            varint::encode(self.columns.len() as u64, &mut out);
            for column in &self.columns {
                if column.values.len() as u64 != self.documents() {
                    return Err(Error::Malformed {
                        detail: "a document attribute column does not have one value per document",
                    });
                }
                varint::encode(column.name.len() as u64, &mut out);
                out.extend_from_slice(column.name.as_bytes());
                out.push(column.scale_exponent as u8);
            }
            for column in &self.columns {
                let zigzagged: Vec<u64> = column.values.iter().map(|&v| zigzag(v)).collect();
                bitpack::encode(&zigzagged, &mut out);
            }
            Some(out)
        };

        let mut order = vec![
            SectionId::META,
            SectionId::TERM_INDEX,
            SectionId::TERM_BLOCK_OFFSETS,
            SectionId::PAYLOAD_OFFSETS,
        ];
        if doc_lengths.is_some() {
            order.push(SectionId::DOC_LENGTHS);
        }
        if attributes.is_some() {
            order.push(SectionId::DOC_ATTRIBUTES);
        }
        order.push(SectionId::POSTINGS);
        if self.options.positions {
            // Before the term blocks that point into it, for the same reason
            // the postings are: so that an offset is known before any entry is
            // written and the file can be built in one pass.
            order.push(SectionId::POSITIONS);
        }
        order.extend([SectionId::TERM_BLOCKS, SectionId::PAYLOAD]);
        let table_len = SECTION_ENTRY_LEN * order.len();

        let postings_len: u64 = self.terms.iter().map(|t| t.run.len() as u64).sum();
        let positions_len: u64 = self.terms.iter().map(|t| t.positions.len() as u64).sum();
        let postings_at = (HEADER_LEN + table_len) as u64
            + meta.len() as u64
            + term_index.len() as u64
            + 8 * (term_chunks.len() as u64 + 1)
            + 12
            + 8 * (payload_blocks.len() as u64 + 1)
            + doc_lengths.as_ref().map_or(0, |d| d.len() as u64)
            + attributes.as_ref().map_or(0, |a| a.len() as u64);

        let positions_at = postings_at + postings_len;
        let (blocks, term_blocks_codec) =
            self.pack(self.term_blocks(&term_chunks, postings_at, positions_at))?;

        let mut placed = Vec::new();
        let mut cursor = (HEADER_LEN + table_len) as u64;
        for id in order.iter().copied() {
            let length = match id {
                SectionId::META => meta.len() as u64,
                SectionId::TERM_INDEX => term_index.len() as u64,
                SectionId::TERM_BLOCK_OFFSETS => 8 * (term_chunks.len() as u64 + 1),
                SectionId::PAYLOAD_OFFSETS => 12 + 8 * (payload_blocks.len() as u64 + 1),
                SectionId::DOC_LENGTHS => {
                    doc_lengths.as_ref().expect("present when ordered").len() as u64
                }
                SectionId::DOC_ATTRIBUTES => {
                    attributes.as_ref().expect("present when ordered").len() as u64
                }
                SectionId::POSTINGS => postings_len,
                SectionId::POSITIONS => positions_len,
                SectionId::TERM_BLOCKS => blocks.iter().map(|b| b.len() as u64).sum(),
                SectionId::PAYLOAD => payload_blocks.iter().map(|b| b.len() as u64).sum(),
                _ => unreachable!("only the sections in `order` are sized"),
            };
            placed.push((id, cursor, length));
            cursor += length;
        }

        let at = |id: SectionId| {
            placed
                .iter()
                .find(|(other, _, _)| *other == id)
                .map(|(_, offset, len)| (*offset, *len))
                .expect("every section in `order` is placed")
        };
        debug_assert_eq!(at(SectionId::POSTINGS).0, postings_at);

        let term_block_offsets = offset_table(at(SectionId::TERM_BLOCKS).0, &blocks);
        let (codec, values_per_doc, scale_exponent) = match self.options.payload {
            PayloadEncoding::Opaque => (PayloadCodec::Opaque, 0u8, 0i8),
            PayloadEncoding::Ints {
                values_per_doc,
                scale_exponent,
            } => (PayloadCodec::DeltaInts, values_per_doc, scale_exponent),
        };
        let mut payload_offsets = vec![codec as u8, values_per_doc, scale_exponent as u8, 0];
        payload_offsets.extend_from_slice(&self.options.docs_per_payload_block.to_le_bytes());
        payload_offsets.extend_from_slice(&(payload_blocks.len() as u32).to_le_bytes());
        payload_offsets.extend_from_slice(&offset_table(at(SectionId::PAYLOAD).0, &payload_blocks));

        let mut flags = Flags::HAS_PAYLOAD;
        if self.options.has_freqs {
            flags = flags.with(Flags::HAS_FREQS);
        }
        if self.options.positions {
            flags = flags.with(Flags::HAS_POSITIONS);
        }
        let header = Header {
            spec_major: SPEC_MAJOR,
            spec_minor: SPEC_MINOR,
            flags,
            section_count: order.len() as u32,
            num_docs: self.documents(),
            sum_doclen: self.sum_doclen,
            section_table_offset: HEADER_LEN as u64,
            section_table_len: table_len as u32,
        };

        let mut out = header.encode().to_vec();
        // The table is sorted by identifier, which is not the order the
        // sections are written in: the resident ones go first so that a cold
        // start can fetch them in one read (SPEC.md §4, §5.2).
        let codec_of = |id: SectionId| match id {
            SectionId::TERM_INDEX => term_index_codec,
            SectionId::TERM_BLOCKS => term_blocks_codec,
            SectionId::PAYLOAD => payload_codec,
            // Metadata stays readable without a decompressor, which is worth
            // more than the bytes: it is the provenance record, and tooling
            // that only wants to know where a file came from should not need
            // to implement the format to find out.
            //
            // The offset tables stay raw because their size feeds back into
            // the placement they describe. Compressing them is possible and
            // worthwhile, and wants a settled layout to be built on.
            _ => SectionCodec::Raw,
        };
        let mut sorted = placed.clone();
        sorted.sort_by_key(|(id, _, _)| *id);
        for (id, offset, length) in &sorted {
            out.extend_from_slice(
                &SectionEntry {
                    id: *id,
                    codec: codec_of(*id),
                    offset: *offset,
                    length: *length,
                }
                .encode(),
            );
        }

        for id in order.iter().copied() {
            debug_assert_eq!(out.len() as u64, at(id).0);
            match id {
                SectionId::META => out.extend_from_slice(meta),
                SectionId::TERM_INDEX => out.extend_from_slice(&term_index),
                SectionId::TERM_BLOCK_OFFSETS => out.extend_from_slice(&term_block_offsets),
                SectionId::PAYLOAD_OFFSETS => out.extend_from_slice(&payload_offsets),
                SectionId::DOC_LENGTHS => {
                    out.extend_from_slice(doc_lengths.as_ref().expect("present when ordered"))
                }
                SectionId::DOC_ATTRIBUTES => {
                    out.extend_from_slice(attributes.as_ref().expect("present when ordered"))
                }
                SectionId::TERM_BLOCKS => blocks.iter().for_each(|b| out.extend_from_slice(b)),
                SectionId::POSTINGS => self
                    .terms
                    .iter()
                    .for_each(|t| out.extend_from_slice(&t.run)),
                SectionId::POSITIONS => self
                    .terms
                    .iter()
                    .for_each(|t| out.extend_from_slice(&t.positions)),
                SectionId::PAYLOAD => payload_blocks.iter().for_each(|b| out.extend_from_slice(b)),
                _ => unreachable!("only the sections in `order` are written"),
            }
        }
        Ok(out)
    }

    /// Compresses each unit, or leaves them all raw if that is no smaller.
    ///
    /// Decided per section, since the codec is recorded per section: a file
    /// cannot say "these blocks are deflated and those are not".
    fn pack(&self, units: Vec<Vec<u8>>) -> Result<(Vec<Vec<u8>>, SectionCodec)> {
        if !self.options.compress {
            return Ok((units, SectionCodec::Raw));
        }
        let raw: usize = units.iter().map(Vec::len).sum();
        let packed = units
            .iter()
            .map(|unit| block::compress(unit))
            .collect::<Result<Vec<_>>>()?;
        if packed.iter().map(Vec::len).sum::<usize>() < raw {
            Ok((packed, SectionCodec::Deflate))
        } else {
            Ok((units, SectionCodec::Raw))
        }
    }

    fn pack_one(&self, unit: Vec<u8>) -> Result<(Vec<u8>, SectionCodec)> {
        let (mut units, codec) = self.pack(vec![unit])?;
        Ok((units.remove(0), codec))
    }

    fn term_blocks(
        &self,
        chunks: &[&[Term]],
        postings_base: u64,
        positions_base: u64,
    ) -> Vec<Vec<u8>> {
        let mut blocks = Vec::with_capacity(chunks.len());
        let mut term_id = 0u64;
        let mut postings_at = postings_base;
        let mut positions_at = positions_base;
        for chunk in chunks {
            let mut block = Vec::new();
            varint::encode(chunk.len() as u64, &mut block);
            varint::encode(term_id, &mut block);

            let mut previous_key: &[u8] = b"";
            let mut previous_offset: Option<u64> = None;
            let mut previous_positions: Option<u64> = None;
            for term in chunk.iter() {
                front_coded::encode(&term.term, previous_key, &mut block);
                varint::encode(term.doc_freq, &mut block);
                varint::encode(
                    match previous_offset {
                        None => postings_at,
                        Some(previous) => postings_at - previous,
                    },
                    &mut block,
                );
                varint::encode(term.run.len() as u64, &mut block);
                if self.options.positions {
                    varint::encode(
                        match previous_positions {
                            None => positions_at,
                            Some(previous) => positions_at - previous,
                        },
                        &mut block,
                    );
                    varint::encode(term.positions.len() as u64, &mut block);
                    previous_positions = Some(positions_at);
                    positions_at += term.positions.len() as u64;
                }
                previous_key = &term.term;
                previous_offset = Some(postings_at);
                postings_at += term.run.len() as u64;
            }
            term_id += chunk.len() as u64;
            blocks.push(block);
        }
        blocks
    }

    fn close_term(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };

        // Positions, in the same document order the postings run uses.
        let mut positions = Vec::new();
        if self.options.positions {
            for (row, &freq) in open.positions.iter().zip(&open.freqs) {
                let mut values = Vec::with_capacity(row.len());
                let mut previous: Option<u64> = None;
                for &at in row {
                    values.push(match previous {
                        None => at,
                        Some(previous) => at - previous - 1,
                    });
                    previous = Some(at);
                }
                // A posting recorded without positions still needs its share of
                // the run, or the frequencies stop lining up.
                values.resize(freq as usize, 0);
                bitpack::encode(&values, &mut positions);
            }
        }

        // The dictionary can state this term's postings by itself: it names one
        // document, that document's ID is the ID this term is about to be
        // given, and it occurs there once (SPEC.md §5.3.4). Writing a run would
        // record nothing and cost a range read to read back.
        let term_id = self.terms.len() as u64;
        if open.docs == [term_id] && open.freqs == [1] {
            self.terms.push(Term {
                term: open.term,
                doc_freq: 1,
                run: Vec::new(),
                positions,
            });
            return;
        }

        let mut run = vec![0u8]; // codec 0
        varint::encode(open.docs.len() as u64, &mut run);

        // The first value is a document ID and the rest are gaps less one, so
        // that consecutive documents become zeros and pack to nothing.
        let mut values = Vec::with_capacity(open.docs.len());
        let mut previous: Option<u64> = None;
        for &doc in &open.docs {
            values.push(match previous {
                None => doc,
                Some(previous) => doc - previous - 1,
            });
            previous = Some(doc);
        }
        bitpack::encode(&values, &mut run);

        if self.options.has_freqs {
            let freqs: Vec<u64> = open.freqs.iter().map(|f| f - 1).collect();
            bitpack::encode(&freqs, &mut run);
        }

        self.terms.push(Term {
            term: open.term,
            doc_freq: open.docs.len() as u64,
            run,
            positions,
        });
    }
}

fn offset_table(base: u64, blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 * (blocks.len() + 1));
    let mut cursor = base;
    for block in blocks {
        out.extend_from_slice(&cursor.to_le_bytes());
        cursor += block.len() as u64;
    }
    out.extend_from_slice(&cursor.to_le_bytes());
    out
}
