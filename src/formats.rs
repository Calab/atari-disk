//! How the disk images and file systems are laid out, byte by byte: what
//! this crate reads and writes. Nothing here is needed to *use* the crate
//! ([`crate::Volume`] does it all); it's for checking its work, writing
//! your own tools, or curiosity. Offsets in `$` are hexadecimal.
//!
//! Much of it was checked against real disks: DOS 2.0S, DOS 2.5, MyDOS
//! 4.5, SpartaDOS 3.2g and SpartaDOS X 4.49 images, blanks those DOSes
//! formatted themselves, and disks this crate wrote, tried in AltirraSDL.
//!
//! # Sectors
//! Sectors are numbered from 1, as the Atari numbers them. A disk has
//! 128-byte sectors (single density: 720 of them; "enhanced" or "medium"
//! density: 1040), 256-byte sectors (double density: 720; double-sided
//! on an XF551: 1440; hard disks: up to 65535) or, for SpartaDOS X hard
//! disks only, 512-byte sectors.
//!
//! On a 256-byte disk the first three sectors are still 128 bytes long,
//! because the Atari OS always reads them that way. On a 512-byte disk
//! they're full length (the OS can't boot from such a disk anyway).
//!
//! **Booting:** the OS reads sector 1; its byte 1 says how many 128-byte
//! sectors to load (3 for DOS 2 and SpartaDOS), bytes 2–3 where, and
//! bytes 4–5 the DOS start address. It then calls the loaded code at
//! byte 6; carry set on return means the boot failed, and the OS prints
//! BOOT ERROR and tries again. The blank data disks this crate formats
//! have a boot sector of their own that does exactly that, as a disk
//! formatted by DOS with no DOS on it does (the DOSes' own boot code is
//! copyrighted, so it isn't shipped).
//!
//! # ATR files
//! A 16-byte header, then the sectors:
//!
//! | Bytes | Meaning |
//! |---|---|
//! | 0–1 | `$96 $02` (the sum of the letters "NICKATARI") |
//! | 2–3 | Size of the sector data in 16-byte paragraphs, low word |
//! | 4–5 | Sector size: 128, 256 or 512 |
//! | 6 | Size in paragraphs, high byte |
//! | 7–15 | Extras some programs use, kept as they are. Bit 0 of byte 15 is the write-protect flag (APE and SIO2PC). |
//!
//! With 256-byte sectors the three boot sectors are normally stored as
//! 128 bytes each. Some images, made by older tools, give them three full
//! 256-byte sectors' room instead, in one of two ways:
//!
//! * packed: the three 128-byte sectors, then 384 bytes of zeros;
//! * in slots: each in the first half of its own 256 bytes.
//!
//! When the 384 bytes after the boot sectors are all zero the image is
//! read as packed, otherwise as slots. Each image is written back the
//! way it came, so every image round-trips byte for byte.
//!
//! # XFD files
//! No header, only the sectors, so the layout comes from the file size.
//! Any multiple of 128 bytes is read as 128-byte sectors; the two
//! double-density sizes (183,936 bytes, or 184,320 with padded boot
//! sectors) are read as 720 256-byte sectors. Other layouts can only be
//! saved as ATR.
//!
//! # Atari DOS 2 (2.0S, 2.0D, 2.5)
//! * Sectors 1–3: boot sectors. DOS 2 keeps facts about DOS.SYS in them:
//!
//!   | Byte | Meaning |
//!   |---|---|
//!   | `$0E` | DOS.SYS is present: 1 on 128-byte disks, 2 on 256-byte (0: no DOS) |
//!   | `$0F`–`$10` | DOS.SYS's first sector |
//!   | `$11` | Data bytes per sector: `$7D` (125) or `$FD` (253) |
//!
//!   DUP writes DOS.SYS from memory, so a written DOS.SYS isn't a byte
//!   copy of the one on the disk DOS booted from.
//! * Sector 360: the VTOC (volume table of contents):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | DOS code: 2 (MyDOS: more, see below) |
//!   | 1–2 | Sectors DOS can use (707 single or double density, 1010 enhanced) |
//!   | 3–4 | Free sectors (on enhanced disks, of sectors 1–719 only) |
//!   | 10– | Bitmap: bit 7 of byte 10 is sector 0; 1 = free |
//!
//!   DOS 2 never uses sector 720 (the 810 drive can't always read it).
//! * Sectors 361–368: the directory, 8 entries of 16 bytes per sector (64
//!   files):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | Flags: `$80` deleted, `$40` in use, `$20` locked, `$02` made by DOS 2, `$01` open for writing (and see DOS 2.5 below); MyDOS adds `$10` and `$04` |
//!   | 1–2 | Length in sectors |
//!   | 3–4 | First sector |
//!   | 5–12 | Name, padded with spaces |
//!   | 13–15 | Extension, padded with spaces |
//!
//!   An entry whose flags are 0 has never been used, and ends the
//!   directory. Deleting a file sets its flags to `$80`.
//! * A file is a chain of sectors. The last 3 bytes of each hold the file
//!   number (its directory entry, 0–63) in the top 6 bits, then the next
//!   sector in 10 bits (0 at the end of the file), then how many bytes of
//!   the sector are data. So a 128-byte sector holds 125 bytes of the
//!   file and a 256-byte sector 253. Even an empty file has one sector.
//! * **DOS 2.5 enhanced density** (1040 sectors of 128 bytes) adds a
//!   second VTOC in sector 1024 for sectors 720–1023: its bytes 0–83
//!   repeat the VTOC bitmap for sectors 48–719, bytes 84–121 cover
//!   sectors 720–1023, and bytes 122–123 count the free ones (303 on a
//!   blank disk). DOS 2.0 sees only the first VTOC.
//!
//!   A file that uses sectors above 719 gets flags `$03` (`$23` locked):
//!   "in use" is clear so DOS 2.0 ignores it, and DOS 2.5 lists it in
//!   angle brackets, `<BIG     DAT>`. (From DOS 2.5's DOS.SYS: an entry
//!   is a file if its flags AND `$43` are `$03`, or bit 0 is clear; DIR
//!   prints the brackets when bit 0 is set.) DOS 2.5 can't read 256-byte
//!   sectors (error 143).
//!
//! # MyDOS 4.x
//! MyDOS keeps DOS 2's layout and adds:
//! * Every sector is usable, 720 included: a 720-sector disk has 708.
//! * When the bitmap doesn't fit in sector 360 it runs on into 359, 358
//!   and so on, and VTOC byte 0 is 2 plus the bitmap's length in 256-byte
//!   units: 3 for 1040 × 128, 35 for 65535 sectors (which takes 66
//!   128-byte or 33 256-byte sectors).
//! * Files flagged `$04` (`$46`; MyDOS uses them on disks over 1023
//!   sectors) have 16-bit links: next sector high byte, low byte, byte
//!   count, and no file number.
//! * An entry flagged `$10` is a subdirectory: 8 consecutive sectors from
//!   its first sector, laid out like the main directory. It has no link
//!   back to its parent.
//! * The boot sector bytes `$0E`–`$11` are as DOS 2's. Writing a file
//!   called DOS.SYS seems to make MyDOS write its DOS and point the boot
//!   sector at it.
//!
//! # SpartaDOS (2.x, 3.x and SpartaDOS X)
//! One file system for all of them (BW-DOS uses it too).
//! * Sectors 1–3: boot sectors. Sector 1 also holds the volume
//!   information:
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | `$07` | `$80`: marks a SpartaDOS disk (otherwise SpartaDOS gives error 148) |
//!   | `$09`–`$0A` | Sector map of the main directory |
//!   | `$0B`–`$0C` | Sectors on the disk |
//!   | `$0D`–`$0E` | Free sectors |
//!   | `$0F` | Sectors in the bitmap |
//!   | `$10`–`$11` | First bitmap sector |
//!   | `$12`–`$13` | Where to start looking for a free data sector |
//!   | `$14`–`$15` | Where to start looking for a free directory sector |
//!   | `$16`–`$1D` | Volume name, padded with spaces |
//!   | `$1E` | Tracks (bit 7 set: double-sided) |
//!   | `$1F` | Sector size: `$80` 128 bytes, `$00` 256, `$01` 512 (SpartaDOS X) |
//!   | `$20` | File system version: `$20` SpartaDOS 2/3, `$21` SpartaDOS X |
//!   | `$21`–`$25` | Kept by SpartaDOS as it works; SpartaDOS 3.2's XINIT formats them `06 01 FF FF 00` |
//!   | `$26` | Volume sequence number (changes with each write) |
//!   | `$27` | Volume random number (set by format) |
//!   | `$28`–`$29` | Sector map of the file to boot (e.g. `X32G.DOS`); 0 for none, as on SpartaDOS X disks, which boot from the cartridge |
//!
//!   SpartaDOS 3.2's boot code is the same on every density.
//! * The bitmap: one bit per sector, from sector 0, bit 7 first; 1 =
//!   free. XINIT keeps the disk's last sector out of use (720 single
//!   density, 1040 enhanced) and starts the data search at sector 32 (37
//!   on double density), leaving the sectors after the main directory for
//!   directories to grow into. SpartaDOS X leaves the last sector free.
//! * Every file, directories included, has a chain of *sector maps*.
//!   Each map sector starts with the next map (0 at the end) and the
//!   previous one, then lists the file's data sectors, 2 bytes each: 62
//!   per 128-byte map, 126 per 256-byte, 254 per 512-byte. Data sectors
//!   hold nothing but data, all of every sector up to the file's length.
//! * A directory is a file of 23-byte entries. The first is its header:
//!   flags `$28`, the parent directory's sector map (0 for the main
//!   directory) in bytes 1–2, its own length in bytes 3–5, its name in
//!   6–16 and its date in 17–22. Each entry after that is:
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | Flags: `$01` locked, `$02` hidden, `$04` archived, `$08` in use, `$10` deleted, `$20` directory |
//!   | 1–2 | First sector map |
//!   | 3–5 | Length in bytes |
//!   | 6–13 | Name, padded with spaces |
//!   | 14–16 | Extension, padded with spaces |
//!   | 17–19 | Date: day, month, year (2 digits; this crate reads them as 1980–2079) |
//!   | 20–22 | Time: hours, minutes, seconds |
//!
//!   Flags of 0 end the directory; a deleted entry's flags lose `$08` and
//!   gain `$10`. In the parent, a directory's own length isn't kept up to
//!   date (SpartaDOS X leaves it at 23), so the header's length is the
//!   one to trust.
//!
//! # Names
//! Names are 8 + 3 characters, padded with spaces, stored as ATASCII
//! (see [`crate::atascii`]). DOS 2 and MyDOS names are letters and
//! digits, starting with a letter; SpartaDOS also allows `_` and a digit
//! first. This crate makes new names by those rules, but finds existing
//! files by their exact stored bytes, so even files with odd names (as
//! some old disks have) can be read, renamed and deleted.
