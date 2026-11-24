//! The one error type the crate returns.

use std::fmt;

/// Everything that can go wrong reading or changing a disk image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes aren't a disk image this crate understands; the text says
    /// why.
    NotAnImage(String),
    /// A valid image, but of a kind not supported (yet), e.g. 512-byte
    /// sectors.
    Unsupported(String),
    /// Sector numbers run from 1 to the image's sector count.
    SectorOutOfRange { sector: u32, count: u32 },
    /// Data written to a sector must be exactly the sector's length.
    WrongSectorLength {
        sector: u32,
        expected: usize,
        got: usize,
    },
    /// XFD images have no header, so they can only hold layouts that
    /// can be recognised from the file size alone.
    NotRepresentable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAnImage(why) => write!(f, "not a disk image: {why}"),
            Error::Unsupported(what) => write!(f, "not supported: {what}"),
            Error::SectorOutOfRange { sector, count } => {
                write!(f, "sector {sector} is outside the disk (1 to {count})")
            }
            Error::WrongSectorLength {
                sector,
                expected,
                got,
            } => write!(
                f,
                "sector {sector} holds {expected} bytes, but {got} were given"
            ),
            Error::NotRepresentable(why) => write!(f, "can't be saved that way: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Shorthand for results from this crate.
pub type Result<T> = std::result::Result<T, Error>;
