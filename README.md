# atari-disk

A Rust library for reading and writing Atari 8-bit disk images.

**Status: early work.** The disk-image layer (ATR and XFD) is done. The
DOS 2 and SpartaDOS file systems are next.

## What it does

- **Disk images**: `.ATR` and `.XFD` files with 128- or 256-byte sectors:
  single (720 × 128), enhanced (1040 × 128), double (720 × 256) and larger
  ATR images such as hard-disk partitions. Every image is written back
  exactly as it was read, including the three ways double-density images
  store their boot sectors and the extra ATR header bytes (such as the
  write-protect flag).
- **File systems** (in progress):
  - Atari DOS 2.0S, 2.0D and 2.5
  - SpartaDOS 2.x/3.x and SpartaDOS X (which share one file-system format)

  For each: list directories, read, write, rename and delete files, make
  directories (SpartaDOS), and format blank disks. Copy and move are left
  to the program using the library.

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
