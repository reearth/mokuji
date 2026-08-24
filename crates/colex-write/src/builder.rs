use colex_codec::{bitpack, front_coded, varint};
use colex_format::{
    Error, Flags, Header, Result, SectionCodec, SectionEntry, SectionId, HEADER_LEN,
    SECTION_ENTRY_LEN, SPEC_MAJOR, SPEC_MINOR,
};

/// Build parameters.
///
/// These belong in the metadata section's `generator.params`, since `SPEC.md`
/// §3.4 requires that the parameters be sufficient to reproduce the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Terms per dictionary block.
    ///
    /// Trades the size of the resident term index against the cost of scanning
    /// one block. Both are reader-visible, so it is recorded rather than
    /// chosen by the reader.
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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            terms_per_block: 128,
            docs_per_payload_block: 64,
            has_freqs: true,
        }
    }
}

/// Builds a COLex file from documents and a sorted stream of postings.
///
/// Documents are added first, in document ID order; postings are then pushed in
/// term order, and within a term in document order. Both orderings are checked
/// rather than assumed, since a stream that is merely nearly sorted would
/// produce a file that is subtly wrong rather than obviously broken.
#[derive(Debug)]
pub struct Builder {
    options: Options,
    payloads: Vec<Vec<u8>>,
    sum_doclen: u64,
    terms: Vec<Term>,
    open: Option<OpenTerm>,
}

#[derive(Debug)]
struct Term {
    term: Vec<u8>,
    doc_freq: u64,
    run: Vec<u8>,
}

#[derive(Debug)]
struct OpenTerm {
    term: Vec<u8>,
    docs: Vec<u64>,
    freqs: Vec<u64>,
}

impl Builder {
    /// Starts a build.
    pub fn new(options: Options) -> Result<Self> {
        if options.terms_per_block == 0 || options.docs_per_payload_block == 0 {
            return Err(Error::Malformed {
                detail: "block sizes must be non-zero",
            });
        }
        Ok(Builder {
            options,
            payloads: Vec::new(),
            sum_doclen: 0,
            terms: Vec::new(),
            open: None,
        })
    }

    /// Appends a document, returning its ID.
    ///
    /// Payload bytes are opaque to the format (`SPEC.md` §5.5).
    pub fn add_document(&mut self, payload: impl Into<Vec<u8>>) -> u64 {
        self.payloads.push(payload.into());
        self.payloads.len() as u64 - 1
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
        if doc >= self.payloads.len() as u64 {
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
                });
            }
        }
        self.sum_doclen += freq;
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
        let payload_blocks: Vec<Vec<u8>> = self
            .payloads
            .chunks(self.options.docs_per_payload_block as usize)
            .map(|group| {
                let mut block = Vec::new();
                varint::encode(group.len() as u64, &mut block);
                for payload in group {
                    varint::encode(payload.len() as u64, &mut block);
                    block.extend_from_slice(payload);
                }
                block
            })
            .collect();

        let mut term_index = (term_chunks.len() as u32).to_le_bytes().to_vec();
        let mut previous: &[u8] = b"";
        for chunk in &term_chunks {
            let first = chunk[0].term.as_slice();
            front_coded::encode(first, previous, &mut term_index);
            previous = first;
        }

        // A term block entry carries an absolute postings offset, and the
        // postings section sits after the term blocks, so each size depends on
        // the other. Both shrink as the guess improves, so iterating reaches a
        // fixed point; eight rounds is far beyond what a varint width can need.
        let order = [
            SectionId::META,
            SectionId::TERM_INDEX,
            SectionId::TERM_BLOCK_OFFSETS,
            SectionId::PAYLOAD_OFFSETS,
            SectionId::TERM_BLOCKS,
            SectionId::POSTINGS,
            SectionId::PAYLOAD,
        ];
        let table_len = SECTION_ENTRY_LEN * order.len();
        let head = (HEADER_LEN + table_len) as u64;

        let mut postings_base = 0u64;
        let (blocks, placed) = loop {
            let blocks = self.term_blocks(&term_chunks, postings_base);
            let sizes = |id: SectionId| -> u64 {
                match id {
                    SectionId::META => meta.len() as u64,
                    SectionId::TERM_INDEX => term_index.len() as u64,
                    SectionId::TERM_BLOCK_OFFSETS => 8 * (term_chunks.len() as u64 + 1),
                    SectionId::PAYLOAD_OFFSETS => 8 + 8 * (payload_blocks.len() as u64 + 1),
                    SectionId::TERM_BLOCKS => blocks.iter().map(|b| b.len() as u64).sum(),
                    SectionId::POSTINGS => self.terms.iter().map(|t| t.run.len() as u64).sum(),
                    SectionId::PAYLOAD => payload_blocks.iter().map(|b| b.len() as u64).sum(),
                    _ => unreachable!("only the sections in `order` are sized"),
                }
            };

            let mut placed = Vec::new();
            let mut cursor = head;
            for id in order {
                placed.push((id, cursor, sizes(id)));
                cursor += sizes(id);
            }
            let postings_at = placed
                .iter()
                .find(|(id, _, _)| *id == SectionId::POSTINGS)
                .map(|(_, offset, _)| *offset)
                .expect("postings is in `order`");
            if postings_at == postings_base {
                break (blocks, placed);
            }
            postings_base = postings_at;
        };

        let at = |id: SectionId| {
            placed
                .iter()
                .find(|(other, _, _)| *other == id)
                .map(|(_, offset, len)| (*offset, *len))
                .expect("every section in `order` is placed")
        };

        let term_block_offsets = offset_table(at(SectionId::TERM_BLOCKS).0, &blocks);
        let mut payload_offsets = self.options.docs_per_payload_block.to_le_bytes().to_vec();
        payload_offsets.extend_from_slice(&(payload_blocks.len() as u32).to_le_bytes());
        payload_offsets.extend_from_slice(&offset_table(at(SectionId::PAYLOAD).0, &payload_blocks));

        let mut flags = Flags::HAS_PAYLOAD;
        if self.options.has_freqs {
            flags = flags.with(Flags::HAS_FREQS);
        }
        let header = Header {
            spec_major: SPEC_MAJOR,
            spec_minor: SPEC_MINOR,
            flags,
            section_count: order.len() as u32,
            num_docs: self.payloads.len() as u64,
            sum_doclen: self.sum_doclen,
            section_table_offset: HEADER_LEN as u64,
            section_table_len: table_len as u32,
        };

        let mut out = header.encode().to_vec();
        // The table is sorted by identifier, which is not the order the
        // sections are written in: the resident ones go first so that a cold
        // start can fetch them in one read (SPEC.md §4, §5.2).
        let mut sorted = placed.clone();
        sorted.sort_by_key(|(id, _, _)| *id);
        for (id, offset, length) in &sorted {
            out.extend_from_slice(
                &SectionEntry {
                    id: *id,
                    codec: SectionCodec::Raw,
                    offset: *offset,
                    length: *length,
                }
                .encode(),
            );
        }

        for id in order {
            debug_assert_eq!(out.len() as u64, at(id).0);
            match id {
                SectionId::META => out.extend_from_slice(meta),
                SectionId::TERM_INDEX => out.extend_from_slice(&term_index),
                SectionId::TERM_BLOCK_OFFSETS => out.extend_from_slice(&term_block_offsets),
                SectionId::PAYLOAD_OFFSETS => out.extend_from_slice(&payload_offsets),
                SectionId::TERM_BLOCKS => blocks.iter().for_each(|b| out.extend_from_slice(b)),
                SectionId::POSTINGS => self
                    .terms
                    .iter()
                    .for_each(|t| out.extend_from_slice(&t.run)),
                SectionId::PAYLOAD => payload_blocks.iter().for_each(|b| out.extend_from_slice(b)),
                _ => unreachable!("only the sections in `order` are written"),
            }
        }
        Ok(out)
    }

    fn term_blocks(&self, chunks: &[&[Term]], postings_base: u64) -> Vec<Vec<u8>> {
        let mut blocks = Vec::with_capacity(chunks.len());
        let mut term_id = 0u64;
        let mut postings_at = postings_base;
        for chunk in chunks {
            let mut block = Vec::new();
            varint::encode(chunk.len() as u64, &mut block);
            varint::encode(term_id, &mut block);

            let mut previous_key: &[u8] = b"";
            let mut previous_offset: Option<u64> = None;
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
