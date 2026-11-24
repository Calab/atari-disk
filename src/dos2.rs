//! Atari DOS 2 file systems: DOS 2.0S, 2.0D and 2.5 (single, double and
//! enhanced density).
//!
//! # Layout
//! * Sectors 1–3: boot sectors; DOS.SYS follows from sector 4.
//! * Sector 360: the VTOC (volume table of contents):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | DOS code: 2 |
//!   | 1–2 | Sectors DOS can use (707 single density, 1010 enhanced) |
//!   | 3–4 | Free sectors (on enhanced disks, of sectors 1–719 only) |
//!   | 10–99 | Bitmap of sectors 0–719: bit 7 of byte 10 is sector 0; 1 = free |
//!
//! * Sectors 361–368: the directory, 8 entries of 16 bytes per sector
//!   (64 files):
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | Flags: `$80` deleted, `$40` in use, `$20` locked, `$02` made by DOS 2, `$01` open for writing (or see below) |
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

use crate::error::{Error, Result};
use crate::image::DiskImage;

const VTOC: u32 = 360;
const VTOC2: u32 = 1024;
const DIR_FIRST: u32 = 361;
const DIR_SECTORS: u32 = 8;
const ENTRY_LEN: usize = 16;
const ENTRIES_PER_SECTOR: usize = 8;
/// Bitmap byte 10 holds sectors 0–7.
const BITMAP: usize = 10;

/// Directory entry flags.
const DELETED: u8 = 0x80;
const IN_USE: u8 = 0x40;
const LOCKED: u8 = 0x20;
const MADE_BY_DOS2: u8 = 0x02;
const OPEN: u8 = 0x01;

/// A DOS 2 disk, read from a [`DiskImage`].
#[derive(Debug, Clone)]
pub struct Dos2<'a> {
    image: &'a DiskImage,
    /// DOS 2.5 enhanced density, with a second VTOC in sector 1024.
    enhanced: bool,
}

/// One file in a DOS 2 directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where the entry is in the directory, 0–63. DOS also writes it in
    /// every sector of the file, as the file number.
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
    /// The length in bytes, found by following the file's sectors.
    pub size: u32,
    /// Following the file's sectors went wrong (see [`Dos2::read`] for
    /// why); `size` counts the bytes up to there.
    pub damaged: bool,
}

impl Entry {
    /// The name as DOS shows it, e.g. `SAMPLE.LST` (no dot when there is
    /// no extension). Bytes other than printable ASCII show as `?`.
    pub fn file_name(&self) -> String {
        let name = text(&self.name);
        let ext = text(&self.ext);
        if ext.is_empty() {
            name
        } else {
            format!("{name}.{ext}")
        }
    }

    /// Locked files can't be written, renamed or deleted from DOS.
    pub fn locked(&self) -> bool {
        self.flags & LOCKED != 0
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
}

/// Trimmed, printable text from a name field.
fn text(bytes: &[u8]) -> String {
    let s: String = bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect();
    s.trim_end().to_string()
}

impl<'a> Dos2<'a> {
    /// Read `image` as a DOS 2 disk. Fails with
    /// [`Error::UnknownFileSystem`] when sector 360 doesn't look like a
    /// DOS 2 VTOC.
    pub fn open(image: &'a DiskImage) -> Result<Self> {
        let not = |why: &str| Err(Error::UnknownFileSystem(format!("not DOS 2: {why}")));
        if image.sector_count() < DIR_FIRST + DIR_SECTORS - 1 {
            return not("too few sectors");
        }
        let vtoc = image.sector(VTOC)?;
        if vtoc[0] != 2 {
            return not("sector 360 is not a DOS 2 VTOC");
        }
        let total = word(vtoc, 1);
        let free = word(vtoc, 3);
        if total == 0 || u32::from(total) > image.sector_count() || free > total {
            return not("the VTOC's sector counts don't fit the disk");
        }
        let enhanced = image.sector_size() == 128 && image.sector_count() >= VTOC2 && total > 720;
        Ok(Self { image, enhanced })
    }

    /// The disk the file system is on.
    pub fn image(&self) -> &'a DiskImage {
        self.image
    }

    /// Is this a DOS 2.5 enhanced-density disk (with a second VTOC)?
    pub fn enhanced(&self) -> bool {
        self.enhanced
    }

    /// Free sectors, as DOS 2.5 counts them (on enhanced-density disks
    /// DOS 2.0 sees only [`Dos2::vtoc_free_sectors`]).
    pub fn free_sectors(&self) -> u32 {
        let mut free = self.vtoc_free_sectors();
        if self.enhanced
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
    /// disk, 1010 on an enhanced one).
    pub fn total_sectors(&self) -> u32 {
        self.image.sector(VTOC).map_or(0, |v| u32::from(word(v, 1)))
    }

    /// Is sector `n` marked free in the VTOC (or, from 720 up on an
    /// enhanced disk, the second VTOC)?
    pub fn is_free(&self, n: u32) -> bool {
        let (sector, byte, mask) = bit_at(self.enhanced, n);
        self.image
            .sector(sector)
            .ok()
            .and_then(|s| s.get(byte))
            .is_some_and(|b| b & mask != 0)
    }

    /// The files in the directory, in directory order. Deleted entries
    /// are left out.
    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (index, raw) in self.raw_entries() {
            let flags = raw[0];
            if flags == 0 {
                break;
            }
            let dos25 = flags & (IN_USE | MADE_BY_DOS2 | OPEN) == MADE_BY_DOS2 | OPEN;
            if flags & DELETED != 0 || (flags & IN_USE == 0 && !dos25) {
                continue;
            }
            let mut entry = Entry {
                index,
                name: raw[5..13].try_into().unwrap(),
                ext: raw[13..16].try_into().unwrap(),
                flags,
                sectors: word(raw, 1),
                start: word(raw, 3),
                size: 0,
                damaged: false,
            };
            let mut size = 0;
            entry.damaged = self
                .walk(&entry, |_, data| size += data.len() as u32)
                .is_err();
            entry.size = size;
            out.push(entry);
        }
        out
    }

    /// The file called `name` (e.g. `"SAMPLE.LST"`), ignoring case.
    pub fn find(&self, name: &str) -> Option<Entry> {
        self.list()
            .into_iter()
            .find(|e| e.file_name().eq_ignore_ascii_case(name))
    }

    /// The contents of a file. Fails with [`Error::Damaged`] if a sector
    /// of the file is outside the disk, belongs to another file (by its
    /// file number), claims more data than a sector can hold, or leads
    /// back into the chain.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size as usize);
        self.walk(entry, |_, data| out.extend_from_slice(data))?;
        Ok(out)
    }

    /// Every directory slot, with its index, whatever its flags.
    fn raw_entries(&self) -> impl Iterator<Item = (u8, &'a [u8])> + 'a {
        let image = self.image;
        (0..DIR_SECTORS).flat_map(move |s| {
            // The directory sectors exist: `open` checked the count.
            let sector = image.sector(DIR_FIRST + s).unwrap_or(&[]);
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
            let file = link[0] >> 2;
            if file != entry.index {
                return damaged(format!(
                    "sector {n} belongs to file number {file}, not {}",
                    entry.index
                ));
            }
            let count = usize::from(link[2]);
            if count > data_len {
                return damaged(format!("sector {n} claims {count} bytes"));
            }
            each(n, &sector[..count]);
            n = u32::from(link[0] & 3) << 8 | u32::from(link[1]);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Changing a disk. These work on the image directly. `Volume` calls them
// on a copy and keeps the copy only if they succeed, so a failure part
// way through leaves the disk as it was.

/// Where sector `n`'s bit is in the bitmaps: (sector, byte, mask).
fn bit_at(enhanced: bool, n: u32) -> (u32, usize, u8) {
    let mask = 0x80 >> (n % 8);
    if enhanced && (720..VTOC2).contains(&n) {
        (VTOC2, n as usize / 8 - 6, mask)
    } else {
        // Also sector 720 on MyDOS-style double-density disks, in byte
        // 100 of the VTOC.
        (VTOC, BITMAP + n as usize / 8, mask)
    }
}

/// Mark sector `n` free or in use, keeping the free counts (and, on an
/// enhanced disk, the second VTOC's copy of the bitmap) in step.
fn mark(image: &mut DiskImage, enhanced: bool, n: u32, free: bool) -> Result<()> {
    let (sector, byte, mask) = bit_at(enhanced, n);
    let was_free = image
        .sector(sector)?
        .get(byte)
        .is_some_and(|b| b & mask != 0);
    if was_free == free {
        return Ok(());
    }
    let mut places = vec![(sector, byte)];
    if enhanced && (48..720).contains(&n) {
        places.push((VTOC2, n as usize / 8 - 6));
    }
    for (sector, byte) in places {
        let b = image
            .sector_mut(sector)?
            .get_mut(byte)
            .ok_or_else(|| Error::Damaged(format!("sector {n} is outside the VTOC")))?;
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
    // The 10-bit sector links can't reach past 1023.
    let last = image.sector_count().min(VTOC2 - 1);
    Ok((4..=last)
        .filter(|n| !(VTOC..DIR_FIRST + DIR_SECTORS).contains(n))
        .filter(|&n| fs.is_free(n))
        .collect())
}

/// A directory entry's 16 bytes, to change.
fn raw_entry_mut(image: &mut DiskImage, index: u8) -> Result<&mut [u8]> {
    let sector = image.sector_mut(DIR_FIRST + u32::from(index) / ENTRIES_PER_SECTOR as u32)?;
    let at = usize::from(index) % ENTRIES_PER_SECTOR * ENTRY_LEN;
    Ok(&mut sector[at..at + ENTRY_LEN])
}

/// Mark the sectors of `entry` free. Only the sectors that pass the
/// checks in [`Dos2::read`] are freed: in a damaged chain, the sectors
/// from the damage on may belong to another file.
fn free_chain(image: &mut DiskImage, entry: &Entry) -> Result<()> {
    let fs = Dos2::open(image)?;
    let enhanced = fs.enhanced();
    let mut sectors = Vec::new();
    // Damage just ends the list.
    let _ = fs.walk(entry, |n, _| sectors.push(n));
    for n in sectors {
        mark(image, enhanced, n, true)?;
    }
    Ok(())
}

/// Write a file called `name` holding `data`. If `old` is given (a file
/// of the same name), it's replaced and keeps its place in the
/// directory; otherwise the first unused or deleted entry is taken.
pub(crate) fn write(
    image: &mut DiskImage,
    old: Option<&Entry>,
    name: &[u8; 11],
    data: &[u8],
) -> Result<()> {
    let enhanced = Dos2::open(image)?.enhanced();
    let index = match old {
        Some(old) => {
            free_chain(image, old)?;
            old.index
        }
        None => free_entry(image)?,
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
    for (i, chunk) in chunks.iter().enumerate() {
        let n = free[i];
        let next = if i + 1 < chunks.len() { free[i + 1] } else { 0 };
        let sector = image.sector_mut(n)?;
        sector.fill(0);
        sector[..chunk.len()].copy_from_slice(chunk);
        sector[per_sector] = index << 2 | (next >> 8) as u8;
        sector[per_sector + 1] = next as u8;
        sector[per_sector + 2] = chunk.len() as u8;
        mark(image, enhanced, n, false)?;
    }
    let used = &free[..chunks.len()];
    let raw = raw_entry_mut(image, index)?;
    // As DOS 2.5 does: a file reaching above sector 719 is hidden from
    // DOS 2.0, which can't free those sectors.
    raw[0] = if used.iter().any(|&n| n >= 720) {
        MADE_BY_DOS2 | OPEN
    } else {
        IN_USE | MADE_BY_DOS2
    };
    raw[1..3].copy_from_slice(&(chunks.len() as u16).to_le_bytes());
    raw[3..5].copy_from_slice(&(free[0] as u16).to_le_bytes());
    raw[5..16].copy_from_slice(name);
    Ok(())
}

/// The first directory entry that's unused or deleted.
fn free_entry(image: &DiskImage) -> Result<u8> {
    Dos2::open(image)?
        .raw_entries()
        .find(|(_, raw)| raw[0] == 0 || raw[0] & DELETED != 0)
        .map(|(index, _)| index)
        .ok_or(Error::DirectoryFull)
}

/// Delete a file: free its sectors and mark its entry deleted. As in DOS
/// 2, the rest of the entry is left, so undelete tools can still find
/// it.
pub(crate) fn delete(image: &mut DiskImage, entry: &Entry) -> Result<()> {
    free_chain(image, entry)?;
    raw_entry_mut(image, entry.index)?[0] = DELETED;
    Ok(())
}

/// Give a file a new name (already checked).
pub(crate) fn rename(image: &mut DiskImage, entry: &Entry, name: &[u8; 11]) -> Result<()> {
    raw_entry_mut(image, entry.index)?[5..16].copy_from_slice(name);
    Ok(())
}

/// Lock or unlock a file.
pub(crate) fn set_locked(image: &mut DiskImage, entry: &Entry, locked: bool) -> Result<()> {
    let raw = raw_entry_mut(image, entry.index)?;
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

/// A blank DOS 2 data disk: VTOC, an empty directory, and a boot sector
/// that gives BOOT ERROR ([`DATA_DISK_BOOT`]).
pub(crate) fn format(container: crate::Container, kind: Kind) -> Result<DiskImage> {
    let (sector_size, count, total) = match kind {
        Kind::Single => (128, 720, 707u16),
        Kind::Double => (256, 720, 707),
        Kind::Enhanced => (128, 1040, 1010),
    };
    let enhanced = kind == Kind::Enhanced;
    let mut image = DiskImage::new(container, sector_size, count)?;
    image.sector_mut(1)?[..DATA_DISK_BOOT.len()].copy_from_slice(&DATA_DISK_BOOT);
    let vtoc = image.sector_mut(VTOC)?;
    vtoc[0] = 2;
    vtoc[1..3].copy_from_slice(&total.to_le_bytes());
    // `mark` counts the free sectors up from 0. Sector 720 stays in use,
    // as DOS 2 leaves it (the 810 drive can't always read it); on an
    // enhanced disk so does 1024, the second VTOC.
    let data = (4..720).chain(if enhanced { 721..VTOC2 } else { 0..0 });
    for n in data.filter(|n| !(VTOC..DIR_FIRST + DIR_SECTORS).contains(n)) {
        mark(&mut image, enhanced, n, true)?;
    }
    Ok(image)
}

/// Turn `name` into the 11 bytes of a directory entry, checking it
/// follows DOS 2's rules: a name of 1–8 letters and digits starting with
/// a letter, then optionally a dot and an extension of up to 3 letters
/// and digits. With `uppercase`, lower-case letters are turned into
/// capitals first; without it they're refused.
pub(crate) fn parse_name(name: &str, uppercase: bool) -> Result<[u8; 11]> {
    let bad = |why: &str| Err(Error::BadName(format!("{name}: {why}")));
    let text = if uppercase {
        name.to_ascii_uppercase()
    } else {
        name.to_string()
    };
    let (base, ext) = text.split_once('.').unwrap_or((&text, ""));
    if base.is_empty() || base.len() > 8 {
        return bad("the name must be 1 to 8 characters");
    }
    if ext.len() > 3 {
        return bad("the extension can be at most 3 characters");
    }
    let ok = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit();
    if !base.chars().chain(ext.chars()).all(ok) {
        return if text.chars().any(|c| c.is_ascii_lowercase()) {
            bad("lower-case letters aren't allowed")
        } else {
            bad("only letters A-Z and digits 0-9 are allowed")
        };
    }
    if !base.starts_with(|c: char| c.is_ascii_uppercase()) {
        return bad("the name must start with a letter");
    }
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(out)
}

/// The 11 bytes `name` would be stored as, without checking DOS's rules,
/// to find files already on a disk whatever their names. `None` if it
/// can't be the name of any file.
pub(crate) fn stored_name(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    if base.len() > 8 || ext.len() > 3 {
        return None;
    }
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(out)
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
