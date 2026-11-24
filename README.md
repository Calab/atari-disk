# atari-disk

A Rust library for reading and writing Atari 8-bit disk images.

**Status: early work.** Disk images (ATR and XFD), DOS 2, MyDOS and
SpartaDOS read and write; the API may still change before a first release.

## What it does

- **Disk images**: `.ATR` and `.XFD` files with 128-, 256- or 512-byte
  sectors (512 in ATR only):
  single (720 × 128), enhanced (1040 × 128), double (720 × 256) and larger
  ATR images such as hard-disk partitions. Every image is written back
  exactly as it was read, including the three ways double-density images
  store their boot sectors and the extra ATR header bytes (such as the
  write-protect flag).
- **File systems**:
  - Atari DOS 2.0S, 2.0D and 2.5
  - MyDOS 4.x (subdirectories, disks up to 65535 sectors)
  - SpartaDOS 2.x/3.x and SpartaDOS X (which share one file-system format)

  For each: list directories, read (all or part of a file), write,
  rename, move, delete, lock and unlock files, make directories, set
  dates, hidden/archive flags and volume names (SpartaDOS), check names,
  map what every sector holds, validate a disk, read and write raw
  sectors, format blank data disks, and make a disk bootable from a DOS
  disk you supply. File names and paths are ATASCII bytes, with
  conversions to and from ASCII.

Not covered: copying files (read then write), undeleting, and image
formats other than ATR and XFD (DCM, ATX, PRO).

The library works on images in memory and does no file I/O of its own: read
the file, make changes, then save `DiskImage::to_bytes()` the way your
program prefers.

```rust
use atari_disk::DiskImage;

let disk = DiskImage::from_bytes(&std::fs::read("GAMES.ATR")?)?;
println!("{} sectors of {} bytes", disk.sector_count(), disk.sector_size());
let boot = disk.sector(1)?;
```

## Testing

`cargo test` runs the built-in tests. The tests in `tests/samples.rs` also
read real disk images from `samples/DOS 2/`, `samples/SpartaDOS/` and
`samples/MyDOS/`. Those images are copyrighted, so they aren't in the
repository, and any missing folder is skipped.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
licence, shall be dual licensed as above, without any additional terms or
conditions.
