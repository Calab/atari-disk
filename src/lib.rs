//! Read and write Atari 8-bit disk images.
//!
//! * [`DiskImage`]: the sectors of an `.ATR` or `.XFD` file.
//! * [`Dos2`]: the Atari DOS 2 file system (2.0S, 2.0D, 2.5): list the
//!   directory and read files. Writing, renaming, deleting and
//!   formatting are coming, then SpartaDOS (2.x, 3.x and SpartaDOS X).
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

mod dos2;
mod error;
mod image;

pub use dos2::{Dos2, Entry as Dos2Entry};
pub use error::{Error, Result};
pub use image::{Container, Density, DiskImage};
