use crate::format::Result;

use crate::read::reader::{check_range, RangeReader};

/// A [`RangeReader`] over bytes already in memory.
///
/// Used by the conformance suite and by `mokuji info`, where the file is small
/// enough to hold whole. It still goes through the range-read interface, so
/// the read counts it produces are the same ones a remote reader would.
#[derive(Debug, Clone)]
pub struct SliceReader {
    bytes: Vec<u8>,
}

impl SliceReader {
    /// Wraps the given bytes.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        SliceReader {
            bytes: bytes.into(),
        }
    }
}

impl RangeReader for SliceReader {
    fn file_len(&self) -> u64 {
        self.bytes.len() as u64
    }

    async fn read_range(&self, offset: u64, len: u32) -> Result<Vec<u8>> {
        check_range(offset, len, self.file_len())?;
        let start = offset as usize;
        Ok(self.bytes[start..start + len as usize].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Error;
    use crate::read::block_on::block_on;

    #[test]
    fn reads_a_range() {
        let r = SliceReader::new(b"0123456789".to_vec());
        assert_eq!(block_on(r.read_range(2, 3)).unwrap(), b"234");
    }

    #[test]
    fn reads_to_the_exact_end() {
        let r = SliceReader::new(b"0123456789".to_vec());
        assert_eq!(block_on(r.read_range(7, 3)).unwrap(), b"789");
    }

    #[test]
    fn reads_an_empty_range() {
        let r = SliceReader::new(b"0123456789".to_vec());
        assert_eq!(block_on(r.read_range(10, 0)).unwrap(), b"");
    }

    #[test]
    fn rejects_a_range_past_the_end() {
        let r = SliceReader::new(b"0123456789".to_vec());
        assert!(matches!(
            block_on(r.read_range(8, 3)),
            Err(Error::OutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_an_overflowing_range() {
        let r = SliceReader::new(b"0123456789".to_vec());
        assert!(matches!(
            block_on(r.read_range(u64::MAX, 1)),
            Err(Error::OutOfBounds { .. })
        ));
    }
}
