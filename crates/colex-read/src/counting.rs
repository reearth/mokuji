use std::cell::Cell;

use colex_format::Result;

use crate::RangeReader;

/// What a [`CountingReader`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadStats {
    /// Number of successful range reads.
    pub reads: u32,
    /// Total bytes returned.
    pub bytes: u64,
}

/// Wraps a [`RangeReader`] and records how much I/O passed through it.
///
/// The I/O bound is the contract COLex adds to a field that otherwise has
/// none, so it is checked mechanically rather than reasoned about. Conformance
/// cases assert the read count alongside the query result; a change that
/// silently adds a fetch fails the same way a wrong answer does.
///
/// Only successful reads are counted. A failed read fetched nothing the caller
/// can use, and counting it would let an implementation hide requests behind
/// deliberate failures.
#[derive(Debug)]
pub struct CountingReader<R> {
    inner: R,
    stats: Cell<ReadStats>,
}

impl<R> CountingReader<R> {
    /// Wraps `inner`, starting from zero.
    pub fn new(inner: R) -> Self {
        CountingReader {
            inner,
            stats: Cell::new(ReadStats::default()),
        }
    }

    /// What has been observed so far.
    pub fn stats(&self) -> ReadStats {
        self.stats.get()
    }

    /// Number of successful range reads so far.
    pub fn reads(&self) -> u32 {
        self.stats.get().reads
    }

    /// Resets the counters.
    ///
    /// Used to measure a query separately from the cold start that populated
    /// the resident set, which `SPEC.md` §4 accounts for separately.
    pub fn reset(&self) {
        self.stats.set(ReadStats::default());
    }

    /// Returns the wrapped reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: RangeReader> RangeReader for CountingReader<R> {
    fn file_len(&self) -> u64 {
        self.inner.file_len()
    }

    async fn read_range(&self, offset: u64, len: u32) -> Result<Vec<u8>> {
        let bytes = self.inner.read_range(offset, len).await?;
        let mut stats = self.stats.get();
        stats.reads += 1;
        stats.bytes += bytes.len() as u64;
        self.stats.set(stats);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block_on::block_on, SliceReader};

    fn reader() -> CountingReader<SliceReader> {
        CountingReader::new(SliceReader::new(b"0123456789".to_vec()))
    }

    #[test]
    fn counts_reads_and_bytes() {
        let r = reader();
        block_on(r.read_range(0, 4)).unwrap();
        block_on(r.read_range(4, 2)).unwrap();
        assert_eq!(r.stats(), ReadStats { reads: 2, bytes: 6 });
    }

    #[test]
    fn does_not_count_failed_reads() {
        let r = reader();
        assert!(block_on(r.read_range(9, 4)).is_err());
        assert_eq!(r.stats(), ReadStats::default());
    }

    #[test]
    fn reset_clears_the_counters() {
        let r = reader();
        block_on(r.read_range(0, 4)).unwrap();
        r.reset();
        assert_eq!(r.reads(), 0);
    }

    #[test]
    fn reports_the_wrapped_file_length() {
        assert_eq!(reader().file_len(), 10);
    }
}
