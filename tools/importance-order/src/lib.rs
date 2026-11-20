//! What the two measurements share: a file read in ranges, and the head of a
//! postings run decoded as far as it goes.

use std::fs::File;
use std::os::unix::fs::FileExt;

use mokuji::codec::{bitpack, varint};
use mokuji::format::{Error, Result};
use mokuji::read::RangeReader;

/// A local file, read one range at a time.
pub struct FileReader {
    file: File,
    len: u64,
}

impl FileReader {
    pub fn open(path: &str) -> Self {
        let file = File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let len = file.metadata().unwrap().len();
        FileReader { file, len }
    }

    fn read(&self, offset: u64, len: u32) -> Result<Vec<u8>> {
        if offset + u64::from(len) > self.len {
            return Err(Error::OutOfBounds {
                offset,
                len: u64::from(len),
                file_len: self.len,
            });
        }
        let mut buf = vec![0; len as usize];
        self.file
            .read_exact_at(&mut buf, offset)
            .expect("reading the index");
        Ok(buf)
    }
}

impl RangeReader for FileReader {
    fn file_len(&self) -> u64 {
        self.len
    }

    fn read_range(
        &self,
        offset: u64,
        len: u32,
    ) -> impl std::future::Future<Output = Result<Vec<u8>>> {
        std::future::ready(self.read(offset, len))
    }
}

/// As many documents as the first bytes of a run hold, and whether that was
/// all of them.
pub fn decode_head(head: &[u8], total_len: u64) -> Result<(Vec<u64>, bool)> {
    let (count, n) = varint::decode(&head[1..])?;
    let mut pos = 1 + n;
    let mut docs = Vec::new();
    let mut previous: Option<u64> = None;
    let mut remaining = count as usize;
    while remaining > 0 {
        let group = remaining.min(128);
        let Ok((values, next)) = bitpack::decode_group(head, pos, group) else {
            break;
        };
        for value in values {
            let doc = match previous {
                None => value,
                Some(p) => p + value + 1,
            };
            docs.push(doc);
            previous = Some(doc);
        }
        pos = next;
        remaining -= group;
    }
    let complete = remaining == 0 || head.len() as u64 == total_len;
    Ok((docs, complete))
}
