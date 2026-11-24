//! Read and write Atari 8-bit disk images.
//!
//! * [`DiskImage`]: the sectors of an `.ATR` or `.XFD` file (128-, 256-
//!   or 512-byte sectors), read and written directly if you like.
//! * [`Volume`]: a disk and its file system: list, read (all or part of
//!   a file), write, rename, move, delete, lock and unlock files; make
//!   directories; set dates, hidden and archive flags; check names; see
//!   what every sector holds and validate the disk; format blank data
//!   disks; make a disk bootable from a DOS disk you supply. For Atari
//!   DOS 2 (2.0S, 2.0D, 2.5), MyDOS 4.x and SpartaDOS (2.x, 3.x and
//!   SpartaDOS X).
//! * [`atascii`]: names and paths are ATASCII bytes; this converts to and
//!   from ASCII text.
//! * [`Dos2`] and [`Sparta`]: the low-level readers underneath.
//!
//! Everything works on an image in memory: read the file's bytes, change
//! the image, then write [`DiskImage::to_bytes`] back. The crate does no
//! file I/O of its own, so the program using it decides how to save
//! safely.
//!
//! ```
//! use atari_disk::{Container, DiskImage};
//!
//! let mut disk = DiskImage::new(Container::Atr, 128, 720)?;
//! disk.write_sector(360, &[0; 128])?;
//! let bytes = disk.to_bytes(); // ready to save as an .ATR file
//! assert_eq!(DiskImage::from_bytes(&bytes)?, disk);
//! # Ok::<(), atari_disk::Error>(())
//! ```

pub mod atascii;
mod check;
mod dos2;
mod error;
mod image;
mod names;
mod sparta;
mod volume;

pub use check::{Problem, SectorUse};
pub use dos2::{Dos2, Entry as Dos2Entry};
pub use error::{Error, Result};
pub use image::{Container, Density, DiskImage};
pub use sparta::{Entry as SpartaEntry, Sparta, Timestamp};
pub use volume::{DirEntry, EntryKind, FileSystem, Layout, Volume};
