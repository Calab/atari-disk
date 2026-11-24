//! [`Volume`]: one disk and its file system, behind the calls the editor
//! uses (list, read, write, rename, delete, lock, make a directory,
//! format), whichever DOS the disk is for.

use crate::dos2::{self, Dos2};
use crate::error::{Error, Result};
use crate::image::{Container, DiskImage};
use crate::sparta::{self, Sparta, Timestamp};

/// The file systems a [`Volume`] can work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileSystem {
    /// Atari DOS 2.0S, 2.0D or 2.5 on a single- or double-density disk.
    Dos2,
    /// DOS 2.5 on an enhanced-density disk (1040 sectors, two VTOCs).
    Dos25Enhanced,
    /// SpartaDOS 2.x or 3.x (file system version `$20`). SpartaDOS X
    /// reads these too.
    SpartaDos,
    /// SpartaDOS X (file system version `$21`). The same file system as
    /// [`FileSystem::SpartaDos`], with a few additions.
    SpartaDosX,
}

impl FileSystem {
    /// Does it have directories (so `>` paths and
    /// [`Volume::make_dir`])?
    pub fn has_directories(self) -> bool {
        matches!(self, FileSystem::SpartaDos | FileSystem::SpartaDosX)
    }
}

/// The blank disks [`Volume::format`] can make. They're data disks:
/// they hold no DOS, so they can't be booted (booting one gives the OS's
/// BOOT ERROR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Layout {
    /// DOS 2.0S single density: 720 sectors of 128 bytes, 707 free.
    Dos2Single,
    /// DOS 2.0D double density: 720 sectors of 256 bytes, 707 free.
    /// For DOS 2.0S/2.0D (and MyDOS); DOS 2.5 can't read 256-byte
    /// sectors (in AltirraSDL it gives error 143).
    Dos2Double,
    /// DOS 2.5 enhanced density: 1040 sectors of 128 bytes, 1010 free.
    Dos25Enhanced,
    /// SpartaDOS single density: 720 sectors of 128 bytes.
    SpartaSingle,
    /// SpartaDOS enhanced density: 1040 sectors of 128 bytes.
    SpartaEnhanced,
    /// SpartaDOS double density: 720 sectors of 256 bytes.
    SpartaDouble,
    /// SpartaDOS with any number of sectors (16 to 65535) of 128, 256 or
    /// 512 bytes, e.g. 1440 × 256 for a double-sided disk or 65535 × 256
    /// for a hard disk partition. 512-byte sectors are for SpartaDOS X
    /// only.
    SpartaCustom { sectors: u32, sector_size: usize },
}

/// Is a directory entry a file or a directory?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    /// Only on file systems with directories (SpartaDOS).
    Directory,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The name as DOS shows it, e.g. `SAMPLE.LST`.
    pub name: String,
    pub kind: EntryKind,
    /// The length in bytes.
    pub size: u32,
    /// The sectors it takes up (on SpartaDOS, its sector maps included).
    pub sectors: u32,
    pub locked: bool,
    /// Left out of DIR listings (SpartaDOS only).
    pub hidden: bool,
    /// When it was last written, if the file system keeps the date
    /// (SpartaDOS does, DOS 2 doesn't) and it's set.
    pub modified: Option<Timestamp>,
    /// The file system's records for it don't add up, so reading it will
    /// fail (and `size` is only what could be followed).
    pub damaged: bool,
}

/// A disk image and the file system on it. The volume owns the image;
/// [`Volume::image`] gives it back to save, e.g. with
/// [`DiskImage::to_bytes`].
///
/// Paths use `>` between directories, as SpartaDOS does:
/// `"GAMES>PACMAN.COM"`, with no `D:` and no leading `>`. On DOS 2,
/// which has no directories, a path is just a file name.
///
/// Every change either succeeds completely or leaves the disk as it was.
/// On SpartaDOS, changes are refused with [`Error::Damaged`] if any file
/// or directory on the disk can't be followed, and new files never take a
/// sector something else uses, even if the bitmap says it's free.
///
/// ```
/// use atari_disk::{Container, Layout, Volume};
///
/// let mut vol = Volume::format(Layout::Dos2Single, Container::Atr)?;
/// vol.write("HELLO.TXT", b"Hello, Atari", None)?;
/// assert_eq!(vol.read("HELLO.TXT")?, b"Hello, Atari");
/// assert_eq!(vol.free_sectors(), 706);
/// let bytes = vol.image().to_bytes(); // ready to save as an .ATR file
/// # Ok::<(), atari_disk::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Volume {
    image: DiskImage,
    fs: FileSystem,
    uppercase: bool,
    overwrite: bool,
}

/// The existing entry a path names, on either file system.
enum Found {
    Dos2(dos2::Entry),
    Sparta(sparta::Entry),
}

impl Found {
    fn name(&self) -> String {
        match self {
            Found::Dos2(e) => e.file_name(),
            Found::Sparta(e) => e.file_name(),
        }
    }

    fn locked(&self) -> bool {
        match self {
            Found::Dos2(e) => e.locked(),
            Found::Sparta(e) => e.locked(),
        }
    }

    fn check_unlocked(&self) -> Result<()> {
        if self.locked() {
            Err(Error::Locked(self.name()))
        } else {
            Ok(())
        }
    }
}

impl Volume {
    /// Which file system, if any, `image` holds. [`Volume::open`] takes
    /// the image, so ask this first if you want to keep the image when
    /// it isn't one this crate can read.
    pub fn detect(image: &DiskImage) -> Option<FileSystem> {
        // SpartaDOS first: its sector 1 is checked closely, while a
        // SpartaDOS file can happen to make sector 360 look like a DOS 2
        // VTOC.
        if let Ok(fs) = Sparta::open(image) {
            return Some(if fs.version() == 0x21 {
                FileSystem::SpartaDosX
            } else {
                FileSystem::SpartaDos
            });
        }
        let fs = Dos2::open(image).ok()?;
        Some(if fs.enhanced() {
            FileSystem::Dos25Enhanced
        } else {
            FileSystem::Dos2
        })
    }

    /// Open the file system on `image`. Fails with
    /// [`Error::UnknownFileSystem`] when [`Volume::detect`] finds none.
    pub fn open(image: DiskImage) -> Result<Self> {
        let fs = Self::detect(&image)
            .ok_or_else(|| Error::UnknownFileSystem("not a DOS 2 or SpartaDOS disk".into()))?;
        Ok(Self {
            image,
            fs,
            uppercase: false,
            overwrite: false,
        })
    }

    /// A blank, formatted data disk. A SpartaDOS one has no volume name
    /// until [`Volume::set_volume_name`] gives it one.
    pub fn format(layout: Layout, container: Container) -> Result<Self> {
        let image = match layout {
            Layout::Dos2Single => dos2::format(container, dos2::Kind::Single)?,
            Layout::Dos2Double => dos2::format(container, dos2::Kind::Double)?,
            Layout::Dos25Enhanced => dos2::format(container, dos2::Kind::Enhanced)?,
            Layout::SpartaSingle => sparta::format(container, 128, 720, "")?,
            Layout::SpartaEnhanced => sparta::format(container, 128, 1040, "")?,
            Layout::SpartaDouble => sparta::format(container, 256, 720, "")?,
            Layout::SpartaCustom {
                sectors,
                sector_size,
            } => sparta::format(container, sector_size, sectors, "")?,
        };
        Self::open(image)
    }

    pub fn file_system(&self) -> FileSystem {
        self.fs
    }

    /// The disk image, to save.
    pub fn image(&self) -> &DiskImage {
        &self.image
    }

    /// Give up the volume, keeping the disk image.
    pub fn into_image(self) -> DiskImage {
        self.image
    }

    /// Sector `n` as it is on the disk, whatever the file system.
    pub fn read_sector(&self, n: u32) -> Result<&[u8]> {
        self.image.sector(n)
    }

    /// Write sector `n` directly, bypassing the file system: for making
    /// custom disks. `bytes` must be exactly the sector's length
    /// ([`DiskImage::sector_len`]). Nothing checks that the file system
    /// still makes sense afterwards; if it doesn't, file calls fail with
    /// [`Error::Damaged`] or [`Error::UnknownFileSystem`]. If the disk now
    /// holds a different file system, the volume switches to it.
    pub fn write_sector(&mut self, n: u32, bytes: &[u8]) -> Result<()> {
        self.image.write_sector(n, bytes)?;
        if let Some(fs) = Self::detect(&self.image) {
            self.fs = fs;
        }
        Ok(())
    }

    /// Turn lower-case letters in new names into capitals, instead of
    /// refusing them with [`Error::BadName`] (the default). Finding
    /// existing files then ignores case too.
    pub fn set_auto_uppercase(&mut self, on: bool) {
        self.uppercase = on;
    }

    pub fn auto_uppercase(&self) -> bool {
        self.uppercase
    }

    /// Let [`Volume::write`] replace a file that's already there, instead
    /// of failing with [`Error::AlreadyExists`] (the default). Locked
    /// files are still refused.
    pub fn set_overwrite(&mut self, on: bool) {
        self.overwrite = on;
    }

    pub fn overwrite(&self) -> bool {
        self.overwrite
    }

    /// The volume name, on file systems that have one (SpartaDOS).
    pub fn volume_name(&self) -> Option<String> {
        self.sparta().ok().map(|fs| fs.volume_name())
    }

    /// Set the volume name: up to 8 printable characters (SpartaDOS
    /// only; DOS 2 disks have none, [`Error::Unsupported`]).
    pub fn set_volume_name(&mut self, name: &str) -> Result<()> {
        self.need_dirs("volume names")?;
        self.change_sparta(|w| w.set_volume_name(name))
    }

    /// Free sectors, as the disk's DOS counts them.
    pub fn free_sectors(&self) -> u32 {
        match self.fs.has_directories() {
            true => self.sparta().map_or(0, |fs| fs.free_sectors()),
            false => self.dos2().map_or(0, |fs| fs.free_sectors()),
        }
    }

    /// Free space in bytes: how long a file could be written. DOS 2
    /// keeps 3 bytes of each sector for itself; SpartaDOS needs a sector
    /// map for every 62 data sectors (126 on 256-byte sectors, 254 on
    /// 512). (On SpartaDOS a new file may also need a sector for its
    /// directory, not counted here.)
    pub fn free_bytes(&self) -> u32 {
        let free = self.free_sectors();
        let size = self.image.sector_size() as u32;
        if self.fs.has_directories() {
            let per_map = (size - 4) / 2;
            (free - free.div_ceil(per_map + 1)) * size
        } else {
            free * (size - 3)
        }
    }

    /// The entries in directory `dir`, in directory order: `""` for the
    /// top one, or e.g. `"GAMES>OLD"` on SpartaDOS (a trailing `>` is
    /// fine). Fails with [`Error::NotFound`] if there's no such
    /// directory.
    pub fn list(&self, dir: &str) -> Result<Vec<DirEntry>> {
        if self.fs.has_directories() {
            let entries = self.sparta()?.list_path(dir, |e, n| self.same(e, n))?;
            return Ok(entries.iter().map(sparta_entry).collect());
        }
        if !dir.is_empty() {
            return Err(no_dirs());
        }
        Ok(self
            .dos2()?
            .list()
            .into_iter()
            .map(|e| DirEntry {
                name: e.file_name(),
                kind: EntryKind::File,
                size: e.size,
                sectors: u32::from(e.sectors),
                locked: e.locked(),
                hidden: false,
                modified: None,
                damaged: e.damaged,
            })
            .collect())
    }

    /// A file's contents.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self.find(path)? {
            Found::Dos2(e) => self.dos2()?.read(&e),
            Found::Sparta(e) if e.is_dir() => Err(Error::NotFound(format!(
                "{path} is a directory, not a file"
            ))),
            Found::Sparta(e) => self.sparta()?.read(&e),
        }
    }

    /// Write a new file. If one of that name is already there this fails
    /// with [`Error::AlreadyExists`], unless [`Volume::set_overwrite`] is
    /// on; then it's replaced, unless it's locked ([`Error::Locked`]).
    ///
    /// `when` is the date and time to give it, on file systems that keep
    /// one (SpartaDOS); `None` leaves it blank. DOS 2 ignores it.
    pub fn write(&mut self, path: &str, data: &[u8], when: Option<Timestamp>) -> Result<()> {
        let old = match self.find(path) {
            Ok(old) => Some(old),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        if let Some(old) = &old {
            let is_dir = matches!(old, Found::Sparta(e) if e.is_dir());
            if !self.overwrite || is_dir {
                return Err(Error::AlreadyExists(old.name()));
            }
            old.check_unlocked()?;
        }
        if self.fs.has_directories() {
            let when = when.map(Timestamp::to_bytes).transpose()?;
            let (dir, name) = split(path);
            let old = match old {
                Some(Found::Sparta(e)) => Some(e),
                _ => None,
            };
            let stored = match &old {
                // Keep the name exactly as it is on the disk.
                Some(e) => [&e.name[..], &e.ext[..]].concat().try_into().unwrap(),
                None => sparta::parse_name(name, self.uppercase)?,
            };
            let map = self.sparta()?.dir_map(dir, |e, n| self.same(e, n))?;
            return self.change_sparta(|w| w.write(map, old.as_ref(), &stored, data, when));
        }
        let old = match old {
            Some(Found::Dos2(e)) => Some(e),
            _ => None,
        };
        let name = match &old {
            Some(old) => {
                let mut name = [0; 11];
                name[..8].copy_from_slice(&old.name);
                name[8..].copy_from_slice(&old.ext);
                name
            }
            None => dos2::parse_name(path, self.uppercase)?,
        };
        self.change(|image| dos2::write(image, old.as_ref(), &name, data))
    }

    /// Give a file (or directory) the name `new_name` (a name only: it
    /// stays in its directory). Fails with [`Error::Locked`] if it's
    /// locked, or [`Error::AlreadyExists`] if another entry has that name.
    pub fn rename(&mut self, path: &str, new_name: &str) -> Result<()> {
        let entry = self.find(path)?;
        entry.check_unlocked()?;
        let (dir, _) = split(path);
        let sibling = if dir.is_empty() {
            new_name.to_string()
        } else {
            format!("{dir}>{new_name}")
        };
        if let Ok(other) = self.find(&sibling) {
            let same_entry = match (&other, &entry) {
                (Found::Dos2(a), Found::Dos2(b)) => a.index == b.index,
                (Found::Sparta(a), Found::Sparta(b)) => a.index == b.index,
                _ => false,
            };
            if !same_entry {
                return Err(Error::AlreadyExists(other.name()));
            }
        }
        match entry {
            Found::Dos2(e) => {
                let name = dos2::parse_name(new_name, self.uppercase)?;
                self.change(|image| dos2::rename(image, &e, &name))
            }
            Found::Sparta(e) => {
                let name = sparta::parse_name(new_name, self.uppercase)?;
                self.change_sparta(|w| w.rename(&e, &name))
            }
        }
    }

    /// Delete a file, or an empty directory. Fails with [`Error::Locked`]
    /// if it's locked, or [`Error::DirectoryNotEmpty`].
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let entry = self.find(path)?;
        entry.check_unlocked()?;
        match entry {
            Found::Dos2(e) => self.change(|image| dos2::delete(image, &e)),
            Found::Sparta(e) => self.change_sparta(|w| w.delete(&e)),
        }
    }

    /// Lock a file (or directory), so it can't be written, renamed or
    /// deleted.
    pub fn lock(&mut self, path: &str) -> Result<()> {
        self.set_locked(path, true)
    }

    /// Unlock a file (or directory).
    pub fn unlock(&mut self, path: &str) -> Result<()> {
        self.set_locked(path, false)
    }

    fn set_locked(&mut self, path: &str, locked: bool) -> Result<()> {
        match self.find(path)? {
            Found::Dos2(e) => self.change(|image| dos2::set_locked(image, &e, locked)),
            Found::Sparta(e) => self.change_sparta(|w| w.set_locked(&e, locked)),
        }
    }

    /// Make an empty directory, e.g. `"GAMES"` or `"GAMES>OLD"` (its
    /// parent must be there). `when` is its date and time, or `None` for
    /// none. Only SpartaDOS has directories; on DOS 2 this fails with
    /// [`Error::Unsupported`].
    pub fn make_dir(&mut self, path: &str, when: Option<Timestamp>) -> Result<()> {
        self.need_dirs("directories")?;
        match self.find(path) {
            Ok(old) => return Err(Error::AlreadyExists(old.name())),
            Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let when = when.map(Timestamp::to_bytes).transpose()?;
        let (dir, name) = split(path);
        let name = sparta::parse_name(name, self.uppercase)?;
        let map = self.sparta()?.dir_map(dir, |e, n| self.same(e, n))?;
        self.change_sparta(|w| w.make_dir(map, &name, when))
    }

    fn need_dirs(&self, what: &str) -> Result<()> {
        if self.fs.has_directories() {
            Ok(())
        } else {
            Err(Error::Unsupported(format!("DOS 2 has no {what}")))
        }
    }

    fn dos2(&self) -> Result<Dos2<'_>> {
        // A direct sector write may have broken it.
        Dos2::open(&self.image).map_err(|e| Error::Damaged(e.to_string()))
    }

    fn sparta(&self) -> Result<Sparta<'_>> {
        if !self.fs.has_directories() {
            return Err(Error::Unsupported("not a SpartaDOS disk".into()));
        }
        Sparta::open(&self.image).map_err(|e| Error::Damaged(e.to_string()))
    }

    /// Is `wanted` the stored name of `e`? See [`Volume::same_name`].
    fn same(&self, e: &sparta::Entry, wanted: &str) -> bool {
        self.same_name(&e.name, &e.ext, wanted)
    }

    /// Is `wanted` the stored name `name` + `ext`? Names are matched as
    /// stored, so files with names DOS wouldn't make can still be found;
    /// with auto-uppercase on, case is ignored.
    fn same_name(&self, name: &[u8], ext: &[u8], wanted: &str) -> bool {
        let Some(wanted) = dos2::stored_name(wanted) else {
            return false;
        };
        let stored = [name, ext].concat();
        if self.uppercase {
            stored.eq_ignore_ascii_case(&wanted)
        } else {
            stored == wanted
        }
    }

    /// The existing entry at `path`.
    fn find(&self, path: &str) -> Result<Found> {
        let not_found = || Error::NotFound(path.to_string());
        if self.fs.has_directories() {
            let (dir, name) = split(path);
            return self
                .sparta()?
                .list_path(dir, |e, n| self.same(e, n))?
                .into_iter()
                .find(|e| self.same(e, name))
                .map(Found::Sparta)
                .ok_or_else(not_found);
        }
        if path.contains('>') {
            return Err(no_dirs());
        }
        self.dos2()?
            .list()
            .into_iter()
            .find(|e| self.same_name(&e.name, &e.ext, path))
            .map(Found::Dos2)
            .ok_or_else(not_found)
    }

    /// Make a change on a copy of the disk, keeping it only if every
    /// step worked.
    fn change(&mut self, f: impl FnOnce(&mut DiskImage) -> Result<()>) -> Result<()> {
        let mut copy = self.image.clone();
        f(&mut copy)?;
        self.image = copy;
        Ok(())
    }

    /// [`Volume::change`] for SpartaDOS.
    fn change_sparta(&mut self, f: impl FnOnce(&mut sparta::Writer) -> Result<()>) -> Result<()> {
        let mut copy = self.image.clone();
        f(&mut sparta::Writer::new(&mut copy)?)?;
        self.image = copy;
        Ok(())
    }
}

/// A path's directory and its last name: `"A>B>C.TXT"` gives `"A>B"` and
/// `"C.TXT"`.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('>').unwrap_or(("", path))
}

fn sparta_entry(e: &sparta::Entry) -> DirEntry {
    DirEntry {
        name: e.file_name(),
        kind: if e.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: e.size,
        sectors: e.sectors,
        locked: e.locked(),
        hidden: e.hidden(),
        modified: e.timestamp(),
        damaged: e.damaged,
    }
}

fn no_dirs() -> Error {
    Error::Unsupported("DOS 2 has no directories".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Volume {
        Volume::format(Layout::Dos2Single, Container::Atr).unwrap()
    }

    fn names(vol: &Volume) -> Vec<String> {
        vol.list("").unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn formats() {
        for (layout, fs, free, sectors) in [
            (Layout::Dos2Single, FileSystem::Dos2, 707, 720),
            (Layout::Dos2Double, FileSystem::Dos2, 707, 720),
            (Layout::Dos25Enhanced, FileSystem::Dos25Enhanced, 1010, 1040),
        ] {
            let vol = Volume::format(layout, Container::Atr).unwrap();
            assert_eq!(vol.file_system(), fs);
            assert_eq!(vol.free_sectors(), free);
            assert_eq!(vol.image().sector_count(), sectors);
            assert!(vol.list("").unwrap().is_empty());
            let dos2 = Dos2::open(vol.image()).unwrap();
            let marked = (0..1024).filter(|&n| dos2.is_free(n)).count() as u32;
            assert_eq!(marked, free, "{layout:?}");
        }
        let vol = Volume::format(Layout::Dos2Double, Container::Atr).unwrap();
        assert_eq!(vol.free_bytes(), 707 * 253);
    }

    #[test]
    fn write_read_replace() {
        let mut vol = blank();
        let big: Vec<u8> = (0..1000).map(|i| i as u8).collect();
        vol.write("BIG.DAT", &big, None).unwrap();
        assert_eq!(vol.free_sectors(), 707 - 8);
        vol.write("EMPTY", b"", None).unwrap();
        assert_eq!(vol.free_sectors(), 707 - 9);
        assert_eq!(vol.read("BIG.DAT").unwrap(), big);
        assert_eq!(vol.read("EMPTY").unwrap(), b"");
        // Replacing frees the old sectors and keeps the directory place.
        assert!(matches!(
            vol.write("BIG.DAT", b"small", None),
            Err(Error::AlreadyExists(_))
        ));
        assert_eq!(vol.read("BIG.DAT").unwrap(), big);
        vol.set_overwrite(true);
        vol.write("BIG.DAT", b"small", None).unwrap();
        assert_eq!(vol.free_sectors(), 707 - 2);
        assert_eq!(names(&vol), ["BIG.DAT", "EMPTY"]);
        let entry = &vol.list("").unwrap()[0];
        assert_eq!((entry.size, entry.sectors), (5, 1));
    }

    #[test]
    fn delete_reuses_the_entry_and_sectors() {
        let mut vol = blank();
        vol.write("A", &[1; 300], None).unwrap();
        vol.write("B", &[2; 10], None).unwrap();
        vol.delete("A").unwrap();
        assert_eq!(names(&vol), ["B"]);
        assert_eq!(vol.free_sectors(), 706);
        vol.write("C", &[3; 10], None).unwrap();
        assert_eq!(names(&vol), ["C", "B"]);
        assert!(matches!(vol.read("A"), Err(Error::NotFound(_))));
        assert!(matches!(vol.delete("A"), Err(Error::NotFound(_))));
    }

    #[test]
    fn names_are_checked() {
        let mut vol = blank();
        for bad in [
            "",
            "1ABC",
            "TOOLONGNAME",
            "A.TOOLONG",
            "A B",
            "A*",
            "lower.txt",
        ] {
            assert!(
                matches!(vol.write(bad, b"x", None), Err(Error::BadName(_))),
                "{bad:?}"
            );
        }
        assert!(vol.write("GAMES>X", b"x", None).is_err());
        vol.set_auto_uppercase(true);
        vol.write("lower.txt", b"x", None).unwrap();
        assert_eq!(names(&vol), ["LOWER.TXT"]);
        assert_eq!(vol.read("Lower.Txt").unwrap(), b"x");
        vol.set_auto_uppercase(false);
        assert!(matches!(vol.read("lower.txt"), Err(Error::NotFound(_))));
    }

    #[test]
    fn rename() {
        let mut vol = blank();
        vol.write("OLD.TXT", b"x", None).unwrap();
        vol.write("OTHER", b"y", None).unwrap();
        vol.rename("OLD.TXT", "NEW.TXT").unwrap();
        assert_eq!(names(&vol), ["NEW.TXT", "OTHER"]);
        assert!(matches!(
            vol.rename("NEW.TXT", "OTHER"),
            Err(Error::AlreadyExists(_))
        ));
        assert!(matches!(
            vol.rename("NEW.TXT", "bad"),
            Err(Error::BadName(_))
        ));
        assert_eq!(vol.read("NEW.TXT").unwrap(), b"x");
    }

    #[test]
    fn locked_files_refuse_changes_until_unlocked() {
        let mut vol = blank();
        vol.write("KEEP", b"x", None).unwrap();
        vol.lock("KEEP").unwrap();
        vol.set_overwrite(true);
        assert!(vol.list("").unwrap()[0].locked);
        for result in [
            vol.write("KEEP", b"y", None),
            vol.rename("KEEP", "OTHER"),
            vol.delete("KEEP"),
        ] {
            assert!(matches!(result, Err(Error::Locked(_))));
        }
        assert_eq!(vol.read("KEEP").unwrap(), b"x");
        vol.unlock("KEEP").unwrap();
        vol.write("KEEP", b"y", None).unwrap();
        assert_eq!(vol.read("KEEP").unwrap(), b"y");
    }

    #[test]
    fn a_full_disk_leaves_everything_as_it_was() {
        let mut vol = blank();
        vol.write("FIRST", b"x", None).unwrap();
        let before = vol.image().clone();
        let too_big = vec![0; 707 * 125];
        assert!(matches!(
            vol.write("HUGE", &too_big, None),
            Err(Error::DiskFull {
                needed: 707,
                free: 706
            })
        ));
        assert_eq!(*vol.image(), before);
        // Replacing a file can use the file's own sectors.
        vol.set_overwrite(true);
        vol.write("FIRST", &too_big, None).unwrap();
        assert_eq!(vol.free_sectors(), 0);
        assert_eq!(vol.read("FIRST").unwrap(), too_big);
    }

    #[test]
    fn the_directory_holds_64_files() {
        let mut vol = blank();
        for i in 0..64 {
            vol.write(&format!("F{i}"), b"x", None).unwrap();
        }
        assert!(matches!(
            vol.write("ONEMORE", b"x", None),
            Err(Error::DirectoryFull)
        ));
        let image = vol.image();
        // File numbers in the data sectors match the directory places.
        let dos2 = Dos2::open(image).unwrap();
        for entry in dos2.list() {
            assert!(!entry.damaged, "{}", entry.file_name());
        }
    }

    #[test]
    fn enhanced_disks_use_both_vtocs() {
        let mut vol = Volume::format(Layout::Dos25Enhanced, Container::Atr).unwrap();
        // 800 sectors' worth: all 707 below 720, then 93 above.
        let data = vec![7; 800 * 125];
        vol.write("BIG", &data, None).unwrap();
        assert_eq!(vol.free_sectors(), 210);
        let dos2 = Dos2::open(vol.image()).unwrap();
        assert_eq!(dos2.vtoc_free_sectors(), 0);
        assert_eq!(vol.read("BIG").unwrap(), data);
        // The second VTOC's copy of the bitmap (sectors 48-719) matches.
        let vtoc = vol.image().sector(360).unwrap();
        let vtoc2 = vol.image().sector(1024).unwrap();
        assert_eq!(vtoc[16..100], vtoc2[..84]);
        // DOS 2.5 marks a file above sector 719 with $03; a small one
        // below stays $42.
        let entry = &Dos2::open(vol.image()).unwrap().list()[0];
        assert!(entry.above_720());
        assert_eq!(entry.flags, 0x03);
        vol.lock("BIG").unwrap();
        assert_eq!(Dos2::open(vol.image()).unwrap().list()[0].flags, 0x23);
        assert!(vol.list("").unwrap()[0].locked);
        vol.unlock("BIG").unwrap();
        vol.delete("BIG").unwrap();
        vol.write("SMALL", b"x", None).unwrap();
        let entry = &Dos2::open(vol.image()).unwrap().list()[0];
        assert!(!entry.above_720());
        assert_eq!(entry.flags, 0x42);
        assert_eq!(vol.free_sectors(), 1009);
        let vtoc = vol.image().sector(360).unwrap();
        let vtoc2 = vol.image().sector(1024).unwrap();
        assert_eq!(vtoc[16..100], vtoc2[..84]);
    }

    #[test]
    fn dos2_has_no_directories() {
        let mut vol = blank();
        assert!(matches!(
            vol.make_dir("GAMES", None),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(vol.list("GAMES"), Err(Error::Unsupported(_))));
    }

    // ---- SpartaDOS ----

    fn sparta_blank() -> Volume {
        Volume::format(Layout::SpartaSingle, Container::Atr).unwrap()
    }

    fn when(day: u8) -> Timestamp {
        Timestamp {
            year: 2026,
            month: 11,
            day,
            hour: 9,
            minute: 5,
            second: 30,
        }
    }

    /// Every used sector is marked used, and the free count matches the
    /// bitmap.
    fn check_bitmap(vol: &Volume) {
        let fs = Sparta::open(vol.image()).unwrap();
        let used = fs.sectors_in_use().unwrap();
        let total = fs.total_sectors();
        for n in 1..=total {
            assert!(!(used[n as usize] && fs.is_free(n)), "sector {n}");
        }
        let marked = (0..=total).filter(|&n| fs.is_free(n)).count() as u32;
        assert_eq!(marked, fs.free_sectors());
    }

    #[test]
    fn sparta_formats() {
        for (layout, fs, free, count, size) in [
            (Layout::SpartaSingle, FileSystem::SpartaDos, 713, 720, 128),
            (
                Layout::SpartaEnhanced,
                FileSystem::SpartaDos,
                1032,
                1040,
                128,
            ),
            (Layout::SpartaDouble, FileSystem::SpartaDos, 713, 720, 256),
            (
                Layout::SpartaCustom {
                    sectors: 65535,
                    sector_size: 256,
                },
                FileSystem::SpartaDos,
                65535 - 3 - 32 - 2 - 1,
                65535,
                256,
            ),
            (
                Layout::SpartaCustom {
                    sectors: 4000,
                    sector_size: 512,
                },
                FileSystem::SpartaDosX,
                4000 - 3 - 1 - 2,
                4000,
                512,
            ),
        ] {
            let mut vol = Volume::format(layout, Container::Atr).unwrap();
            assert_eq!(vol.file_system(), fs, "{layout:?}");
            assert_eq!(vol.free_sectors(), free, "{layout:?}");
            assert_eq!(vol.image().sector_count(), count);
            assert_eq!(vol.image().sector_size(), size);
            assert!(vol.list("").unwrap().is_empty());
            assert_eq!(vol.volume_name().as_deref(), Some(""));
            check_bitmap(&vol);
            // SpartaDOS's mark, and the boot sector that fails politely.
            assert_eq!(vol.image().sector(1).unwrap()[7], 0x80);
            vol.set_volume_name("My Disk").unwrap();
            assert_eq!(vol.volume_name().as_deref(), Some("My Disk"));
            let data: Vec<u8> = (0..3000).map(|i| i as u8).collect();
            vol.write("A.DAT", &data, None).unwrap();
            assert_eq!(vol.read("A.DAT").unwrap(), data);
            check_bitmap(&vol);
            // Round trip through the file bytes.
            let back =
                Volume::open(DiskImage::from_bytes(&vol.image().to_bytes()).unwrap()).unwrap();
            assert_eq!(back.read("A.DAT").unwrap(), data);
        }
        assert!(
            Volume::format(
                Layout::SpartaCustom {
                    sectors: 70000,
                    sector_size: 256
                },
                Container::Atr
            )
            .is_err()
        );
        assert!(matches!(
            sparta_blank().set_volume_name("TOOLONGNAME"),
            Err(Error::BadName(_))
        ));
        assert!(matches!(
            blank().set_volume_name("X"),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn sparta_files_and_directories() {
        let mut vol = sparta_blank();
        let free = vol.free_sectors();
        vol.write("README.TXT", b"hello", Some(when(1))).unwrap();
        vol.make_dir("GAMES", Some(when(2))).unwrap();
        vol.make_dir("GAMES>OLD", None).unwrap();
        vol.write("GAMES>OLD>PAC_1.COM", &[7; 200], Some(when(3)))
            .unwrap();
        // 1 map + 1 data each for README.TXT and both directories, 1 + 2
        // for PAC_1.COM.
        assert_eq!(vol.free_sectors(), free - 9);
        let root = vol.list("").unwrap();
        assert_eq!(root[0].name, "README.TXT");
        assert_eq!(root[0].modified, Some(when(1)));
        assert_eq!(
            (root[1].name.as_str(), root[1].kind),
            ("GAMES", EntryKind::Directory)
        );
        assert_eq!(root[1].modified, Some(when(2)));
        let old = vol.list("GAMES>OLD>").unwrap();
        assert_eq!(
            (old[0].name.as_str(), old[0].size, old[0].sectors),
            ("PAC_1.COM", 200, 3)
        );
        assert_eq!(vol.read("GAMES>OLD>PAC_1.COM").unwrap(), [7; 200]);
        assert!(matches!(vol.read("GAMES"), Err(Error::NotFound(_))));
        assert!(matches!(vol.list("NOPE"), Err(Error::NotFound(_))));
        assert!(matches!(
            vol.write("NOPE>X", b"x", None),
            Err(Error::NotFound(_))
        ));
        // Names that exist already, as a file or a directory.
        assert!(matches!(
            vol.make_dir("README.TXT", None),
            Err(Error::AlreadyExists(_))
        ));
        vol.set_overwrite(true);
        assert!(matches!(
            vol.write("GAMES", b"x", None),
            Err(Error::AlreadyExists(_))
        ));
        // Replacing keeps the place and takes the new date.
        vol.write("README.TXT", &[1; 300], Some(when(4))).unwrap();
        let root = vol.list("").unwrap();
        assert_eq!((root[0].size, root[0].modified), (300, Some(when(4))));
        check_bitmap(&vol);

        // Renaming a directory renames its header too.
        vol.rename("GAMES", "PLAY").unwrap();
        assert_eq!(vol.read("PLAY>OLD>PAC_1.COM").unwrap(), [7; 200]);
        let fs = Sparta::open(vol.image()).unwrap();
        let play = fs.find("PLAY").unwrap();
        assert_eq!(&fs.read(&play).unwrap()[6..17], b"PLAY       ");
        assert!(matches!(
            vol.rename("PLAY", "README.TXT"),
            Err(Error::AlreadyExists(_))
        ));

        // Only empty directories can be deleted.
        assert!(matches!(
            vol.delete("PLAY>OLD"),
            Err(Error::DirectoryNotEmpty(_))
        ));
        vol.lock("PLAY>OLD>PAC_1.COM").unwrap();
        assert!(vol.list("PLAY>OLD").unwrap()[0].locked);
        assert!(matches!(
            vol.delete("PLAY>OLD>PAC_1.COM"),
            Err(Error::Locked(_))
        ));
        vol.unlock("PLAY>OLD>PAC_1.COM").unwrap();
        vol.delete("PLAY>OLD>PAC_1.COM").unwrap();
        vol.delete("PLAY>OLD").unwrap();
        vol.delete("PLAY").unwrap();
        vol.delete("README.TXT").unwrap();
        assert!(vol.list("").unwrap().is_empty());
        assert_eq!(vol.free_sectors(), free);
        check_bitmap(&vol);
        // A new file takes the first deleted entry's place.
        vol.write("NEW", b"x", None).unwrap();
        let fs = Sparta::open(vol.image()).unwrap();
        assert_eq!(fs.list_dir(fs.root()).unwrap()[0].index, 1);
    }

    #[test]
    fn sparta_names_and_dates() {
        let mut vol = sparta_blank();
        for bad in ["", "TOOLONGNAME", "A.TOOLONG", "A B", "A*", "lower", "A>"] {
            assert!(
                matches!(vol.write(bad, b"x", None), Err(Error::BadName(_))),
                "{bad:?}"
            );
        }
        // SpartaDOS lets names start with a digit and use _.
        vol.write("1ST_TRY.BAS", b"x", None).unwrap();
        let mut bad = when(1);
        bad.month = 2;
        bad.day = 30;
        assert!(matches!(
            vol.write("X", b"x", Some(bad)),
            Err(Error::BadDate(_))
        ));
        bad = when(1);
        bad.year = 2080;
        assert!(matches!(
            vol.make_dir("D", Some(bad)),
            Err(Error::BadDate(_))
        ));
        vol.set_auto_uppercase(true);
        vol.make_dir("games", None).unwrap();
        vol.write("Games>Hi.txt", b"x", None).unwrap();
        assert_eq!(vol.read("GAMES>HI.TXT").unwrap(), b"x");
    }

    #[test]
    fn sparta_directories_grow() {
        // 128-byte sectors: 62 data sectors per map, so a directory of
        // 400 entries (9200 bytes, 72 sectors) needs a second map.
        let mut vol = Volume::format(Layout::SpartaEnhanced, Container::Atr).unwrap();
        for i in 0..400 {
            vol.write(&format!("F{i}"), &[i as u8], None).unwrap();
        }
        let names: Vec<String> = vol.list("").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names.len(), 400);
        assert_eq!(names[399], "F399");
        assert_eq!(vol.read("F250").unwrap(), [250]);
        // 400 files of 2 sectors each; the directory (401 entries, 73
        // sectors) grew by 72 data sectors and a second map.
        assert_eq!(vol.free_sectors(), 1032 - 800 - 73);
        check_bitmap(&vol);
        // Too big for what's left: nothing changes.
        let before = vol.image().clone();
        let huge = vec![0; 200 * 128];
        assert!(matches!(
            vol.write("HUGE", &huge, None),
            Err(Error::DiskFull { .. })
        ));
        assert_eq!(*vol.image(), before);
    }

    #[test]
    fn sparta_never_reuses_a_used_sector() {
        // Mark the main directory's data sector (6) free, as on the
        // Turbo-Basic sample: a new file must not take it.
        let mut vol = sparta_blank();
        let mut bitmap = vol.read_sector(4).unwrap().to_vec();
        bitmap[0] |= 0x80 >> 6;
        vol.write_sector(4, &bitmap).unwrap();
        vol.write("A", &[9; 128], None).unwrap();
        vol.write("B", &[8; 128], None).unwrap();
        assert_eq!(vol.read("A").unwrap(), [9; 128]);
        assert_eq!(vol.list("").unwrap().len(), 2);
        // A damaged disk isn't changed at all.
        let mut map = vol.read_sector(5).unwrap().to_vec();
        map[4..6].copy_from_slice(&9999u16.to_le_bytes());
        vol.write_sector(5, &map).unwrap();
        assert!(matches!(vol.write("C", b"x", None), Err(Error::Damaged(_))));
        assert!(vol.list("").is_err());
    }

    #[test]
    fn raw_sectors_can_change_the_file_system() {
        let mut vol = sparta_blank();
        assert!(matches!(
            vol.write_sector(1, &[0; 64]),
            Err(Error::WrongSectorLength { .. })
        ));
        // Wiping sector 1 leaves no SpartaDOS: file calls fail, not panic.
        vol.write_sector(1, &[0; 128]).unwrap();
        assert!(vol.list("").is_err());
        assert_eq!(vol.free_sectors(), 0);
        // Writing a DOS 2 disk's sectors over it makes it DOS 2.
        let dos = Volume::format(Layout::Dos2Single, Container::Atr).unwrap();
        for n in 1..=720 {
            vol.write_sector(n, dos.read_sector(n).unwrap()).unwrap();
        }
        assert_eq!(vol.file_system(), FileSystem::Dos2);
        assert_eq!(vol.free_sectors(), 707);
    }
}
