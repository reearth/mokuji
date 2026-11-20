use std::fmt;

/// Result alias for format-level operations.
pub type Result<T> = std::result::Result<T, Error>;

/// A malformed or unsupported Mokuji file.
///
/// Every variant here describes untrusted input that was rejected. Per
/// `SPEC.md` §8, a reader must fail rather than panic on such input, so these
/// are returned from parsing rather than asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The file does not begin with the Mokuji magic.
    BadMagic {
        /// The first bytes that were found in place of the magic.
        found: [u8; 8],
    },
    /// The file declares a major specification version this reader does not
    /// implement.
    UnsupportedVersion {
        /// The declared major version.
        major: u8,
        /// The declared minor version.
        minor: u8,
    },
    /// A buffer was shorter than the structure it was expected to contain.
    Truncated {
        /// Bytes required.
        expected: usize,
        /// Bytes available.
        found: usize,
    },
    /// A reserved field was non-zero, or a field held a value outside its
    /// defined range.
    Malformed {
        /// What was wrong.
        detail: &'static str,
    },
    /// An offset and length pair does not lie within the file.
    OutOfBounds {
        /// Start of the range.
        offset: u64,
        /// Length of the range.
        len: u64,
        /// Total size of the file.
        file_len: u64,
    },
    /// The bytes could not be read from where they are kept: a file that
    /// went away, a disk that failed. Not a fault of the file's contents.
    Read {
        /// What the source reported, as `std::io::ErrorKind` names it.
        kind: std::io::ErrorKind,
        /// Its message.
        detail: String,
    },
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Read {
            kind: e.kind(),
            detail: e.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic { found } => {
                write!(f, "not a Mokuji file (magic was {found:?})")
            }
            Error::UnsupportedVersion { major, minor } => {
                write!(f, "unsupported Mokuji version {major}.{minor}")
            }
            Error::Truncated { expected, found } => {
                write!(f, "truncated: expected {expected} bytes, found {found}")
            }
            Error::Malformed { detail } => write!(f, "malformed: {detail}"),
            Error::OutOfBounds {
                offset,
                len,
                file_len,
            } => write!(
                f,
                "range {offset}..+{len} lies outside a file of {file_len} bytes"
            ),
            Error::Read { detail, .. } => write!(f, "could not read: {detail}"),
        }
    }
}

impl std::error::Error for Error {}
