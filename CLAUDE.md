# atari-disk: notes for Claude

A standalone Rust library for Atari 8-bit disk images (ATR, XFD) and their
DOS 2 and SpartaDOS file systems. Started 2026-11-23 to be used by
**Edit Atari** (`~/Coding/atari_editor`, github.com/Calab/edit_atari),
which will add a File > Image Manager page on top of it. Kept in its own
repository so others can use it without the editor. Licence: MIT OR
Apache-2.0. Repo: github.com/Calab/atari-disk.

## Working with Steve
- Steve Norton is the author: an Atari 8-bit expert, but new to Rust, so
  explain cargo steps plainly. Uses VS Code on Ubuntu.
- Commit only when asked; end messages with the Co-Authored-By line. Never
  push: Steve pushes himself. `gh` is not installed.
- Files for Steve to review go in this project's `tmp/` (git-ignored), not
  the session scratchpad.
- CI must pass `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings` and `cargo test`.
- No dependencies unless there's a strong reason.

## Samples
Real images come from Steve's NAS:
`/mnt/NAS_Storage/Retro Computing & Emulation/Atari computers/800, XL, XE/! Software/Disks/`
(the `DOS/` folder there has DOS 2.0S, 2.5, MyDOS, SpartaDOS 3.2g, SDX
4.49e). Searching the whole NAS is slow; search only that folder. Copy any
image used into `samples/<DOS>/` (git-ignored, copyrighted);
`tests/samples.rs` round-trips them all byte for byte. Verify writes in
AltirraSDL (DIR, free sectors, the DOS's own disk check).

## Design (agreed with Steve)
- Works on an image in memory; no file I/O in the library.
- Layers: `image.rs` (ATR/XFD sectors, done) → file systems (DOS 2, then
  SpartaDOS) → a common API the editor uses: list a directory, read,
  write, rename, delete, make directory (SpartaDOS only), format a blank
  disk. No copy or move.
- Formatted blank disks are data disks, NOT bootable: DOS.SYS/DUP.SYS or
  a SpartaDOS boot loader can't be shipped (copyright).
- XFD only for layouts its file size identifies (128-byte sectors, or 720
  × 256).
- Respect locked files and the ATR write-protect flag (report them; the
  program decides).
- Steve said "SpartaDOS 4.5", meaning SpartaDOS X 4.x; it shares the
  SpartaDOS 2/3 file system. Check against the SDX 4.49e sample.
- Later, maybe: MyDOS and DOS XE (don't design them out), 512-byte sectors.

## Editor side (later, in the editor's repository)
File > Image Manager: directory tree + file list, a File name box used
only by Save, buttons Open / Save / Rename / Delete / New Dir / New
Image... / Close. Rename and New Dir ask in a pop-up. A file opened from an
image stays linked to it, so Ctrl+S writes back. Recent Files shows
`MYDISK.ATR › SAMPLE.LST`. Warn that AltirraSDL with the image mounted
read/write can overwrite changes.
