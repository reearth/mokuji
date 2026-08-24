use std::future::Future;

use colex_format::{Error, Result};

/// A source of arbitrary byte ranges.
///
/// Implementations are expected to perform exactly one underlying fetch per
/// call. That is what makes counting calls a meaningful proxy for counting
/// requests, and therefore what makes the bound in `SPEC.md` §4 testable.
///
/// A reader may coalesce ranges *before* calling this trait, which lowers the
/// count. It must not split one logical range into several calls, which would
/// hide requests from the count.
pub trait RangeReader {
    /// Total size of the file.
    ///
    /// Required so that offsets taken from the file, which are untrusted, can
    /// be bounds-checked before use (`SPEC.md` §8).
    fn file_len(&self) -> u64;

    /// Returns `len` bytes starting at `offset`.
    ///
    /// Returns [`Error::OutOfBounds`] if the range extends past the end of the
    /// file. A short read is an error, not a truncated success: partial data
    /// would be indistinguishable from a corrupt file.
    fn read_range(&self, offset: u64, len: u32) -> impl Future<Output = Result<Vec<u8>>>;
}

/// Lets a shared reference stand in for a reader.
///
/// Opening an index consumes the reader, so this is what allows a
/// [`CountingReader`](crate::CountingReader) to be kept in hand and questioned
/// about what a query cost.
impl<R: RangeReader> RangeReader for &R {
    fn file_len(&self) -> u64 {
        R::file_len(self)
    }

    fn read_range(&self, offset: u64, len: u32) -> impl Future<Output = Result<Vec<u8>>> {
        R::read_range(self, offset, len)
    }
}

/// Checks that `offset..offset + len` lies within a file of `file_len` bytes.
///
/// Shared by implementations so that they reject the same ranges.
pub fn check_range(offset: u64, len: u32, file_len: u64) -> Result<()> {
    let len = u64::from(len);
    let end = offset.checked_add(len).ok_or(Error::OutOfBounds {
        offset,
        len,
        file_len,
    })?;
    if end > file_len {
        return Err(Error::OutOfBounds {
            offset,
            len,
            file_len,
        });
    }
    Ok(())
}
