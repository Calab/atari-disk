# atari-disk

A Rust library for Atari 8-bit disk images: `.ATR` and `.XFD` files, and
the files on them under **Atari DOS 2** (2.0S, 2.0D, 2.5), **MyDOS 4.x**
and **SpartaDOS** (2.x, 3.x and SpartaDOS X). No dependencies.

Disks it writes have been checked in AltirraSDL under each DOS: DIR,
copying, the DOS's own disk check, and booting.

**Status:** complete for the formats above; not yet on crates.io, and the
API may still change a little before a first release.

## Using it

Add it to your `Cargo.toml`:

```toml
[dependencies]
atari-disk = { git = "https://github.com/Calab/atari-disk" }
```

Then:

```rust
use atari_disk::{DiskImage, Timestamp, Volume};

let bytes = std::fs::read("GAMES.ATR")?;
let mut vol = Volume::open(DiskImage::from_bytes(&bytes)?)?;

for entry in vol.list("")? {             // "" is the main directory
    println!("{:12} {:6} bytes", entry.name_text(), entry.size);
}
let listing = vol.read("GAMES>PACMAN.LST")?;      // `>` between directories
vol.write("GAMES>COPY.LST", &listing, Some(Timestamp::new(2026, 11, 24, 15, 30, 0)?))?;

std::fs::write("GAMES.ATR", vol.image().to_bytes())?;
```

The library works on the image in memory and does no file I/O of its
own: your program reads the file, and saves `vol.image().to_bytes()`
the way it prefers. Every change either succeeds completely or leaves
the disk as it was.

To see it on a real disk:

```text
cargo run --example ls -- GAMES.ATR
```

lists every directory and file, with sizes, dates and flags, and any
problems found on the disk.

## What it does

`Volume` is the main type. For every file system it can:

- list directories, read files (all or a byte range), write, rename,
  move, delete and lock/unlock files, and make directories (MyDOS,
  SpartaDOS);
- set dates, hidden and archive flags, and volume names (SpartaDOS);
- check a name before using it, and say whether a file would fit;
- format blank data disks: DOS 2 single, double and enhanced density;
  MyDOS and SpartaDOS single, enhanced, double, double-sided, or any
  size up to 65535 sectors (512-byte sectors for SpartaDOS X);
- make a disk bootable from a DOS disk you supply (no DOS ships with
  the crate), and say whether a disk is set up to boot;
- validate a disk (bitmap errors, sectors claimed twice, lost sectors,
  damaged files) and say what every sector holds;
- say what a disk's DOS supports (`features()`), so a program can offer
  only what works.

`DiskImage` gives sector-by-sector access for anything else, including
disks with no file system the crate knows. **Atari DOS 3**, **DOS XE**,
**LiteDOS** and **TOP-DOS** disks are recognised but not supported: they
open with their own `FileSystem` (and `name()` to show), and give sector
access only.

Each `FileSystem` is named for its disk format, so DOSes that write the
same disks share one: **SmartDOS** and **DOS XL** disks are DOS 2, and
**K-DOS** disks are MyDOS.

File names and paths are ATASCII bytes, exactly as the Atari stores
them, so every file can be reached; `atari_disk::atascii` converts to
and from ASCII.

Not covered: copying files (read, then write), undeleting, and image
formats other than ATR and XFD (DCM, ATX, PRO).

## Documentation

Every public type and call is documented, most with examples:

```text
cargo doc --open
```

The `formats` page in those docs describes the ATR and XFD files and
each file system byte by byte, including details found on real disks.

## Testing

`cargo test` runs the built-in tests. The tests in `tests/samples.rs`
also read real disk images from `samples/DOS 2/`, `samples/MyDOS/`,
`samples/SpartaDOS/`, `samples/DOS 3/`, `samples/DOS XE/`,
`samples/LiteDOS/` and `samples/TOP-DOS/`. Those images are
copyrighted, so they aren't in the repository, and missing folders are
skipped.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
licence, shall be dual licensed as above, without any additional terms or
conditions.
