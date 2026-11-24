//! SpartaDOS file systems: SpartaDOS 2.x and 3.x (version `$20`) and
//! SpartaDOS X (version `$21`), on 128- or 256-byte sectors.
//!
//! # Layout
//! * Sectors 1–3: boot sectors. Sector 1 also holds the volume
//!   information:
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | `$09`–`$0A` | Sector map of the main (root) directory |
//!   | `$0B`–`$0C` | Sectors on the disk |
//!   | `$0D`–`$0E` | Free sectors |
//!   | `$0F` | Sectors in the bitmap |
//!   | `$10`–`$11` | First bitmap sector |
//!   | `$12`–`$13` | Where to start looking for a free data sector |
//!   | `$14`–`$15` | Where to start looking for a free directory sector |
//!   | `$16`–`$1D` | Volume name, padded with spaces |
//!   | `$1E` | Tracks (bit 7 set: double-sided) |
//!   | `$07` | `$80`: marks a SpartaDOS disk (otherwise SpartaDOS gives error 148) |
//!   | `$1F` | Sector size: `$80` 128 bytes, `$00` 256, `$01` 512 (SpartaDOS X) |
//!   | `$20` | File system version: `$20` SpartaDOS 2/3, `$21` SpartaDOS X |
//!   | `$26` | Volume sequence number (changes with each write) |
//!   | `$27` | Volume random number (set by format) |
//!
//! * The bitmap: one bit per sector, from sector 0, bit 7 first; 1 =
//!   free. Sector 0 doesn't exist and is marked in use.
//! * Every file, directories included, has a chain of *sector maps*.
//!   Each map sector starts with the next map (0 at the end) and the
//!   previous one, then lists the file's data sectors, 2 bytes each: 62
//!   per 128-byte map, 126 per 256-byte one. Data sectors hold nothing
//!   but data, all of every sector up to the file's length. (254 per
//!   map on 512-byte sectors.)
//! * A directory is a file of 23-byte entries. The first is its header:
//!   the parent directory's sector map (0 for the main directory) in
//!   bytes 1–2, its own length in bytes 3–5 and its name in 6–13. Each
//!   entry after that is:
//!
//!   | Bytes | Meaning |
//!   |---|---|
//!   | 0 | Flags: `$01` locked, `$02` hidden, `$04` archived, `$08` in use, `$10` deleted, `$20` directory |
//!   | 1–2 | First sector map |
//!   | 3–5 | Length in bytes |
//!   | 6–13 | Name, padded with spaces |
//!   | 14–16 | Extension, padded with spaces |
//!   | 17–19 | Date: day, month, year (2 digits) |
//!   | 20–22 | Time: hours, minutes, seconds |
//!
//!   Flags of 0 end the directory. In the parent, a directory's own
//!   length isn't kept up to date (SpartaDOS X leaves it at 23), so the
//!   header's length is the one to trust.

use crate::error::{Error, Result};
use crate::image::DiskImage;

const ENTRY_LEN: usize = 23;

const LOCKED: u8 = 0x01;
const HIDDEN: u8 = 0x02;
const ARCHIVED: u8 = 0x04;
const IN_USE: u8 = 0x08;
const DELETED: u8 = 0x10;
const DIRECTORY: u8 = 0x20;

/// A SpartaDOS disk, read from a [`DiskImage`].
#[derive(Debug, Clone)]
pub struct Sparta<'a> {
    image: &'a DiskImage,
    root: u16,
    total: u32,
    bitmap_first: u32,
    bitmap_count: u32,
}

/// One entry in a SpartaDOS directory: a file or a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// The sector map of the directory the entry is in.
    pub parent: u16,
    /// Where the entry is in that directory, counting the header as 0.
    pub index: u32,
    /// The flags byte (see the module notes).
    pub flags: u8,
    /// The first sector map.
    pub map: u16,
    /// The length in bytes. For a directory, from its own header.
    pub size: u32,
    /// The name as stored: 8 bytes, padded with spaces.
    pub name: [u8; 8],
    /// The extension as stored: 3 bytes, padded with spaces.
    pub ext: [u8; 3],
    /// Day, month, year (2 digits).
    pub date: [u8; 3],
    /// Hours, minutes, seconds.
    pub time: [u8; 3],
    /// The sectors it uses: data sectors and sector maps.
    pub sectors: u32,
    /// Following its sector maps went wrong (see [`Sparta::read`] for
    /// why), so reading it will fail.
    pub damaged: bool,
}

/// A date and time, as SpartaDOS keeps them for files and directories.
/// Make one with [`Timestamp::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Timestamp {
    /// The full year: SpartaDOS keeps 2 digits, read as 1980–2079.
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Entry {
    /// The name as SpartaDOS shows it, e.g. `SAMPLE.LST` (no dot when
    /// there is no extension). Bytes other than printable ASCII show as
    /// `?`.
    pub fn file_name(&self) -> String {
        crate::names::display(&self.name, &self.ext)
    }

    /// The name as stored, `NAME.EXT` in ATASCII: what a path gives to
    /// find it.
    pub fn stored_name(&self) -> Vec<u8> {
        crate::names::joined(&self.name, &self.ext)
    }

    pub fn is_dir(&self) -> bool {
        self.flags & DIRECTORY != 0
    }

    /// Locked ("protected") entries can't be written, renamed or deleted
    /// from DOS.
    pub fn locked(&self) -> bool {
        self.flags & LOCKED != 0
    }

    /// Hidden entries are left out of DIR listings.
    pub fn hidden(&self) -> bool {
        self.flags & HIDDEN != 0
    }

    /// SpartaDOS X's archive bit.
    pub fn archived(&self) -> bool {
        self.flags & ARCHIVED != 0
    }

    /// When it was last written, if the date is set and makes sense.
    pub fn timestamp(&self) -> Option<Timestamp> {
        let [day, month, yy] = self.date;
        let [hour, minute, second] = self.time;
        if !(1..=31).contains(&day)
            || !(1..=12).contains(&month)
            || yy > 99
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let year = if yy < 80 { 2000 } else { 1900 } + u16::from(yy);
        Some(Timestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

impl<'a> Sparta<'a> {
    /// Read `image` as a SpartaDOS disk. Fails with
    /// [`Error::UnknownFileSystem`] when sector 1 doesn't hold SpartaDOS
    /// volume information that fits the disk.
    pub fn open(image: &'a DiskImage) -> Result<Self> {
        let not = |why: &str| Err(Error::UnknownFileSystem(format!("not SpartaDOS: {why}")));
        let s1 = image.sector(1)?;
        if !matches!(s1[0x20], 0x20 | 0x21) {
            return not("sector 1 has no SpartaDOS version number");
        }
        let size = match s1[0x1F] {
            0x80 => 128,
            0x00 => 256,
            0x01 => 512,
            _ => return not("sector 1 gives an unknown sector size"),
        };
        if size != image.sector_size() {
            return not("sector 1's sector size isn't the disk's");
        }
        let count = image.sector_count();
        let total = u32::from(word(s1, 0x0B));
        let free = u32::from(word(s1, 0x0D));
        let root = word(s1, 0x09);
        let bitmap_count = u32::from(s1[0x0F]);
        let bitmap_first = u32::from(word(s1, 0x10));
        let in_disk = |n: u32| (1..=count).contains(&n);
        if total == 0 || total > count || free > total {
            return not("sector 1's sector counts don't fit the disk");
        }
        if !in_disk(u32::from(root))
            || bitmap_count == 0
            || !in_disk(bitmap_first)
            || !in_disk(bitmap_first + bitmap_count - 1)
            || bitmap_count * size as u32 * 8 <= total
        {
            return not("sector 1's directory or bitmap is outside the disk");
        }
        Ok(Self {
            image,
            root,
            total,
            bitmap_first,
            bitmap_count,
        })
    }

    /// The disk the file system is on.
    pub fn image(&self) -> &'a DiskImage {
        self.image
    }

    fn info(&self) -> &'a [u8] {
        // `open` read sector 1, so it's there.
        self.image.sector(1).unwrap_or(&[0; 128])
    }

    /// The file system version: `$20` for SpartaDOS 2.x and 3.x, `$21`
    /// for SpartaDOS X.
    pub fn version(&self) -> u8 {
        self.info()[0x20]
    }

    /// The volume name as stored (ATASCII), without the spaces at the
    /// end.
    pub fn volume_name(&self) -> Vec<u8> {
        crate::names::joined(&self.info()[0x16..0x1E], b"")
    }

    /// The sectors on the disk, as sector 1 gives them.
    pub fn total_sectors(&self) -> u32 {
        self.total
    }

    /// Free sectors, as sector 1 counts them.
    pub fn free_sectors(&self) -> u32 {
        u32::from(word(self.info(), 0x0D))
    }

    /// The sector map of the main directory.
    pub fn root(&self) -> u16 {
        self.root
    }

    /// Is sector `n` marked free in the bitmap?
    pub fn is_free(&self, n: u32) -> bool {
        if n > self.total {
            return false;
        }
        let bits = self.image.sector_size() as u32 * 8;
        let sector = self.bitmap_first + n / bits;
        let byte = (n % bits / 8) as usize;
        sector < self.bitmap_first + self.bitmap_count
            && self
                .image
                .sector(sector)
                .is_ok_and(|s| s[byte] & (0x80 >> (n % 8)) != 0)
    }

    /// The entries in the directory whose sector map is `map` (see
    /// [`Sparta::root`] and [`Entry::map`]), in directory order. Deleted
    /// entries are left out.
    pub fn list_dir(&self, map: u16) -> Result<Vec<Entry>> {
        let data = self.read_dir(map)?;
        let mut out = Vec::new();
        for (i, raw) in data.as_chunks::<ENTRY_LEN>().0.iter().enumerate().skip(1) {
            let flags = raw[0];
            if flags == 0 {
                break;
            }
            if flags & DELETED != 0 || flags & IN_USE == 0 {
                continue;
            }
            let mut entry = Entry {
                parent: map,
                index: i as u32,
                flags,
                map: word(raw, 1),
                size: u32::from(raw[3]) | u32::from(raw[4]) << 8 | u32::from(raw[5]) << 16,
                name: raw[6..14].try_into().unwrap(),
                ext: raw[14..17].try_into().unwrap(),
                date: raw[17..20].try_into().unwrap(),
                time: raw[20..23].try_into().unwrap(),
                sectors: 0,
                damaged: false,
            };
            if entry.is_dir() {
                match self.dir_len(entry.map) {
                    Ok(len) => entry.size = len,
                    Err(_) => entry.damaged = true,
                }
            }
            match self.data_sectors(entry.map, entry.size, &entry.file_name()) {
                Ok((maps, data)) => entry.sectors = (maps.len() + data.len()) as u32,
                Err(_) => entry.damaged = true,
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// The entries in directory `path` (`""` for the main one, or e.g.
    /// `"GAMES>OLD"`; a trailing `>` is fine). Names are matched with
    /// `same`.
    pub(crate) fn list_path(
        &self,
        path: &[u8],
        same: impl Fn(&Entry, &[u8]) -> bool,
    ) -> Result<Vec<Entry>> {
        self.list_dir(self.dir_map(path, same)?)
    }

    /// The sector map of directory `path`. Fails with
    /// [`Error::NotFound`] if a part of it isn't there, or
    /// [`Error::NotADirectory`] if it's a file.
    pub(crate) fn dir_map(&self, path: &[u8], same: impl Fn(&Entry, &[u8]) -> bool) -> Result<u16> {
        let mut map = self.root;
        for part in path.split(|&c| c == b'>').filter(|p| !p.is_empty()) {
            let text = || crate::atascii::atascii_to_ascii(path);
            let entry = self
                .list_dir(map)?
                .into_iter()
                .find(|e| same(e, part))
                .ok_or_else(|| Error::NotFound(text()))?;
            if !entry.is_dir() {
                return Err(Error::NotADirectory(text()));
            }
            map = entry.map;
        }
        Ok(map)
    }

    /// The entry at `path` (e.g. `"GAMES>PACMAN.COM"`, as ATASCII),
    /// ignoring case.
    pub fn find(&self, path: impl AsRef<[u8]>) -> Result<Entry> {
        let path = path.as_ref();
        let same = |e: &Entry, name: &[u8]| e.stored_name().eq_ignore_ascii_case(name);
        let (dir, name) = crate::names::split_path(path);
        self.list_path(dir, same)?
            .into_iter()
            .find(|e| same(e, name))
            .ok_or_else(|| Error::NotFound(crate::atascii::atascii_to_ascii(path)))
    }

    /// The contents of a file (or the raw bytes of a directory). Fails
    /// with [`Error::Damaged`] if a sector map or data sector is outside
    /// the disk, the maps go round in a loop, or they list fewer sectors
    /// than the length needs.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        self.read_map(entry.map, entry.size, &entry.file_name())
    }

    /// The bitmap's sectors.
    pub(crate) fn bitmap_list(&self) -> std::ops::Range<u32> {
        self.bitmap_first..self.bitmap_first + self.bitmap_count
    }

    /// Which sectors the disk's own records use: the boot sectors, the
    /// bitmap, and every directory's and file's sector maps and data,
    /// found by walking the whole tree. Index `n` is sector `n` (index 0
    /// is unused). Compare with [`Sparta::is_free`] to find sectors the
    /// bitmap has wrong. Fails with [`Error::Damaged`] if any directory
    /// or file can't be followed, or a directory is reached twice.
    pub fn sectors_in_use(&self) -> Result<Vec<bool>> {
        let mut used = vec![false; self.total as usize + 1];
        for n in (1..=3).chain(self.bitmap_first..self.bitmap_first + self.bitmap_count) {
            if let Some(u) = used.get_mut(n as usize) {
                *u = true;
            }
        }
        let mut dirs = vec![self.root];
        let mut seen = Vec::new();
        while let Some(map) = dirs.pop() {
            if seen.contains(&map) {
                return Err(Error::Damaged(format!(
                    "the directory at sector map {map} is reached twice"
                )));
            }
            seen.push(map);
            let len = self.dir_len(map)?;
            let (maps, data) = self.data_sectors(map, len, "a directory")?;
            for entry in self.list_dir(map)? {
                if entry.is_dir() {
                    dirs.push(entry.map);
                } else {
                    let (m, d) = self.data_sectors(entry.map, entry.size, &entry.file_name())?;
                    for n in m.into_iter().chain(d) {
                        used[n as usize] = true;
                    }
                }
            }
            for n in maps.into_iter().chain(data) {
                used[n as usize] = true;
            }
        }
        Ok(used)
    }

    /// A directory's length, from its header.
    pub(crate) fn dir_len(&self, map: u16) -> Result<u32> {
        let header = self.read_map(map, ENTRY_LEN as u32, "a directory")?;
        let len = u32::from(header[3]) | u32::from(header[4]) << 8 | u32::from(header[5]) << 16;
        if len < ENTRY_LEN as u32 {
            return Err(Error::Damaged(format!(
                "the directory at sector map {map} claims {len} bytes"
            )));
        }
        Ok(len)
    }

    /// A directory's bytes, header included.
    fn read_dir(&self, map: u16) -> Result<Vec<u8>> {
        let len = self.dir_len(map)?;
        self.read_map(map, len, "a directory")
    }

    fn read_map(&self, map: u16, len: u32, name: &str) -> Result<Vec<u8>> {
        let (_, data) = self.data_sectors(map, len, name)?;
        let mut out = Vec::with_capacity(len as usize);
        for n in data {
            out.extend_from_slice(self.image.sector(n)?);
        }
        out.truncate(len as usize);
        Ok(out)
    }

    /// The sector maps of a file and the data sectors its `len` bytes
    /// take, checked.
    pub(crate) fn data_sectors(
        &self,
        map: u16,
        len: u32,
        name: &str,
    ) -> Result<(Vec<u32>, Vec<u32>)> {
        let damaged = |why: String| Err(Error::Damaged(format!("{name}: {why}")));
        let size = self.image.sector_size() as u32;
        let wanted = len.div_ceil(size) as usize;
        let mut maps = Vec::new();
        let mut data = Vec::with_capacity(wanted);
        let mut m = u32::from(map);
        while m != 0 && data.len() < wanted {
            if maps.len() as u32 >= self.total {
                return damaged("its sector maps go round in a loop".into());
            }
            if m > self.total {
                return damaged(format!("sector map {m} is outside the disk"));
            }
            maps.push(m);
            let sector = self.image.sector(m)?;
            for pair in sector[4..].as_chunks::<2>().0 {
                if data.len() == wanted {
                    break;
                }
                let n = u32::from(u16::from_le_bytes(*pair));
                if n == 0 || n > self.total {
                    return damaged(format!("sector map {m} lists sector {n}"));
                }
                data.push(n);
            }
            m = u32::from(word(sector, 0));
        }
        if data.len() < wanted {
            return damaged(format!(
                "its sector maps list {} sectors, but {len} bytes need {wanted}",
                data.len()
            ));
        }
        // An empty file still has its first map.
        if maps.is_empty() && map != 0 {
            maps.push(u32::from(map));
        }
        Ok((maps, data))
    }
}

// ---------------------------------------------------------------------
// Changing a disk. `Volume` calls these on a copy of the image and keeps
// the copy only if they succeed, so a failure part way through leaves the
// disk as it was.

const DATA_HINT: usize = 0x12;
const DIR_HINT: usize = 0x14;
const SEQUENCE: usize = 0x26;
/// SpartaDOS 3.x keeps directory lengths in 16 bits.
const MAX_DIR_LEN: u32 = 0xFFFF;

use crate::names::Name;

impl Timestamp {
    /// A date and time from 1980 to 2079 (the years SpartaDOS's two digits
    /// can mean). Fails with [`Error::BadDate`] if it isn't a real one.
    pub fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Result<Self> {
        let t = Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        };
        t.to_bytes()?;
        Ok(t)
    }

    /// The 6 bytes a directory entry stores: day, month, year (2 digits),
    /// hours, minutes, seconds. Fails with [`Error::BadDate`] if it isn't
    /// a real date and time from 1980 to 2079.
    pub(crate) fn to_bytes(self) -> Result<[u8; 6]> {
        let days = match self.month {
            2 if self.year.is_multiple_of(4) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if !(1980..=2079).contains(&self.year)
            || !(1..=12).contains(&self.month)
            || !(1..=days).contains(&self.day)
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
        {
            return Err(Error::BadDate(format!("{self:?}")));
        }
        Ok([
            self.day,
            self.month,
            (self.year % 100) as u8,
            self.hour,
            self.minute,
            self.second,
        ])
    }
}

/// Changes one SpartaDOS disk.
pub(crate) struct Writer<'i> {
    image: &'i mut DiskImage,
    root: u16,
    total: u32,
    bitmap_first: u32,
    bitmap_count: u32,
    /// Sectors a new file may take: marked free in the bitmap, and not
    /// used by any file or directory (a bitmap can be wrong).
    usable: Vec<bool>,
}

impl<'i> Writer<'i> {
    /// Fails if the disk isn't SpartaDOS, or any file or directory on it
    /// can't be followed: changing a damaged disk could make it worse.
    pub(crate) fn new(image: &'i mut DiskImage) -> Result<Self> {
        let fs = Sparta::open(image)?;
        let used = fs.sectors_in_use()?;
        let usable = (0..=fs.total)
            .map(|n| fs.is_free(n) && !used[n as usize])
            .collect();
        let (root, total, bitmap_first, bitmap_count) =
            (fs.root, fs.total, fs.bitmap_first, fs.bitmap_count);
        let writer = Self {
            image,
            root,
            total,
            bitmap_first,
            bitmap_count,
            usable,
        };
        // Every change counts as one write of the disk, so SpartaDOS can
        // tell a disk has changed since it last read it.
        let s1 = writer.image.sector_mut(1)?;
        s1[SEQUENCE] = s1[SEQUENCE].wrapping_add(1);
        Ok(writer)
    }

    fn reader(&self) -> Sparta<'_> {
        Sparta {
            image: self.image,
            root: self.root,
            total: self.total,
            bitmap_first: self.bitmap_first,
            bitmap_count: self.bitmap_count,
        }
    }

    fn size(&self) -> usize {
        self.image.sector_size()
    }

    fn per_map(&self) -> usize {
        (self.size() - 4) / 2
    }

    fn free_count(&self) -> u32 {
        self.usable.iter().filter(|&&u| u).count() as u32
    }

    /// Mark sector `n` free or in use in the bitmap, and count it.
    fn mark(&mut self, n: u32, free: bool) -> Result<()> {
        let bits = self.size() as u32 * 8;
        let sector = self.bitmap_first + n / bits;
        let byte = (n % bits / 8) as usize;
        let mask = 0x80 >> (n % 8);
        let map = self.image.sector_mut(sector)?;
        let was_free = map[byte] & mask != 0;
        if free {
            map[byte] |= mask;
        } else {
            map[byte] &= !mask;
        }
        let s1 = self.image.sector_mut(1)?;
        let count = word(s1, 0x0D);
        let count = match (was_free, free) {
            (false, true) => count + 1,
            (true, false) => count.saturating_sub(1),
            _ => count,
        };
        s1[0x0D..0x0F].copy_from_slice(&count.to_le_bytes());
        self.usable[n as usize] = free;
        Ok(())
    }

    /// Take a free sector, searching on from where the last one was
    /// found (SpartaDOS keeps separate places for data and directories).
    fn take(&mut self, dir: bool) -> Result<u32> {
        let hint = if dir { DIR_HINT } else { DATA_HINT };
        let from = u32::from(word(self.image.sector(1)?, hint)).clamp(1, self.total);
        let n = (from..=self.total)
            .chain(1..from)
            .find(|&n| self.usable[n as usize])
            .ok_or(Error::DiskFull { needed: 1, free: 0 })?;
        self.mark(n, false)?;
        let next = if n < self.total { n + 1 } else { 1 } as u16;
        self.image.sector_mut(1)?[hint..hint + 2].copy_from_slice(&next.to_le_bytes());
        Ok(n)
    }

    /// Sectors (maps and data) a file of `len` bytes needs.
    fn needed(&self, len: usize) -> u32 {
        let data = len.div_ceil(self.size());
        (data + data.div_ceil(self.per_map()).max(1)) as u32
    }

    /// Write `bytes` as a new file, giving its first sector map.
    fn new_chain(&mut self, bytes: &[u8], dir: bool) -> Result<u16> {
        let needed = self.needed(bytes.len());
        let free = self.free_count();
        if needed > free {
            return Err(Error::DiskFull { needed, free });
        }
        let size = self.size();
        let chunks: Vec<&[u8]> = bytes.chunks(size).collect();
        let groups: Vec<&[&[u8]]> = if chunks.is_empty() {
            vec![&[]]
        } else {
            chunks.chunks(self.per_map()).collect()
        };
        let (mut first, mut prev) = (0, 0u32);
        for group in groups {
            let m = self.take(dir)?;
            let mut map = vec![0; size];
            map[2..4].copy_from_slice(&(prev as u16).to_le_bytes());
            for (i, chunk) in group.iter().enumerate() {
                let n = self.take(dir)?;
                let mut data = vec![0; size];
                data[..chunk.len()].copy_from_slice(chunk);
                self.image.write_sector(n, &data)?;
                map[4 + 2 * i..6 + 2 * i].copy_from_slice(&(n as u16).to_le_bytes());
            }
            self.image.write_sector(m, &map)?;
            if prev == 0 {
                first = m;
            } else {
                self.image.sector_mut(prev)?[..2].copy_from_slice(&(m as u16).to_le_bytes());
            }
            prev = m;
        }
        Ok(first as u16)
    }

    /// Free a file's sector maps and data.
    fn free_chain(&mut self, map: u16, len: u32) -> Result<()> {
        let (maps, data) = self.reader().data_sectors(map, len, "a file")?;
        for n in maps.into_iter().chain(data) {
            self.mark(n, true)?;
        }
        Ok(())
    }

    /// A directory's bytes, header included.
    fn dir(&self, map: u16) -> Result<Vec<u8>> {
        self.reader().read_dir(map)
    }

    /// Write a directory back, `bytes` header included, giving it more
    /// sectors if it has grown, and its new length in the header.
    fn store_dir(&mut self, map: u16, mut bytes: Vec<u8>) -> Result<()> {
        let len = bytes.len() as u32;
        if len > MAX_DIR_LEN {
            return Err(Error::DirectoryFull);
        }
        bytes[3..6].copy_from_slice(&len.to_le_bytes()[..3]);
        let old = self.reader().dir_len(map)?;
        let (mut maps, mut data) = self.reader().data_sectors(map, old, "a directory")?;
        let per_map = self.per_map();
        let wanted = bytes.len().div_ceil(self.size());
        if wanted > data.len() {
            let needed = (wanted - data.len()) as u32 + 1;
            let free = self.free_count();
            if needed > free {
                return Err(Error::DiskFull { needed, free });
            }
        }
        while data.len() < wanted {
            if data.len() == maps.len() * per_map {
                // The last map is full: link a new one after it.
                let m = self.take(true)?;
                let last = *maps.last().expect("a directory has a map");
                let mut new = vec![0; self.size()];
                new[2..4].copy_from_slice(&(last as u16).to_le_bytes());
                self.image.write_sector(m, &new)?;
                self.image.sector_mut(last)?[..2].copy_from_slice(&(m as u16).to_le_bytes());
                maps.push(m);
            }
            let n = self.take(true)?;
            let slot = 4 + 2 * (data.len() % per_map);
            let last = *maps.last().expect("a directory has a map");
            self.image.sector_mut(last)?[slot..slot + 2].copy_from_slice(&(n as u16).to_le_bytes());
            data.push(n);
        }
        let size = self.size();
        for (i, &n) in data.iter().enumerate() {
            let mut sector = vec![0; size];
            let part = &bytes[(i * size).min(bytes.len())..((i + 1) * size).min(bytes.len())];
            sector[..part.len()].copy_from_slice(part);
            self.image.write_sector(n, &sector)?;
        }
        Ok(())
    }

    /// Put `entry` (23 bytes) into directory `dir`: in the place of the
    /// first deleted entry, or on the end.
    fn add_entry(&mut self, dir: u16, entry: &[u8; ENTRY_LEN]) -> Result<()> {
        let mut bytes = self.dir(dir)?;
        let place = bytes
            .as_chunks::<ENTRY_LEN>()
            .0
            .iter()
            .skip(1)
            .position(|e| e[0] == 0 || e[0] & DELETED != 0)
            .map(|i| (i + 1) * ENTRY_LEN);
        match place {
            Some(at) => bytes[at..at + ENTRY_LEN].copy_from_slice(entry),
            None => {
                bytes.truncate(bytes.len() / ENTRY_LEN * ENTRY_LEN);
                bytes.extend_from_slice(entry);
            }
        }
        self.store_dir(dir, bytes)
    }

    /// Change entry `entry` in its directory with `f`.
    fn edit_entry(&mut self, entry: &Entry, f: impl FnOnce(&mut [u8])) -> Result<()> {
        let mut bytes = self.dir(entry.parent)?;
        let at = entry.index as usize * ENTRY_LEN;
        f(&mut bytes[at..at + ENTRY_LEN]);
        self.store_dir(entry.parent, bytes)
    }

    /// Write a file into directory `dir`, replacing `old` if given (its
    /// sectors are freed first, so a file that only fits once the old one
    /// is gone still fits). `when` of `None` leaves a new file's date
    /// blank, and keeps a replaced file's.
    pub(crate) fn write(
        &mut self,
        dir: u16,
        old: Option<&Entry>,
        name: &Name,
        data: &[u8],
        when: Option<[u8; 6]>,
    ) -> Result<()> {
        if data.len() > 0xFF_FFFF {
            return Err(Error::DiskFull {
                needed: self.needed(data.len()),
                free: self.free_count(),
            });
        }
        if let Some(old) = old {
            self.free_chain(old.map, old.size)?;
        }
        let map = self.new_chain(data, false)?;
        let len = (data.len() as u32).to_le_bytes();
        match old {
            Some(old) => self.edit_entry(old, |e| {
                e[1..3].copy_from_slice(&map.to_le_bytes());
                e[3..6].copy_from_slice(&len[..3]);
                // No new date: keep the old one.
                if let Some(when) = when {
                    e[17..23].copy_from_slice(&when);
                }
            }),
            None => {
                let when = when.unwrap_or_default();
                self.add_entry(dir, &raw_entry(IN_USE, map, data.len() as u32, name, &when))
            }
        }
    }

    /// Make an empty directory in directory `dir`.
    pub(crate) fn make_dir(&mut self, dir: u16, name: &Name, when: Option<[u8; 6]>) -> Result<()> {
        let when = when.unwrap_or_default();
        let mut header = raw_entry(IN_USE | DIRECTORY, dir, ENTRY_LEN as u32, name, &when);
        // Room for its own entry too, so both fit or neither is made.
        let needed = self.needed(ENTRY_LEN) + 2;
        let free = self.free_count();
        if needed > free {
            return Err(Error::DiskFull { needed, free });
        }
        let map = self.new_chain(&header, true)?;
        header[1..3].copy_from_slice(&map.to_le_bytes());
        self.add_entry(dir, &header)
    }

    /// Delete a file, or a directory with nothing in it
    /// ([`Error::DirectoryNotEmpty`] otherwise).
    pub(crate) fn delete(&mut self, entry: &Entry) -> Result<()> {
        if entry.is_dir() && !self.reader().list_dir(entry.map)?.is_empty() {
            return Err(Error::DirectoryNotEmpty(entry.file_name()));
        }
        self.free_chain(entry.map, entry.size)?;
        self.edit_entry(entry, |e| e[0] = (e[0] & !IN_USE) | DELETED)
    }

    /// Give an entry a new name (a directory's own header too).
    pub(crate) fn rename(&mut self, entry: &Entry, name: &Name) -> Result<()> {
        self.edit_entry(entry, |e| e[6..17].copy_from_slice(name))?;
        if entry.is_dir() {
            let mut bytes = self.dir(entry.map)?;
            bytes[6..17].copy_from_slice(name);
            self.store_dir(entry.map, bytes)?;
        }
        Ok(())
    }

    pub(crate) fn set_locked(&mut self, entry: &Entry, locked: bool) -> Result<()> {
        self.set_flag(entry, LOCKED, locked)
    }

    pub(crate) fn set_hidden(&mut self, entry: &Entry, on: bool) -> Result<()> {
        self.set_flag(entry, HIDDEN, on)
    }

    pub(crate) fn set_archived(&mut self, entry: &Entry, on: bool) -> Result<()> {
        self.set_flag(entry, ARCHIVED, on)
    }

    fn set_flag(&mut self, entry: &Entry, flag: u8, on: bool) -> Result<()> {
        self.edit_entry(entry, |e| {
            if on {
                e[0] |= flag;
            } else {
                e[0] &= !flag;
            }
        })
    }

    /// Set an entry's date and time (`None`: blank). A directory's own
    /// header gets it too.
    pub(crate) fn set_timestamp(&mut self, entry: &Entry, when: Option<[u8; 6]>) -> Result<()> {
        let when = when.unwrap_or_default();
        self.edit_entry(entry, |e| e[17..23].copy_from_slice(&when))?;
        if entry.is_dir() {
            let mut bytes = self.dir(entry.map)?;
            bytes[17..23].copy_from_slice(&when);
            self.store_dir(entry.map, bytes)?;
        }
        Ok(())
    }

    /// Move an entry into directory `dir` (keeping its name, date and
    /// sectors). A directory can't go into itself or one inside it.
    pub(crate) fn move_entry(&mut self, entry: &Entry, dir: u16) -> Result<()> {
        if entry.is_dir() {
            // Walk up from `dir` to the main directory.
            let mut m = dir;
            let mut steps = 0;
            while m != 0 {
                if m == entry.map {
                    return Err(Error::Unsupported(format!(
                        "moving {} into itself",
                        entry.file_name()
                    )));
                }
                steps += 1;
                if steps > self.total {
                    return Err(Error::Damaged("directories go round in a loop".into()));
                }
                m = word(&self.dir(m)?, 1);
            }
        }
        let mut raw = [0; ENTRY_LEN];
        let bytes = self.dir(entry.parent)?;
        let at = entry.index as usize * ENTRY_LEN;
        raw.copy_from_slice(&bytes[at..at + ENTRY_LEN]);
        self.edit_entry(entry, |e| e[0] = (e[0] & !IN_USE) | DELETED)?;
        self.add_entry(dir, &raw)?;
        if entry.is_dir() {
            let mut own = self.dir(entry.map)?;
            own[1..3].copy_from_slice(&dir.to_le_bytes());
            self.store_dir(entry.map, own)?;
        }
        Ok(())
    }

    /// Set the volume name: up to 8 printable characters.
    pub(crate) fn set_volume_name(&mut self, name: &[u8]) -> Result<()> {
        let stored = volume_name(name)?;
        self.image.sector_mut(1)?[0x16..0x1E].copy_from_slice(&stored);
        Ok(())
    }
}

/// A 23-byte directory entry.
fn raw_entry(flags: u8, map: u16, len: u32, name: &Name, when: &[u8; 6]) -> [u8; ENTRY_LEN] {
    let mut e = [0; ENTRY_LEN];
    e[0] = flags;
    e[1..3].copy_from_slice(&map.to_le_bytes());
    e[3..6].copy_from_slice(&len.to_le_bytes()[..3]);
    e[6..17].copy_from_slice(name);
    e[17..23].copy_from_slice(when);
    e
}

/// A volume name as stored: up to 8 printable ATASCII characters
/// (`$20`-`$7C`), padded with spaces.
pub(crate) fn volume_name(name: &[u8]) -> Result<[u8; 8]> {
    if name.len() > 8 || !name.iter().all(|b| (0x20..=0x7C).contains(b)) {
        return Err(Error::BadName(format!(
            "{}: a volume name is up to 8 printable characters",
            crate::atascii::atascii_to_ascii(name)
        )));
    }
    let mut out = [b' '; 8];
    out[..name.len()].copy_from_slice(name);
    Ok(out)
}

/// Our boot sector for data disks: load this one sector at `$0640`, then
/// the OS jumps to byte 6, `JMP $0680` (whose `$80` is the SpartaDOS mark
/// at byte 7), and at byte `$40` `SEC` `RTS` makes the boot fail, so the
/// OS prints BOOT ERROR. The volume information goes in bytes 9-42.
const BOOT: [(usize, &[u8]); 2] = [
    (
        0x00,
        &[0x00, 0x01, 0x40, 0x06, 0x41, 0x06, 0x4C, 0x80, 0x06],
    ),
    (0x40, &[0x38, 0x60]),
];

/// A blank, formatted SpartaDOS data disk of `count` sectors of `size`
/// bytes (128, 256 or 512).
pub(crate) fn format(
    container: crate::Container,
    size: usize,
    count: u32,
    name: &[u8],
) -> Result<DiskImage> {
    let stored = volume_name(name)?;
    if !(16..=0xFFFF).contains(&count) {
        return Err(Error::Unsupported(format!(
            "SpartaDOS disks have 16 to 65535 sectors, not {count}"
        )));
    }
    let mut image = DiskImage::new(container, size, count)?;
    let bitmap_count = (count + 1).div_ceil(size as u32 * 8);
    let bitmap_first = 4;
    let root = bitmap_first + bitmap_count;
    let first_free = root + 2;
    // SpartaDOS 3.2's XINIT keeps the last sector out of use (720 on a
    // single-density disk, 1040 on an enhanced one); SpartaDOS X doesn't,
    // and only it reads 512-byte sectors.
    let last_free = if size == 512 { count } else { count - 1 };
    let free = last_free + 1 - first_free;
    let tracks: u8 = match (count, size) {
        (720 | 1040, 128) | (720, 256) => 40,
        (1440, 256) => 0x80 | 40,
        (2880, 256) => 0x80 | 80,
        _ => 1,
    };
    // SpartaDOS 2/3 and SpartaDOS X both read version $20; only SpartaDOS
    // X can use 512-byte sectors, so those get $21.
    let (version, size_code, extra): (u8, u8, [u8; 5]) = match size {
        128 => (0x20, 0x80, [0x06, 0x01, 0xFF, 0xFF, 0x00]),
        256 => (0x20, 0x00, [0x06, 0x01, 0xFF, 0xFF, 0x00]),
        _ => (0x21, 0x01, [0x00, 0x01, 0x7E, 0x00, 0x01]),
    };
    let random = {
        use std::hash::{BuildHasher, RandomState};
        RandomState::new().hash_one(count) as u8
    };

    // XINIT starts looking for data sectors a little way on, leaving the
    // sectors after the main directory for directories to grow into
    // (seen on XINIT's own blanks; 32 on single and enhanced density
    // because X32G.DOS landed there with 7-31 free).
    let data_start: u16 = match (count, size) {
        (720 | 1040, 128) => 32,
        (720, 256) => 37,
        _ => first_free as u16,
    };

    let s1 = image.sector_mut(1)?;
    for (at, bytes) in BOOT {
        s1[at..at + bytes.len()].copy_from_slice(bytes);
    }
    s1[0x09..0x0B].copy_from_slice(&(root as u16).to_le_bytes());
    s1[0x0B..0x0D].copy_from_slice(&(count as u16).to_le_bytes());
    s1[0x0D..0x0F].copy_from_slice(&(free as u16).to_le_bytes());
    s1[0x0F] = bitmap_count as u8;
    s1[0x10..0x12].copy_from_slice(&(bitmap_first as u16).to_le_bytes());
    s1[DATA_HINT..DATA_HINT + 2].copy_from_slice(&data_start.to_le_bytes());
    s1[DIR_HINT..DIR_HINT + 2].copy_from_slice(&(first_free as u16).to_le_bytes());
    s1[0x16..0x1E].copy_from_slice(&stored);
    s1[0x1E] = tracks;
    s1[0x1F] = size_code;
    s1[0x20] = version;
    s1[0x21..0x26].copy_from_slice(&extra);
    s1[0x27] = random;

    let bits = size as u32 * 8;
    for n in first_free..=last_free {
        let sector = image.sector_mut(bitmap_first + n / bits)?;
        sector[(n % bits / 8) as usize] |= 0x80 >> (n % 8);
    }
    let mut map = vec![0; size];
    map[4..6].copy_from_slice(&((root + 1) as u16).to_le_bytes());
    image.write_sector(root, &map)?;
    let mut main = vec![0; size];
    main[..ENTRY_LEN].copy_from_slice(&raw_entry(
        IN_USE | DIRECTORY,
        0,
        ENTRY_LEN as u32,
        b"MAIN       ",
        &[0; 6],
    ));
    image.write_sector(root + 1, &main)?;
    Ok(image)
}

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Container;

    /// A tiny SpartaDOS disk built by hand: 128-byte sectors, the main
    /// directory (map 5, data 6) holding one file, HI.TXT (map 7, data
    /// 8), and a directory, SUB (map 9, data 10) holding nothing.
    fn tiny() -> DiskImage {
        let mut image = DiskImage::new(Container::Atr, 128, 720).unwrap();
        let s1 = image.sector_mut(1).unwrap();
        s1[0x09] = 5;
        s1[0x0B..0x0D].copy_from_slice(&720u16.to_le_bytes());
        s1[0x0D..0x0F].copy_from_slice(&710u16.to_le_bytes());
        s1[0x0F] = 1;
        s1[0x10] = 4;
        s1[0x16..0x1E].copy_from_slice(b"TINY    ");
        s1[0x1F] = 0x80;
        s1[0x20] = 0x20;
        let bitmap = image.sector_mut(4).unwrap();
        for n in 11..=720 {
            bitmap[n / 8] |= 0x80 >> (n % 8);
        }
        image.sector_mut(5).unwrap()[4] = 6;
        let mut root = [0u8; 128];
        root[..17].copy_from_slice(b"\x28\0\0\x45\0\0MAIN       ");
        root[23..46].copy_from_slice(b"\x08\x07\0\x05\0\0HI      TXT\x18\x0b\x1a\x0c\x22\x38");
        root[46..69].copy_from_slice(b"\x28\x09\0\x17\0\0SUB        \0\0\0\0\0\0");
        image.write_sector(6, &root).unwrap();
        image.sector_mut(7).unwrap()[4] = 8;
        image.sector_mut(8).unwrap()[..5].copy_from_slice(b"hello");
        image.sector_mut(9).unwrap()[4] = 10;
        image.sector_mut(10).unwrap()[..17].copy_from_slice(b"\x28\x05\0\x17\0\0SUB        ");
        image
    }

    #[test]
    fn reads_a_tiny_disk() {
        let image = tiny();
        let fs = Sparta::open(&image).unwrap();
        assert_eq!(fs.volume_name(), b"TINY");
        assert_eq!(fs.version(), 0x20);
        assert_eq!(fs.free_sectors(), 710);
        assert!(!fs.is_free(10) && fs.is_free(11) && fs.is_free(720) && !fs.is_free(721));
        let root = fs.list_dir(fs.root()).unwrap();
        assert_eq!(root.len(), 2);
        let hi = &root[0];
        assert_eq!(
            (hi.file_name().as_str(), hi.size, hi.sectors),
            ("HI.TXT", 5, 2)
        );
        assert!(!hi.is_dir() && !hi.damaged);
        assert_eq!(
            hi.timestamp(),
            Some(Timestamp {
                year: 2026,
                month: 11,
                day: 24,
                hour: 12,
                minute: 34,
                second: 56
            })
        );
        assert_eq!(fs.read(hi).unwrap(), b"hello");
        assert!(root[1].is_dir());
        assert_eq!(root[1].timestamp(), None);
        assert!(fs.list_dir(root[1].map).unwrap().is_empty());
        assert_eq!(fs.find("sub").unwrap(), root[1]);
        assert!(matches!(fs.find("SUB>HI.TXT"), Err(Error::NotFound(_))));
        assert!(matches!(fs.find("NOPE>X"), Err(Error::NotFound(_))));
    }

    #[test]
    fn damage_is_reported() {
        let mut image = tiny();
        // HI.TXT's map now lists a sector off the disk.
        image.sector_mut(7).unwrap()[4..6].copy_from_slice(&900u16.to_le_bytes());
        // SUB's map points back to itself, with no data sectors.
        image.sector_mut(9).unwrap()[..6].copy_from_slice(&[9, 0, 0, 0, 0, 0]);
        let fs = Sparta::open(&image).unwrap();
        let root = fs.list_dir(fs.root()).unwrap();
        assert!(root.iter().all(|e| e.damaged));
        assert!(matches!(fs.read(&root[0]), Err(Error::Damaged(_))));
        assert!(fs.list_dir(root[1].map).is_err());
    }

    #[test]
    fn other_disks_are_refused() {
        let blank = DiskImage::new(Container::Atr, 128, 720).unwrap();
        assert!(Sparta::open(&blank).is_err());
        let mut image = tiny();
        image.sector_mut(1).unwrap()[0x1F] = 0; // claims 256-byte sectors
        assert!(Sparta::open(&image).is_err());
    }
}
