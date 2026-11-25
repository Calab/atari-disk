//! Read and write Atari 8-bit disk images (`.ATR` and `.XFD`) and the
//! files on them: Atari DOS 2 (2.0S, 2.0D, 2.5), MyDOS 4.x, and
//! SpartaDOS (2.x, 3.x and SpartaDOS X).
//!
//! # Where to start
//! * [`Volume`] is what most programs need: open a disk, then list,
//!   read, write, rename, move, delete and lock files, make directories,
//!   set dates, check the disk, or format a new one. [`Volume::features`]
//!   says what the disk's DOS can do.
//! * [`DiskImage`] is the disk itself, sector by sector, for programs
//!   that read or write sectors directly.
//! * [`atascii`]: names and paths are ATASCII bytes, as the Atari stores
//!   them; this converts to and from ASCII text.
//! * [`formats`]: how the images and file systems are laid out, byte by
//!   byte.
//! * [`Dos2`] and [`Sparta`]: the low-level readers underneath
//!   [`Volume`], for tools that want the raw directory entries.
//!
//! The crate does no file I/O: read the image file's bytes, open them,
//! make changes, then save [`DiskImage::to_bytes`] the way your program
//! prefers (e.g. to a temporary file, then renamed over the original).
//! Every change either succeeds completely or leaves the disk as it was.
//!
//! # Example
//! ```
//! use atari_disk::{Container, DiskImage, Layout, Timestamp, Volume};
//!
//! // A new SpartaDOS disk (or: Volume::open(DiskImage::from_bytes(&bytes)?)?).
//! let mut vol = Volume::format(Layout::SpartaDouble, Container::Atr)?;
//! vol.set_volume_name("MYDISK")?;
//! vol.make_dir("GAMES", None)?;
//! let when = Timestamp::new(2026, 11, 24, 15, 30, 0)?;
//! vol.write("GAMES>HELLO.TXT", b"Hello, Atari\x9b", Some(when))?;
//!
//! for entry in vol.list("GAMES")? {
//!     println!("{:12} {:6} bytes", entry.name_text(), entry.size);
//! }
//! assert_eq!(vol.read("GAMES>HELLO.TXT")?, b"Hello, Atari\x9b");
//! assert!(vol.validate()?.is_empty());
//!
//! let bytes = vol.image().to_bytes(); // ready to save as an .ATR file
//! # assert_eq!(bytes.len(), 16 + 3 * 128 + 717 * 256);
//! # Ok::<(), atari_disk::Error>(())
//! ```
//!
//! # Errors
//! Every call that can fail returns [`Result`], with an [`Error`] whose
//! text (`to_string()`) is plain English, fit to show a user: "disk
//! full: 12 sectors needed, 3 free", "TEXT.TXT is locked".

#![warn(missing_docs)]

pub mod atascii;
mod check;
mod dos2;
mod error;
pub mod formats;
mod image;
mod names;
mod sparta;
mod volume;

pub use check::{Problem, SectorUse};
pub use dos2::{Dos2, Entry as Dos2Entry};
pub use error::{Error, Result};
pub use image::{Container, Density, DiskImage};
pub use sparta::{Entry as SpartaEntry, Sparta, Timestamp};
pub use volume::{DirEntry, EntryKind, Features, FileSystem, Layout, Volume};
