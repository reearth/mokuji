//! `mokuji info` - report what a file's header and section table declare.

use std::fmt;
use std::path::Path;

use mokuji_format::{parse_section_table, Flags, Header, SectionEntry, HEADER_LEN};
use mokuji_read::{block_on::block_on, CountingReader, RangeReader, SliceReader};

/// Anything that stops `info` from finishing.
#[derive(Debug)]
pub enum Error {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not a valid Mokuji file.
    Format(mokuji_format::Error),
    /// A search was asked for with no terms.
    NoTerms,
    /// The input corpus could not be read.
    Corpus(String),
    /// The file parses, but says something untrue about itself.
    Invalid(String),
    /// The command line could not be understood.
    Usage(String),
}

impl From<mokuji::Error> for Error {
    fn from(e: mokuji::Error) -> Self {
        match e {
            mokuji::Error::File(e) => Error::Format(e),
            other => Error::Invalid(other.to_string()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(e) => write!(f, "{e}"),
            Error::NoTerms => write!(f, "search needs at least one term"),
            Error::Corpus(detail) => write!(f, "{detail}"),
            Error::Invalid(detail) => write!(f, "{detail}"),
            Error::Usage(detail) => write!(f, "{detail}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<mokuji_manifest::Error> for Error {
    fn from(e: mokuji_manifest::Error) -> Self {
        match e {
            mokuji_manifest::Error::Format(e) => Error::Format(e),
            other => Error::Invalid(other.to_string()),
        }
    }
}

impl From<mokuji_format::Error> for Error {
    fn from(e: mokuji_format::Error) -> Self {
        Error::Format(e)
    }
}

/// Prints what the file at `path` declares about itself.
pub fn run(path: &Path) -> Result<(), Error> {
    let bytes = std::fs::read(path)?;
    // Read through the range-read interface even though the file is already in
    // memory, so that the reported read count is the one a remote reader would
    // have paid.
    let reader = CountingReader::new(SliceReader::new(bytes));
    let (header, sections) = block_on(load(&reader))?;

    print_header(&header, reader.file_len());
    println!();
    print_sections(&sections, reader.file_len());
    println!();
    println!("reads to load the resident set: {}", reader.reads());

    // After the count, which is about the header and section table. The
    // metadata is resident too, so this costs nothing a reader opening the
    // file would not already pay.
    let index = block_on(mokuji_read::Index::open(&reader))?;
    println!();
    crate::verify::print_meta(&mokuji_meta::Meta::parse(index.meta())?);
    Ok(())
}

async fn load<R: RangeReader>(
    reader: &R,
) -> Result<(Header, Vec<SectionEntry>), mokuji_format::Error> {
    let header = Header::parse(&reader.read_range(0, HEADER_LEN as u32).await?)?;
    let table = reader
        .read_range(header.section_table_offset, header.section_table_len)
        .await?;
    let sections = parse_section_table(&table, header.section_count)?;
    Ok((header, sections))
}

fn print_header(header: &Header, file_len: u64) {
    println!("file");
    println!("  size            {file_len} bytes");
    println!(
        "  spec version    {}.{}",
        header.spec_major, header.spec_minor
    );
    println!("  flags           {}", format_flags(header.flags));
    println!("statistics");
    println!("  documents       {}", header.num_docs);
    println!("  total tokens    {}", header.sum_doclen);
    match header.avg_doclen() {
        Some(avg) => println!("  mean length     {avg:.2}"),
        None => println!("  mean length     n/a (empty index)"),
    }
}

fn print_sections(sections: &[SectionEntry], file_len: u64) {
    println!("sections ({})", sections.len());
    for entry in sections {
        let name = entry.id.name().unwrap_or(if entry.id.is_application() {
            "application"
        } else {
            "unknown"
        });
        // An out-of-bounds section is reported rather than fatal: the point of
        // `info` is to describe a file that may well be broken.
        let note = match entry.check_bounds(file_len) {
            Ok(()) => String::new(),
            Err(_) => "  <- extends past end of file".to_string(),
        };
        println!(
            "  {:#06x} {:<20} {:>12} + {:<10} {:?}{}",
            entry.id.0, name, entry.offset, entry.length, entry.codec, note
        );
    }
}

fn format_flags(flags: Flags) -> String {
    let mut names = Vec::new();
    if flags.contains(Flags::HAS_PAYLOAD) {
        names.push("has_payload");
    }
    if flags.contains(Flags::HAS_FREQS) {
        names.push("has_freqs");
    }
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mokuji_format::{SectionCodec, SectionId, SPEC_MAJOR, SPEC_MINOR};
    use mokuji_read::ReadStats;

    /// A header and section table with nothing behind them, which is enough to
    /// exercise the resident-set load path.
    fn head_only(sections: &[SectionEntry]) -> Vec<u8> {
        let table: Vec<u8> = sections.iter().flat_map(|s| s.encode()).collect();
        let header = Header {
            spec_major: SPEC_MAJOR,
            spec_minor: SPEC_MINOR,
            flags: Flags::default(),
            section_count: sections.len() as u32,
            num_docs: 4,
            sum_doclen: 26,
            section_table_offset: HEADER_LEN as u64,
            section_table_len: table.len() as u32,
        };
        let mut bytes = header.encode().to_vec();
        bytes.extend_from_slice(&table);
        bytes
    }

    fn section(id: SectionId, offset: u64) -> SectionEntry {
        SectionEntry {
            id,
            codec: SectionCodec::Raw,
            offset,
            length: 8,
        }
    }

    #[test]
    fn loads_the_header_and_section_table() {
        let sections = [
            section(SectionId::META, 200),
            section(SectionId::TERM_INDEX, 208),
        ];
        let reader = CountingReader::new(SliceReader::new(head_only(&sections)));
        let (header, parsed) = block_on(load(&reader)).unwrap();

        assert_eq!(header.num_docs, 4);
        assert_eq!(header.avg_doclen(), Some(6.5));
        assert_eq!(parsed, sections);
    }

    /// The resident set costs one read on a cold start (`SPEC.md` §4). The
    /// header and the section table are fetched separately here because their
    /// combined length is not known until the header has been parsed; a reader
    /// that speculatively fetches a larger prefix collapses this to one.
    #[test]
    fn loading_the_resident_set_costs_two_reads() {
        let sections = [section(SectionId::META, 200)];
        let reader = CountingReader::new(SliceReader::new(head_only(&sections)));
        block_on(load(&reader)).unwrap();

        assert_eq!(
            reader.stats(),
            ReadStats {
                reads: 2,
                bytes: (HEADER_LEN + mokuji_format::SECTION_ENTRY_LEN) as u64,
            }
        );
    }

    #[test]
    fn rejects_a_section_table_that_runs_past_the_end() {
        let mut bytes = head_only(&[section(SectionId::META, 200)]);
        // Claim two entries where only one was written.
        bytes[12] = 2;
        let reader = CountingReader::new(SliceReader::new(bytes));
        assert!(block_on(load(&reader)).is_err());
    }

    #[test]
    fn out_of_bounds_sections_are_reported_not_fatal() {
        let sections = [section(SectionId::META, u64::MAX)];
        let bytes = head_only(&sections);
        let file_len = bytes.len() as u64;
        // Printing must survive a file whose offsets are nonsense, since
        // describing broken files is what this command is for.
        print_sections(&sections, file_len);
    }
}
