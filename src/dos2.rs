//! Atari DOS 2 file systems: DOS 2.0S, 2.0D and 2.5 (single, double and
//! enhanced density), and MyDOS 4.x, which builds on them.
//!
//! # Layout
//! * Sectors 1–3: boot sectors; DOS.SYS follows from sector 4.
//! * Sector 360: the VTOC (volume table of contents):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | DOS code: 2 (MyDOS: more for a VTOC of several sectors, below) |
//!   | 1–2 | Sectors DOS can use (707 single density, 1010 enhanced) |
//!   | 3–4 | Free sectors (on enhanced disks, of sectors 1–719 only) |
//!   | 10– | Bitmap: bit 7 of byte 10 is sector 0; 1 = free |
//!
//! * Sectors 361–368: the directory, 8 entries of 16 bytes per sector
//!   (64 files):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | Flags: `$80` deleted, `$40` in use, `$20` locked, `$10` a MyDOS directory, `$04` MyDOS 16-bit links, `$02` made by DOS 2, `$01` open for writing (or see below) |
//!   | 1–2 | Length in sectors |
//!   | 3–4 | First sector |
//!   | 5–12 | Name, padded with spaces |
//!   | 13–15 | Extension, padded with spaces |
//!
//!   An entry whose flags are 0 has never been used, and ends the
//!   directory.
//! * A file is a chain of sectors. The last 3 bytes of each hold the file
//!   number (its directory entry, 0–63) in the top 6 bits, then the next
//!   sector in 10 bits (0 at the end of the file), then how many bytes of
//!   the sector are data. So a 128-byte sector holds 125 bytes of the
//!   file and a 256-byte sector 253.
//! * DOS 2.5 enhanced density (1040 sectors of 128 bytes) adds a second
//!   VTOC in sector 1024 for sectors 720–1023: its bytes 0–83 repeat the
//!   VTOC bitmap for sectors 48–719, bytes 84–121 cover sectors 720–1023,
//!   and bytes 122–123 count the free ones. DOS 2.0 sees only the first
//!   VTOC, so on such a disk it shows fewer free sectors than DOS 2.5.
//!
//!   A file that uses sectors above 719 has flags `$03` (`$23` locked):
//!   "in use" is clear so DOS 2.0 ignores it, and DOS 2.5 lists it in
//!   angle brackets, `<BIG     DAT>`. (From DOS 2.5's DOS.SYS: an entry is
//!   a file if its flags AND `$43` are `$03`, or bit 0 is clear; DIR
//!   prints the brackets when bit 0 is set.)
//!
//! # MyDOS
//! * Every sector is usable, 720 included: a 720-sector disk has 708.
//! * When the bitmap doesn't fit in sector 360 it runs on into 359, 358
//!   and so on, and VTOC byte 0 is 2 plus the bitmap's length in 256-byte
//!   units (3 for 1040 × 128; 35 for 65535 sectors, which takes 66
//!   128-byte or 33 256-byte sectors).
//! * Files flagged `$04` (`$46`, used on disks over 1023 sectors) have
//!   16-bit links: next sector high byte, low byte, byte count; no file
//!   number.
//! * A directory entry flagged `$10` is a subdirectory: 8 sectors from
//!   its first sector, laid out like the main directory.

use crate::error::{Error, Result};
use crate::image::DiskImage;
use crate::names::Name;

const VTOC: u32 = 360;
const VTOC2: u32 = 1024;
const DIR_FIRST: u32 = 361;
const DIR_SECTORS: u32 = 8;
const ENTRY_LEN: usize = 16;
const ENTRIES_PER_SECTOR: usize = 8;
/// Where the bitmap starts in the VTOC.
const BITMAP: usize = 10;

const DELETED: u8 = 0x80;
const IN_USE: u8 = 0x40;
const LOCKED: u8 = 0x20;
const SUBDIR: u8 = 0x10;
const BIG_LINKS: u8 = 0x04;
const MADE_BY_DOS2: u8 = 0x02;
const OPEN: u8 = 0x01;

/// Which DOS 2-style file system a disk holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavour {
    /// DOS 2.0S/2.0D/2.5 on single or double density.
    Dos2,
    /// DOS 2.5 enhanced density, with a second VTOC in sector 1024.
    Enhanced,
    /// MyDOS 4.x.
    MyDos,
}

/// Where things are on one disk.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    flavour: Flavour,
    /// Sectors the VTOC takes, from 360 down.
    vtoc_sectors: u32,
    size: usize,
    count: u32,
}

impl Geometry {
    /// Where sector `n`'s bit is in the bitmap: (sector, byte, mask).
    fn bit_at(self, n: u32) -> Option<(u32, usize, u8)> {
        let mask = 0x80 >> (n % 8);
        if self.flavour == Flavour::Enhanced && (720..VTOC2).contains(&n) {
            return Some((VTOC2, n as usize / 8 - 6, mask));
        }
        let at = BITMAP + n as usize / 8;
        let k = (at / self.size) as u32;
        (k < self.vtoc_sectors).then(|| (VTOC - k, at % self.size, mask))
    }

    /// Is `n` a sector DOS keeps for itself (boot, VTOC, directory)?
    fn reserved(self, n: u32) -> bool {
        n <= 3
            || (VTOC + 1 - self.vtoc_sectors..DIR_FIRST + DIR_SECTORS).contains(&n)
            || (self.flavour == Flavour::Enhanced && n == VTOC2)
            || (self.flavour != Flavour::MyDos && n == 720)
    }

    /// New files on big MyDOS disks get 16-bit links.
    fn big_links(self) -> bool {
        self.flavour == Flavour::MyDos && self.count > 1023
    }

    /// The highest sector a new file can use.
    fn last_usable(self) -> u32 {
        if self.big_links() {
            self.count
        } else {
            // The 10-bit sector links can't reach past 1023.
            self.count.min(VTOC2 - 1)
        }
    }
}

/// A DOS 2 or MyDOS disk, read from a [`DiskImage`].
#[derive(Debug, Clone)]
pub struct Dos2<'a> {
    image: &'a DiskImage,
    geometry: Geometry,
}

/// One entry in a DOS 2 or MyDOS directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// The first sector of the directory the entry is in: 361 for the
    /// main one, or a MyDOS subdirectory's.
    pub dir: u32,
    /// Where the entry is in its directory, 0–63. DOS also writes it in
    /// every sector of the file, as the file number (except with MyDOS's
    /// 16-bit links).
    pub index: u8,
    /// The name as stored: 8 bytes, padded with spaces.
    pub name: [u8; 8],
    /// The extension as stored: 3 bytes, padded with spaces.
    pub ext: [u8; 3],
    /// The flags byte (see the module notes).
    pub flags: u8,
    /// The length in sectors, as the directory gives it.
    pub sectors: u16,
    /// The first sector.
    pub start: u16,
    /// The length in bytes, found by following the file's sectors (0 for
    /// a directory).
    pub size: u32,
    /// Following the file's sectors went wrong (see [`Dos2::read`] for
    /// why); `size` counts the bytes up to there.
    pub damaged: bool,
}

impl Entry {
    /// The name as DOS shows it, e.g. `SAMPLE.LST` (no dot when there is
    /// no extension). Bytes other than printable ASCII show as `?`.
    pub fn file_name(&self) -> String {
        crate::names::display(&self.name, &self.ext)
    }

    /// The name as stored, `NAME.EXT` in ATASCII: what a path gives to
    /// find it.
    pub fn stored_name(&self) -> Vec<u8> {
        crate::names::joined(&self.name, &self.ext)
    }

    /// Locked files can't be written, renamed or deleted from DOS.
    pub fn locked(&self) -> bool {
        self.flags & LOCKED != 0
    }

    /// A MyDOS subdirectory.
    pub fn is_dir(&self) -> bool {
        self.flags & SUBDIR != 0
    }

    /// The file was opened for writing and never closed (the Atari was
    /// switched off or crashed), so its length may be wrong.
    pub fn open_for_writing(&self) -> bool {
        self.flags & (IN_USE | OPEN) == IN_USE | OPEN
    }

    /// A DOS 2.5 file using sectors above 719 (flags `$03`): DOS 2.5
    /// shows it in angle brackets, and DOS 2.0 doesn't see it.
    pub fn above_720(&self) -> bool {
        self.flags & (IN_USE | MADE_BY_DOS2 | OPEN) == MADE_BY_DOS2 | OPEN
    }

    /// MyDOS 16-bit sector links (flag `$04`), with no file number.
    pub fn big_links(&self) -> bool {
        self.flags & BIG_LINKS != 0
    }
}

impl<'a> Dos2<'a> {
    /// Read `image` as a DOS 2 or MyDOS disk. Fails with
    /// [`Error::UnknownFileSystem`] when sector 360 doesn't look like a
    /// DOS 2 VTOC.
    pub fn open(image: &'a DiskImage) -> Result<Self> {
        let not = |why: &str| Err(Error::UnknownFileSystem(format!("not DOS 2: {why}")));
        let count = image.sector_count();
        if count < DIR_FIRST + DIR_SECTORS - 1 {
            return not("too few sectors");
        }
        let size = image.sector_size();
        if size > 256 {
            return not("DOS 2 has no 512-byte sectors");
        }
        let vtoc = image.sector(VTOC)?;
        let total = u32::from(word(vtoc, 1));
        let free = u32::from(word(vtoc, 3));
        let (flavour, vtoc_sectors) = match vtoc[0] {
            2 if size == 128 && count >= VTOC2 && total > 720 => (Flavour::Enhanced, 1),
            // MyDOS uses every sector: all but boot, VTOC and directory.
            2 if total == count - 12 => (Flavour::MyDos, 1),
            2 => (Flavour::Dos2, 1),
            code @ 3..=0x40 => (Flavour::MyDos, u32::from(code - 2) * (256 / size as u32)),
            _ => return not("sector 360 is not a DOS 2 VTOC"),
        };
        if total == 0 || total > count || free > total {
            return not("the VTOC's sector counts don't fit the disk");
        }
        // A MyDOS bitmap must cover the whole disk. (DOS 2's VTOC always
        // covers what its 10-bit links reach, with VTOC2 on enhanced disks.)
        let covered = (vtoc_sectors as usize * size - BITMAP) * 8;
        if vtoc_sectors >= VTOC - 3 || (flavour == Flavour::MyDos && covered <= count as usize) {
            return not("the VTOC's bitmap doesn't fit the disk");
        }
        let mut fs = Self {
            image,
            geometry: Geometry {
                flavour,
                vtoc_sectors,
                size,
                count,
            },
        };
        // A MyDOS directory or 16-bit links in the main directory mark a
        // MyDOS disk that still says code 2 and DOS 2's sector count.
        if flavour == Flavour::Dos2
            && fs
                .raw_entries(DIR_FIRST)
                .take_while(|(_, raw)| raw[0] != 0)
                .any(|(_, raw)| raw[0] & DELETED == 0 && raw[0] & (SUBDIR | BIG_LINKS) != 0)
        {
            fs.geometry.flavour = Flavour::MyDos;
        }
        Ok(fs)
    }

    /// The disk the file system is on.
    pub fn image(&self) -> &'a DiskImage {
        self.image
    }

    pub(crate) fn flavour(&self) -> Flavour {
        self.geometry.flavour
    }

    /// Is this a DOS 2.5 enhanced-density disk (with a second VTOC)?
    pub fn enhanced(&self) -> bool {
        self.geometry.flavour == Flavour::Enhanced
    }

    /// Is this a MyDOS disk?
    pub fn mydos(&self) -> bool {
        self.geometry.flavour == Flavour::MyDos
    }

    /// The sectors the VTOC takes, from 360 down (1 except on large
    /// MyDOS disks).
    pub fn vtoc_sectors(&self) -> u32 {
        self.geometry.vtoc_sectors
    }

    /// Free sectors, as DOS 2.5 counts them (on enhanced-density disks
    /// DOS 2.0 sees only [`Dos2::vtoc_free_sectors`]).
    pub fn free_sectors(&self) -> u32 {
        let mut free = self.vtoc_free_sectors();
        if self.enhanced()
            && let Ok(vtoc2) = self.image.sector(VTOC2)
        {
            free += u32::from(word(vtoc2, 122));
        }
        free
    }

    /// The free-sector count in the VTOC at sector 360: everything DOS
    /// 2.0 knows about. On an enhanced disk this leaves out sectors 720
    /// and up, which only DOS 2.5 uses.
    pub fn vtoc_free_sectors(&self) -> u32 {
        self.image.sector(VTOC).map_or(0, |v| u32::from(word(v, 3)))
    }

    /// The sectors DOS can use, from the VTOC (707 on a single-density
    /// disk, 1010 enhanced, 708 under MyDOS).
    pub fn total_sectors(&self) -> u32 {
        self.image.sector(VTOC).map_or(0, |v| u32::from(word(v, 1)))
    }

    /// Is sector `n` marked free in the bitmap?
    pub fn is_free(&self, n: u32) -> bool {
        n <= self.geometry.count
            && self.geometry.bit_at(n).is_some_and(|(sector, byte, mask)| {
                self.image
                    .sector(sector)
                    .ok()
                    .and_then(|s| s.get(byte))
                    .is_some_and(|b| b & mask != 0)
            })
    }

    /// The first sector of the main directory.
    pub fn root(&self) -> u32 {
        DIR_FIRST
    }

    /// The files (and MyDOS directories) in the main directory, in
    /// directory order. Deleted entries are left out.
    pub fn list(&self) -> Vec<Entry> {
        self.list_dir(DIR_FIRST).unwrap_or_default()
    }

    /// The entries in the directory starting at sector `dir` (see
    /// [`Dos2::root`] and, for a MyDOS directory, [`Entry::start`]).
    /// Fails with [`Error::Damaged`] if its 8 sectors aren't all on the
    /// disk.
    pub fn list_dir(&self, dir: u32) -> Result<Vec<Entry>> {
        if dir <= 3 || dir + DIR_SECTORS - 1 > self.geometry.count {
            return Err(Error::Damaged(format!(
                "a directory at sector {dir} runs off the disk"
            )));
        }
        let mut out = Vec::new();
        for (index, raw) in self.raw_entries(dir) {
            let flags = raw[0];
            if flags == 0 {
                break;
            }
            let dos25 = flags & (IN_USE | MADE_BY_DOS2 | OPEN) == MADE_BY_DOS2 | OPEN;
            let subdir = self.mydos() && flags & SUBDIR != 0;
            if flags & DELETED != 0 || (flags & IN_USE == 0 && !dos25 && !subdir) {
                continue;
            }
            let mut entry = Entry {
                dir,
                index,
                name: raw[5..13].try_into().unwrap(),
                ext: raw[13..16].try_into().unwrap(),
                flags,
                sectors: word(raw, 1),
                start: word(raw, 3),
                size: 0,
                damaged: false,
            };
            if subdir {
                let start = u32::from(entry.start);
                entry.damaged = start <= 3 || start + DIR_SECTORS - 1 > self.geometry.count;
            } else {
                let mut size = 0;
                entry.damaged = self
                    .walk(&entry, |_, data| size += data.len() as u32)
                    .is_err();
                entry.size = size;
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// The first sector of directory `path` (`""` for the main one, or
    /// e.g. `"GAMES>OLD"` on MyDOS). Names are matched with `same`.
    pub(crate) fn dir_first(
        &self,
        path: &[u8],
        same: impl Fn(&Entry, &[u8]) -> bool,
    ) -> Result<u32> {
        let text = || crate::atascii::atascii_to_ascii(path);
        let mut dir = DIR_FIRST;
        for part in path.split(|&c| c == b'>').filter(|p| !p.is_empty()) {
            let entry = self
                .list_dir(dir)?
                .into_iter()
                .find(|e| same(e, part))
                .ok_or_else(|| Error::NotFound(text()))?;
            if !entry.is_dir() {
                return Err(Error::NotADirectory(text()));
            }
            if entry.damaged {
                return Err(Error::Damaged(format!("directory {}", entry.file_name())));
            }
            dir = u32::from(entry.start);
        }
        Ok(dir)
    }

    /// The file called `name` in the main directory (e.g. `"SAMPLE.LST"`,
    /// as ATASCII), ignoring case.
    pub fn find(&self, name: impl AsRef<[u8]>) -> Option<Entry> {
        self.list()
            .into_iter()
            .find(|e| e.stored_name().eq_ignore_ascii_case(name.as_ref()))
    }

    /// The contents of a file. Fails with [`Error::Damaged`] if a sector
    /// of the file is outside the disk, belongs to another file (by its
    /// file number), claims more data than a sector can hold, or leads
    /// back into the chain; [`Error::IsDirectory`] for a directory.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        if entry.is_dir() && self.mydos() {
            return Err(Error::IsDirectory(entry.file_name()));
        }
        let mut out = Vec::with_capacity(entry.size as usize);
        self.walk(entry, |_, data| out.extend_from_slice(data))?;
        Ok(out)
    }

    /// The sectors a file uses, in order; for a MyDOS directory, its 8.
    pub fn sectors_of(&self, entry: &Entry) -> Result<Vec<u32>> {
        if entry.is_dir() && self.mydos() {
            let start = u32::from(entry.start);
            return Ok((start..start + DIR_SECTORS).collect());
        }
        let mut out = Vec::new();
        self.walk(entry, |n, _| out.push(n))?;
        Ok(out)
    }

    /// The sectors of a file that could be followed, and what went
    /// wrong if not all of them could.
    pub(crate) fn chain(&self, entry: &Entry) -> (Vec<u32>, Result<()>) {
        let mut out = Vec::new();
        let result = self.walk(entry, |n, _| out.push(n));
        (out, result)
    }

    /// Is sector `n` one DOS 2 keeps out of use (720 on DOS 2.0/2.5) or
    /// beyond what its bitmap covers?
    pub(crate) fn reserved(&self, n: u32) -> bool {
        let g = self.geometry;
        match g.flavour {
            Flavour::MyDos => false,
            _ => n == 720 || n > VTOC2 || (g.flavour == Flavour::Dos2 && n >= VTOC2),
        }
    }

    /// The sectors of the VTOC (and VTOC2).
    pub(crate) fn vtoc_list(&self) -> Vec<u32> {
        let g = self.geometry;
        let mut out: Vec<u32> = (VTOC + 1 - g.vtoc_sectors..=VTOC).collect();
        if g.flavour == Flavour::Enhanced {
            out.push(VTOC2);
        }
        out
    }

    /// The highest sector the bitmap can mark (for counting free bits).
    pub(crate) fn last_marked(&self) -> u32 {
        match self.geometry.flavour {
            Flavour::MyDos => self.geometry.count,
            _ => self.geometry.count.min(VTOC2 - 1),
        }
    }

    /// Every slot of the directory starting at sector `dir`, with its
    /// index, whatever its flags.
    fn raw_entries(&self, dir: u32) -> impl Iterator<Item = (u8, &'a [u8])> + 'a {
        let image = self.image;
        (0..DIR_SECTORS).flat_map(move |s| {
            let sector = image.sector(dir + s).unwrap_or(&[]);
            sector
                .as_chunks::<ENTRY_LEN>()
                .0
                .iter()
                .take(ENTRIES_PER_SECTOR)
                .enumerate()
                .map(move |(i, raw)| ((s as usize * ENTRIES_PER_SECTOR + i) as u8, &raw[..]))
        })
    }

    /// Follow a file's sectors, handing each one's number and data to
    /// `each`. A sector is handed over only once it has passed the checks.
    fn walk(&self, entry: &Entry, mut each: impl FnMut(u32, &[u8])) -> Result<()> {
        let name = entry.file_name();
        let damaged = |why: String| Err(Error::Damaged(format!("{name}: {why}")));
        let data_len = self.image.sector_size() - 3;
        let big = entry.big_links();
        let mut n = u32::from(entry.start);
        // A chain can't be longer than the disk without going round in a
        // loop.
        let mut steps = 0;
        while n != 0 {
            steps += 1;
            if steps > self.image.sector_count() {
                return damaged("its sectors go round in a loop".into());
            }
            if n <= 3 || n > self.image.sector_count() {
                return damaged(format!("sector {n} is not a data sector on this disk"));
            }
            let sector = self.image.sector(n)?;
            let link = &sector[data_len..];
            if !big {
                let file = link[0] >> 2;
                if file != entry.index {
                    return damaged(format!(
                        "sector {n} belongs to file number {file}, not {}",
                        entry.index
                    ));
                }
            }
            let count = usize::from(link[2]);
            if count > data_len {
                return damaged(format!("sector {n} claims {count} bytes"));
            }
            each(n, &sector[..count]);
            n = if big {
                u32::from(link[0]) << 8 | u32::from(link[1])
            } else {
                u32::from(link[0] & 3) << 8 | u32::from(link[1])
            };
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Changing a disk. These work on the image directly. `Volume` calls them
// on a copy and keeps the copy only if they succeed, so a failure part
// way through leaves the disk as it was.

fn geometry(image: &DiskImage) -> Result<Geometry> {
    Ok(Dos2::open(image)?.geometry)
}

/// Mark sector `n` free or in use, keeping the free counts (and, on an
/// enhanced disk, the second VTOC's copy of the bitmap) in step.
fn mark(image: &mut DiskImage, g: Geometry, n: u32, free: bool) -> Result<()> {
    let outside = || Error::Damaged(format!("sector {n} is outside the VTOC"));
    let (sector, byte, mask) = g.bit_at(n).ok_or_else(outside)?;
    let was_free = image
        .sector(sector)?
        .get(byte)
        .is_some_and(|b| b & mask != 0);
    if was_free == free {
        return Ok(());
    }
    let enhanced = g.flavour == Flavour::Enhanced;
    let mut places = vec![(sector, byte)];
    if enhanced && (48..720).contains(&n) {
        places.push((VTOC2, n as usize / 8 - 6));
    }
    for (sector, byte) in places {
        let b = image
            .sector_mut(sector)?
            .get_mut(byte)
            .ok_or_else(outside)?;
        if free {
            *b |= mask;
        } else {
            *b &= !mask;
        }
    }
    let (sector, at) = if enhanced && n >= 720 {
        (VTOC2, 122)
    } else {
        (VTOC, 3)
    };
    let counts = image.sector_mut(sector)?;
    let count = word(counts, at);
    let count = if free {
        count.saturating_add(1)
    } else {
        count.saturating_sub(1)
    };
    counts[at..at + 2].copy_from_slice(&count.to_le_bytes());
    Ok(())
}

/// The free data sectors, lowest first (as DOS 2 takes them). Sectors DOS
/// keeps for itself are left out even if the bitmap says they're free.
fn free_list(image: &DiskImage) -> Result<Vec<u32>> {
    let fs = Dos2::open(image)?;
    let g = fs.geometry;
    Ok((4..=g.last_usable())
        .filter(|&n| !g.reserved(n) && fs.is_free(n))
        .collect())
}

/// A directory entry's 16 bytes, to change.
fn raw_entry_mut(image: &mut DiskImage, dir: u32, index: u8) -> Result<&mut [u8]> {
    let sector = image.sector_mut(dir + u32::from(index) / ENTRIES_PER_SECTOR as u32)?;
    let at = usize::from(index) % ENTRIES_PER_SECTOR * ENTRY_LEN;
    Ok(&mut sector[at..at + ENTRY_LEN])
}

/// Mark the sectors of `entry` free. Only the sectors that pass the
/// checks in [`Dos2::read`] are freed: in a damaged chain, the sectors
/// from the damage on may belong to another file.
fn free_chain(image: &mut DiskImage, entry: &Entry) -> Result<()> {
    let fs = Dos2::open(image)?;
    let g = fs.geometry;
    let mut sectors = Vec::new();
    // Damage just ends the list.
    let _ = fs.walk(entry, |n, _| sectors.push(n));
    for n in sectors {
        mark(image, g, n, true)?;
    }
    Ok(())
}

/// Write a file called `name` holding `data` into the directory starting
/// at sector `dir`. If `old` is given (a file of the same name), it's
/// replaced and keeps its place in the directory; otherwise the first
/// unused or deleted entry is taken.
pub(crate) fn write(
    image: &mut DiskImage,
    dir: u32,
    old: Option<&Entry>,
    name: &Name,
    data: &[u8],
) -> Result<()> {
    let g = geometry(image)?;
    let index = match old {
        Some(old) => {
            free_chain(image, old)?;
            old.index
        }
        None => free_entry(image, dir)?,
    };
    let per_sector = image.sector_size() - 3;
    // DOS 2 gives even an empty file one sector.
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(per_sector).collect()
    };
    let free = free_list(image)?;
    if free.len() < chunks.len() {
        return Err(Error::DiskFull {
            needed: chunks.len() as u32,
            free: free.len() as u32,
        });
    }
    let big = g.big_links();
    for (i, chunk) in chunks.iter().enumerate() {
        let n = free[i];
        let next = if i + 1 < chunks.len() { free[i + 1] } else { 0 };
        let sector = image.sector_mut(n)?;
        sector.fill(0);
        sector[..chunk.len()].copy_from_slice(chunk);
        sector[per_sector] = if big {
            (next >> 8) as u8
        } else {
            index << 2 | (next >> 8) as u8
        };
        sector[per_sector + 1] = next as u8;
        sector[per_sector + 2] = chunk.len() as u8;
        mark(image, g, n, false)?;
    }
    let used = &free[..chunks.len()];
    let raw = raw_entry_mut(image, dir, index)?;
    raw[0] = match g.flavour {
        // As DOS 2.5 does: a file reaching above sector 719 is hidden
        // from DOS 2.0, which can't free those sectors.
        Flavour::Enhanced if used.iter().any(|&n| n >= 720) => MADE_BY_DOS2 | OPEN,
        _ if big => IN_USE | BIG_LINKS | MADE_BY_DOS2,
        _ => IN_USE | MADE_BY_DOS2,
    };
    raw[1..3].copy_from_slice(&(chunks.len() as u16).to_le_bytes());
    raw[3..5].copy_from_slice(&(free[0] as u16).to_le_bytes());
    raw[5..16].copy_from_slice(name);
    Ok(())
}

/// The first entry of the directory at `dir` that's unused or deleted.
fn free_entry(image: &DiskImage, dir: u32) -> Result<u8> {
    Dos2::open(image)?
        .raw_entries(dir)
        .find(|(_, raw)| raw[0] == 0 || raw[0] & DELETED != 0)
        .map(|(index, _)| index)
        .ok_or(Error::DirectoryFull)
}

/// Make an empty MyDOS directory called `name` in the directory at
/// `dir`: 8 free sectors in a row, the lowest there are.
pub(crate) fn make_dir(image: &mut DiskImage, dir: u32, name: &Name) -> Result<()> {
    let fs = Dos2::open(image)?;
    let g = fs.geometry;
    if g.flavour != Flavour::MyDos {
        return Err(Error::Unsupported("DOS 2 has no directories".into()));
    }
    let index = free_entry(image, dir)?;
    let usable = |n: u32| !g.reserved(n) && fs.is_free(n);
    let start = (4..=g.count.saturating_sub(DIR_SECTORS - 1))
        .find(|&s| (s..s + DIR_SECTORS).all(usable))
        .ok_or(Error::DiskFull {
            needed: DIR_SECTORS,
            free: fs.free_sectors(),
        })?;
    for n in start..start + DIR_SECTORS {
        image.sector_mut(n)?.fill(0);
        mark(image, g, n, false)?;
    }
    let raw = raw_entry_mut(image, dir, index)?;
    raw[0] = SUBDIR;
    raw[1..3].copy_from_slice(&(DIR_SECTORS as u16).to_le_bytes());
    raw[3..5].copy_from_slice(&(start as u16).to_le_bytes());
    raw[5..16].copy_from_slice(name);
    Ok(())
}

/// Delete a file, or an empty MyDOS directory
/// ([`Error::DirectoryNotEmpty`] otherwise): free its sectors and mark its
/// entry deleted. As in DOS 2, the rest of the entry is left.
pub(crate) fn delete(image: &mut DiskImage, entry: &Entry) -> Result<()> {
    let fs = Dos2::open(image)?;
    if entry.is_dir() && fs.mydos() {
        if !fs.list_dir(u32::from(entry.start))?.is_empty() {
            return Err(Error::DirectoryNotEmpty(entry.file_name()));
        }
        let g = fs.geometry;
        let start = u32::from(entry.start);
        for n in start..start + DIR_SECTORS {
            mark(image, g, n, true)?;
        }
    } else {
        free_chain(image, entry)?;
    }
    raw_entry_mut(image, entry.dir, entry.index)?[0] = DELETED;
    Ok(())
}

/// Move an entry into the directory at `dir` (MyDOS). A file with
/// 10-bit links gets its new file number written in every sector.
pub(crate) fn move_entry(image: &mut DiskImage, entry: &Entry, dir: u32) -> Result<()> {
    let fs = Dos2::open(image)?;
    if entry.is_dir() {
        // Not into itself or a directory inside it.
        let start = u32::from(entry.start);
        let mut below = vec![start];
        while let Some(d) = below.pop() {
            if d == dir {
                return Err(Error::Unsupported(format!(
                    "moving {} into itself",
                    entry.file_name()
                )));
            }
            if below.len() > 4096 {
                return Err(Error::Damaged("directories go round in a loop".into()));
            }
            below.extend(
                fs.list_dir(d)?
                    .iter()
                    .filter(|e| e.is_dir())
                    .map(|e| u32::from(e.start)),
            );
        }
    }
    let sectors = if entry.is_dir() || entry.big_links() {
        Vec::new()
    } else {
        fs.sectors_of(entry)?
    };
    let index = free_entry(image, dir)?;
    let mut raw = [0; ENTRY_LEN];
    raw.copy_from_slice(raw_entry_mut(image, entry.dir, entry.index)?);
    raw_entry_mut(image, dir, index)?.copy_from_slice(&raw);
    raw_entry_mut(image, entry.dir, entry.index)?[0] = DELETED;
    let link = image.sector_size() - 3;
    for n in sectors {
        let sector = image.sector_mut(n)?;
        sector[link] = index << 2 | (sector[link] & 3);
    }
    Ok(())
}

/// Give a file a new name (already checked).
pub(crate) fn rename(image: &mut DiskImage, entry: &Entry, name: &Name) -> Result<()> {
    raw_entry_mut(image, entry.dir, entry.index)?[5..16].copy_from_slice(name);
    Ok(())
}

/// Lock or unlock a file.
pub(crate) fn set_locked(image: &mut DiskImage, entry: &Entry, locked: bool) -> Result<()> {
    let raw = raw_entry_mut(image, entry.dir, entry.index)?;
    if locked {
        raw[0] |= LOCKED;
    } else {
        raw[0] &= !LOCKED;
    }
    Ok(())
}

/// The DOS 2 disk layouts [`format`] can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// 720 × 128 bytes, 707 free (DOS 2.0S, 2.5).
    Single,
    /// 720 × 256 bytes, 707 free (DOS 2.0D).
    Double,
    /// 1040 × 128 bytes, 1010 free (DOS 2.5).
    Enhanced,
    /// MyDOS: any number of sectors of 128 or 256 bytes.
    MyDos { size: usize, count: u32 },
}

/// The boot sector of a blank data disk: a few bytes of our own (DOS's
/// boot code can't be shipped) that make the Atari behave as with a
/// disk formatted by DOS but without DOS on it. The OS loads 1 sector at
/// $0700 and calls $0706, which sets the carry flag and returns. The OS
/// takes that as a failed boot, prints BOOT ERROR and tries again.
const DATA_DISK_BOOT: [u8; 8] = [
    0x00, // flags
    0x01, // sectors to load
    0x00, 0x07, // load at $0700
    0x07, 0x07, // DOSINI: the RTS below (not used, as the boot fails)
    0x38, // $0706: SEC
    0x60, // $0707: RTS
];

/// A blank DOS 2 or MyDOS data disk: VTOC, an empty directory, and a
/// boot sector that gives BOOT ERROR ([`DATA_DISK_BOOT`]).
pub(crate) fn format(container: crate::Container, kind: Kind) -> Result<DiskImage> {
    let (size, count) = match kind {
        Kind::Single => (128, 720),
        Kind::Double => (256, 720),
        Kind::Enhanced => (128, 1040),
        Kind::MyDos { size, count } => {
            if !(DIR_FIRST + DIR_SECTORS..=0xFFFF).contains(&count) || size > 256 {
                return Err(Error::Unsupported(format!(
                    "MyDOS disks have 369 to 65535 sectors of 128 or 256 bytes, not {count} of {size}"
                )));
            }
            (size, count)
        }
    };
    let (flavour, code, vtoc_sectors, total) = match kind {
        Kind::Single | Kind::Double => (Flavour::Dos2, 2, 1, 707),
        Kind::Enhanced => (Flavour::Enhanced, 2, 1, 1010),
        Kind::MyDos { .. } => {
            let bitmap = BITMAP + (count as usize + 1).div_ceil(8);
            let (code, sectors) = if bitmap <= size {
                (2, 1)
            } else {
                let units = bitmap.div_ceil(256);
                (2 + units as u8, (units * 256 / size) as u32)
            };
            (
                Flavour::MyDos,
                code,
                sectors,
                count - 3 - sectors - DIR_SECTORS,
            )
        }
    };
    let g = Geometry {
        flavour,
        vtoc_sectors,
        size,
        count,
    };
    let mut image = DiskImage::new(container, size, count)?;
    image.sector_mut(1)?[..DATA_DISK_BOOT.len()].copy_from_slice(&DATA_DISK_BOOT);
    let vtoc = image.sector_mut(VTOC)?;
    vtoc[0] = code;
    vtoc[1..3].copy_from_slice(&(total as u16).to_le_bytes());
    // `mark` counts the free sectors up from 0. DOS 2 leaves sector 720
    // in use (the 810 drive can't always read it), and on an enhanced
    // disk 1024, the second VTOC; MyDOS uses both.
    let last = if flavour == Flavour::MyDos {
        count
    } else {
        count.min(VTOC2)
    };
    for n in (4..=last).filter(|&n| !g.reserved(n)) {
        mark(&mut image, g, n, true)?;
    }
    Ok(image)
}

/// A little-endian 16-bit number at `at`.
fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Container;

    /// A blank single-density disk with a VTOC and no files.
    fn blank() -> DiskImage {
        let mut image = DiskImage::new(Container::Atr, 128, 720).unwrap();
        let vtoc = image.sector_mut(VTOC).unwrap();
        vtoc[0] = 2;
        vtoc[1..3].copy_from_slice(&707u16.to_le_bytes());
        vtoc[3..5].copy_from_slice(&707u16.to_le_bytes());
        image
    }

    /// Add file `index` called `name`.`ext` holding `data`, in sectors
    /// from `first` up.
    fn add(image: &mut DiskImage, index: u8, name: &[u8; 11], first: u32, data: &[u8]) {
        let chunks: Vec<&[u8]> = data.chunks(125).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let n = first + i as u32;
            let next = if i + 1 < chunks.len() { n + 1 } else { 0 };
            let sector = image.sector_mut(n).unwrap();
            sector[..chunk.len()].copy_from_slice(chunk);
            sector[125] = index << 2 | (next >> 8) as u8;
            sector[126] = next as u8;
            sector[127] = chunk.len() as u8;
        }
        let dir = image.sector_mut(DIR_FIRST + u32::from(index) / 8).unwrap();
        let raw = &mut dir[usize::from(index % 8) * 16..][..16];
        raw[0] = 0x42;
        raw[1..3].copy_from_slice(&(chunks.len() as u16).to_le_bytes());
        raw[3..5].copy_from_slice(&(first as u16).to_le_bytes());
        raw[5..16].copy_from_slice(name);
    }

    #[test]
    fn lists_and_reads_files() {
        let mut image = blank();
        let text: Vec<u8> = (0..300).map(|i| i as u8).collect();
        add(&mut image, 0, b"SAMPLE  LST", 4, &text);
        add(&mut image, 1, b"NOEXT      ", 10, b"hi");
        let fs = Dos2::open(&image).unwrap();
        let list = fs.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].file_name(), "SAMPLE.LST");
        assert_eq!((list[0].sectors, list[0].size), (3, 300));
        assert_eq!(list[1].file_name(), "NOEXT");
        assert!(!list[0].damaged && !list[0].locked());
        assert_eq!(fs.read(&list[0]).unwrap(), text);
        assert_eq!(fs.read(&fs.find("noext").unwrap()).unwrap(), b"hi");
        assert!(fs.find("MISSING").is_none());
    }

    #[test]
    fn deleted_entries_are_skipped_and_unused_ones_end_the_list() {
        let mut image = blank();
        add(&mut image, 0, b"GONE       ", 4, b"x");
        add(&mut image, 1, b"KEPT       ", 5, b"y");
        add(&mut image, 3, b"AFTERGAP   ", 6, b"z");
        image.sector_mut(DIR_FIRST).unwrap()[0] = DELETED;
        let names: Vec<String> = Dos2::open(&image)
            .unwrap()
            .list()
            .iter()
            .map(Entry::file_name)
            .collect();
        assert_eq!(names, ["KEPT"]);
    }

    #[test]
    fn damaged_chains_are_reported() {
        let mut image = blank();
        add(&mut image, 0, b"LOOP       ", 4, &[1; 250]);
        // Point the second sector back at the first.
        image.sector_mut(5).unwrap()[126] = 4;
        add(&mut image, 1, b"WRONGNUM   ", 10, b"abc");
        image.sector_mut(10).unwrap()[125] = 5 << 2;
        let fs = Dos2::open(&image).unwrap();
        for entry in fs.list() {
            assert!(entry.damaged, "{}", entry.file_name());
            assert!(matches!(fs.read(&entry), Err(Error::Damaged(_))));
        }
    }

    #[test]
    fn free_sectors_and_bitmap() {
        let mut image = blank();
        image.sector_mut(VTOC).unwrap()[BITMAP + 1] = 0b0100_0000; // sector 9
        let fs = Dos2::open(&image).unwrap();
        assert_eq!(fs.free_sectors(), 707);
        assert!(fs.is_free(9));
        assert!(!fs.is_free(8) && !fs.is_free(10));
    }

    #[test]
    fn other_disks_are_refused() {
        let image = DiskImage::new(Container::Atr, 128, 720).unwrap();
        assert!(matches!(
            Dos2::open(&image),
            Err(Error::UnknownFileSystem(_))
        ));
        let small = DiskImage::new(Container::Atr, 128, 100).unwrap();
        assert!(Dos2::open(&small).is_err());
    }
}
