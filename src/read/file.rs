use std::fs::File;
use std::path::Path;

use crate::format::Result;

use crate::read::reader::{check_range, BlockingRangeReader, RangeReader};

/// A [`RangeReader`] over a file on disk, read a range at a time.
///
/// Each read is a positional read of exactly the bytes asked for, so a file of
/// gigabytes costs what a query touches and is never loaded whole: the same
/// bound the format states for a file behind HTTP, held on a local disk.
#[derive(Debug)]
pub struct FileReader {
    file: File,
    len: u64,
}

impl FileReader {
    /// Opens the file at `path`.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileReader { file, len })
    }
}

impl BlockingRangeReader for FileReader {
    fn file_len(&self) -> u64 {
        self.len
    }

    fn read_range(&self, offset: u64, len: u32) -> Result<Vec<u8>> {
        check_range(offset, len, self.len)?;
        let mut buf = vec![0; len as usize];
        read_exact_at(&self.file, &mut buf, offset)?;
        Ok(buf)
    }
}

impl RangeReader for FileReader {
    fn file_len(&self) -> u64 {
        self.len
    }

    async fn read_range(&self, offset: u64, len: u32) -> Result<Vec<u8>> {
        BlockingRangeReader::read_range(self, offset, len)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

// Windows has no read_exact_at; seek_read may return short, so loop.
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FileReader;
    use crate::format::Error;
    use crate::read::block_on::block_on;
    use crate::read::RangeReader;
    use std::io::Write;

    fn temp(bytes: &[u8]) -> (std::path::PathBuf, FileReader) {
        let path = std::env::temp_dir().join(format!("mokuji-file-reader-{}", std::process::id()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let reader = FileReader::open(&path).unwrap();
        (path, reader)
    }

    #[test]
    fn reads_a_range_and_nothing_past_the_end() {
        let (path, r) = temp(b"0123456789");
        assert_eq!(r.file_len(), 10);
        assert_eq!(block_on(r.read_range(2, 3)).unwrap(), b"234");
        assert_eq!(block_on(r.read_range(7, 3)).unwrap(), b"789");
        assert!(matches!(
            block_on(r.read_range(8, 3)),
            Err(Error::OutOfBounds { .. })
        ));
        std::fs::remove_file(path).unwrap();
    }
}
