//! The one error type the crate returns.

use std::fmt;

/// Everything that can go wrong reading or changing a disk image. More
/// kinds may be added, so a `match` needs a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The bytes aren't a disk image this crate understands; the text says
    /// why.
    NotAnImage(String),
    /// A valid image, but of a kind not supported (yet), e.g. 512-byte
    /// sectors.
    Unsupported(String),
    /// Sector numbers run from 1 to the image's sector count.
    SectorOutOfRange {
        /// The sector asked for.
        sector: u32,
        /// The disk's sector count.
        count: u32,
    },
    /// Data written to a sector must be exactly the sector's length.
    WrongSectorLength {
        /// The sector.
        sector: u32,
        /// Its length.
        expected: usize,
        /// The length given.
        got: usize,
    },
    /// XFD images have no header, so they can only hold layouts that
    /// can be recognised from the file size alone.
    NotRepresentable(String),
    /// The disk doesn't hold the file system asked for; the text says why.
    UnknownFileSystem(String),
    /// The file system's own records don't add up (e.g. a file's sectors
    /// lead off the disk); the text says where.
    Damaged(String),
    /// No file or directory of that name.
    NotFound(String),
    /// A directory was given where a file is needed (e.g. to read it).
    IsDirectory(String),
    /// A file was given where a directory is needed (e.g. in a path).
    NotADirectory(String),
    /// A file or directory of that name is already there.
    AlreadyExists(String),
    /// The name breaks the file system's rules; the text says how.
    BadName(String),
    /// The file is locked, so it can't be written, renamed or deleted
    /// until it's unlocked.
    Locked(String),
    /// Not enough free sectors for the file.
    DiskFull {
        /// Sectors the change needs.
        needed: u32,
        /// Sectors free.
        free: u32,
    },
    /// The directory has no room for another entry (DOS 2 holds 64).
    DirectoryFull,
    /// Only empty directories can be deleted.
    DirectoryNotEmpty(String),
    /// Not a date and time the file system can store (SpartaDOS: 1980 to
    /// 2079).
    BadDate(String),
    /// The disk given to boot from has no DOS to copy, or one that
    /// doesn't suit the disk being made bootable; the text says why.
    NotBootable(String),
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
            Error::UnknownFileSystem(why) => write!(f, "unknown file system ({why})"),
            Error::Damaged(why) => write!(f, "damaged: {why}"),
            Error::NotFound(name) => write!(f, "{name} not found"),
            Error::IsDirectory(name) => write!(f, "{name} is a directory"),
            Error::NotADirectory(name) => write!(f, "{name} is not a directory"),
            Error::AlreadyExists(name) => write!(f, "{name} already exists"),
            Error::BadName(why) => write!(f, "bad name: {why}"),
            Error::Locked(name) => write!(f, "{name} is locked"),
            Error::DiskFull { needed, free } => {
                write!(f, "disk full: {needed} sectors needed, {free} free")
            }
            Error::DirectoryFull => write!(f, "the directory is full"),
            Error::DirectoryNotEmpty(name) => write!(f, "{name} is not empty"),
            Error::BadDate(when) => write!(f, "bad date: {when}"),
            Error::NotBootable(why) => write!(f, "can't make it bootable: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Shorthand for results from this crate.
pub type Result<T> = std::result::Result<T, Error>;
