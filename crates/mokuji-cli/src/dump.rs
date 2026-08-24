//! `mokuji dump` - write an index back out as the corpus it indexes.
//!
//! The inverse of `build`, near enough: term, tab, payload, in term order. It
//! exists because an index is otherwise only inspectable through queries, and
//! there are things — measuring an encoding, diffing two generations, checking
//! a conversion — that want the whole of it.
//!
//! Deliberately unbounded, like `verify`. It reads every block.

use std::io::{BufWriter, Write};
use std::path::Path;

use mokuji_read::{block_on::block_on, term, Index, RangeReader, SliceReader};

use crate::info::Error;

/// Writes the index at `path` out as a corpus.
pub fn run(path: &Path) -> Result<(), Error> {
    let reader = SliceReader::new(std::fs::read(path)?);
    let index = block_on(Index::open(reader))?;
    let out = std::io::stdout();
    let mut out = BufWriter::new(out.lock());
    block_on(dump(&index, &mut out))
}

async fn dump<R: RangeReader, W: Write>(index: &Index<R>, out: &mut W) -> Result<(), Error> {
    let blocks = index.term_index()?.block_count();
    for ordinal in 0..blocks {
        let block = index.read_term_block(ordinal).await?;
        let entries = term::decode_block(&block)?;

        // A block at a time, so that the postings and payload of terms that sit
        // together are fetched together.
        let terms: Vec<Vec<u8>> = entries.iter().map(|(term, _)| term.clone()).collect();
        let located: Vec<_> = entries.into_iter().map(|(_, entry)| entry).collect();
        let postings = index.postings_span(&located).await?;

        let docs: Vec<u64> = postings
            .iter()
            .filter_map(|p| p.docs.first().copied())
            .collect();
        let payloads = index.payload_span(&docs).await?;

        for (term, payload) in terms.iter().zip(&payloads) {
            out.write_all(term)?;
            out.write_all(b"\t")?;
            out.write_all(crate::payload::render(payload).as_bytes())?;
            out.write_all(b"\n")?;
        }
    }
    out.flush()?;
    Ok(())
}
