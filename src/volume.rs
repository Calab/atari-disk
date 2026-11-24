//! [`Volume`]: one disk and its file system, behind the calls the editor
//! uses (list, read, write, rename, delete, lock, make a directory,
//! format), whichever DOS the disk is for.

use crate::dos2::{self, Dos2};
use crate::error::{Error, Result};
use crate::image::{Container, DiskImage};

/// The file systems a [`Volume`] can work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileSystem {
    /// Atari DOS 2.0S, 2.0D or 2.5 on a single- or double-density disk.
    Dos2,
    /// DOS 2.5 on an enhanced-density disk (1040 sectors, two VTOCs).
    Dos25Enhanced,
}

/// The blank disks [`Volume::format`] can make. They're data disks:
/// they hold no DOS, so they can't be booted.
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
    /// The sectors it takes up, as the directory says.
    pub sectors: u32,
    pub locked: bool,
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
///
/// ```
/// use atari_disk::{Container, Layout, Volume};
///
/// let mut vol = Volume::format(Layout::Dos2Single, Container::Atr)?;
/// vol.write("HELLO.TXT", b"Hello, Atari")?;
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

impl Volume {
    /// Which file system, if any, `image` holds. [`Volume::open`] takes
    /// the image, so ask this first if you want to keep the image when
    /// it isn't one this crate can read.
    pub fn detect(image: &DiskImage) -> Option<FileSystem> {
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
        let fs = Self::detect(&image).ok_or_else(|| {
            Error::UnknownFileSystem("not a DOS 2 disk (SpartaDOS is coming)".into())
        })?;
        Ok(Self {
            image,
            fs,
            uppercase: false,
            overwrite: false,
        })
    }

    /// A blank, formatted data disk.
    pub fn format(layout: Layout, container: Container) -> Result<Self> {
        let kind = match layout {
            Layout::Dos2Single => dos2::Kind::Single,
            Layout::Dos2Double => dos2::Kind::Double,
            Layout::Dos25Enhanced => dos2::Kind::Enhanced,
        };
        Self::open(dos2::format(container, kind)?)
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

    /// Free sectors, as the disk's DOS counts them.
    pub fn free_sectors(&self) -> u32 {
        self.dos2().free_sectors()
    }

    /// Free space in bytes: how long a file could be written. (DOS 2
    /// keeps 3 bytes of each sector for itself.)
    pub fn free_bytes(&self) -> u32 {
        self.free_sectors() * (self.image.sector_size() as u32 - 3)
    }

    /// The entries in directory `dir` (`""` for the top one), in
    /// directory order.
    pub fn list(&self, dir: &str) -> Result<Vec<DirEntry>> {
        if !dir.is_empty() {
            return Err(no_dirs());
        }
        Ok(self
            .dos2()
            .list()
            .into_iter()
            .map(|e| DirEntry {
                name: e.file_name(),
                kind: EntryKind::File,
                size: e.size,
                sectors: u32::from(e.sectors),
                locked: e.locked(),
                damaged: e.damaged,
            })
            .collect())
    }

    /// A file's contents.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.find(path)?;
        self.dos2().read(&entry)
    }

    /// Write a new file. If one of that name is already there this fails
    /// with [`Error::AlreadyExists`], unless [`Volume::set_overwrite`] is
    /// on; then it's replaced, unless it's locked ([`Error::Locked`]).
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let old = match self.find(path) {
            Ok(old) => Some(old),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        let name = match &old {
            Some(old) => {
                if !self.overwrite {
                    return Err(Error::AlreadyExists(old.file_name()));
                }
                check_unlocked(old)?;
                // Keep the name exactly as it is on the disk.
                let mut name = [0; 11];
                name[..8].copy_from_slice(&old.name);
                name[8..].copy_from_slice(&old.ext);
                name
            }
            None => dos2::parse_name(path, self.uppercase)?,
        };
        self.change(|image| dos2::write(image, old.as_ref(), &name, data))
    }

    /// Give a file the name `new_name` (a name only: it stays in its
    /// directory). Fails with [`Error::Locked`] if it's locked, or
    /// [`Error::AlreadyExists`] if another file has that name.
    pub fn rename(&mut self, path: &str, new_name: &str) -> Result<()> {
        let entry = self.find(path)?;
        check_unlocked(&entry)?;
        let name = dos2::parse_name(new_name, self.uppercase)?;
        if let Ok(other) = self.find(new_name)
            && other.index != entry.index
        {
            return Err(Error::AlreadyExists(other.file_name()));
        }
        self.change(|image| dos2::rename(image, &entry, &name))
    }

    /// Delete a file. Fails with [`Error::Locked`] if it's locked.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let entry = self.find(path)?;
        check_unlocked(&entry)?;
        self.change(|image| dos2::delete(image, &entry))
    }

    /// Lock a file, so it can't be written, renamed or deleted.
    pub fn lock(&mut self, path: &str) -> Result<()> {
        let entry = self.find(path)?;
        self.change(|image| dos2::set_locked(image, &entry, true))
    }

    /// Unlock a file.
    pub fn unlock(&mut self, path: &str) -> Result<()> {
        let entry = self.find(path)?;
        self.change(|image| dos2::set_locked(image, &entry, false))
    }

    /// Make a directory. Only SpartaDOS has them; on DOS 2 this fails
    /// with [`Error::Unsupported`].
    pub fn make_dir(&mut self, _path: &str) -> Result<()> {
        Err(no_dirs())
    }

    fn dos2(&self) -> Dos2<'_> {
        // `open` checked the disk, and changes keep it DOS 2.
        Dos2::open(&self.image).expect("a DOS 2 volume")
    }

    /// The existing file at `path`. Names are matched as stored, so
    /// files with names DOS 2 wouldn't make can still be found.
    fn find(&self, path: &str) -> Result<dos2::Entry> {
        if path.contains('>') {
            return Err(no_dirs());
        }
        let not_found = || Error::NotFound(path.to_string());
        let wanted = dos2::stored_name(path).ok_or_else(not_found)?;
        self.dos2()
            .list()
            .into_iter()
            .find(|e| {
                let mut name = [0; 11];
                name[..8].copy_from_slice(&e.name);
                name[8..].copy_from_slice(&e.ext);
                if self.uppercase {
                    name.eq_ignore_ascii_case(&wanted)
                } else {
                    name == wanted
                }
            })
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
}

fn no_dirs() -> Error {
    Error::Unsupported("DOS 2 has no directories".into())
}

fn check_unlocked(entry: &dos2::Entry) -> Result<()> {
    if entry.locked() {
        Err(Error::Locked(entry.file_name()))
    } else {
        Ok(())
    }
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
        vol.write("BIG.DAT", &big).unwrap();
        assert_eq!(vol.free_sectors(), 707 - 8);
        vol.write("EMPTY", b"").unwrap();
        assert_eq!(vol.free_sectors(), 707 - 9);
        assert_eq!(vol.read("BIG.DAT").unwrap(), big);
        assert_eq!(vol.read("EMPTY").unwrap(), b"");
        // Replacing frees the old sectors and keeps the directory place.
        assert!(matches!(
            vol.write("BIG.DAT", b"small"),
            Err(Error::AlreadyExists(_))
        ));
        assert_eq!(vol.read("BIG.DAT").unwrap(), big);
        vol.set_overwrite(true);
        vol.write("BIG.DAT", b"small").unwrap();
        assert_eq!(vol.free_sectors(), 707 - 2);
        assert_eq!(names(&vol), ["BIG.DAT", "EMPTY"]);
        let entry = &vol.list("").unwrap()[0];
        assert_eq!((entry.size, entry.sectors), (5, 1));
    }

    #[test]
    fn delete_reuses_the_entry_and_sectors() {
        let mut vol = blank();
        vol.write("A", &[1; 300]).unwrap();
        vol.write("B", &[2; 10]).unwrap();
        vol.delete("A").unwrap();
        assert_eq!(names(&vol), ["B"]);
        assert_eq!(vol.free_sectors(), 706);
        vol.write("C", &[3; 10]).unwrap();
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
                matches!(vol.write(bad, b"x"), Err(Error::BadName(_))),
                "{bad:?}"
            );
        }
        assert!(vol.write("GAMES>X", b"x").is_err());
        vol.set_auto_uppercase(true);
        vol.write("lower.txt", b"x").unwrap();
        assert_eq!(names(&vol), ["LOWER.TXT"]);
        assert_eq!(vol.read("Lower.Txt").unwrap(), b"x");
        vol.set_auto_uppercase(false);
        assert!(matches!(vol.read("lower.txt"), Err(Error::NotFound(_))));
    }

    #[test]
    fn rename() {
        let mut vol = blank();
        vol.write("OLD.TXT", b"x").unwrap();
        vol.write("OTHER", b"y").unwrap();
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
        vol.write("KEEP", b"x").unwrap();
        vol.lock("KEEP").unwrap();
        vol.set_overwrite(true);
        assert!(vol.list("").unwrap()[0].locked);
        for result in [
            vol.write("KEEP", b"y"),
            vol.rename("KEEP", "OTHER"),
            vol.delete("KEEP"),
        ] {
            assert!(matches!(result, Err(Error::Locked(_))));
        }
        assert_eq!(vol.read("KEEP").unwrap(), b"x");
        vol.unlock("KEEP").unwrap();
        vol.write("KEEP", b"y").unwrap();
        assert_eq!(vol.read("KEEP").unwrap(), b"y");
    }

    #[test]
    fn a_full_disk_leaves_everything_as_it_was() {
        let mut vol = blank();
        vol.write("FIRST", b"x").unwrap();
        let before = vol.image().clone();
        let too_big = vec![0; 707 * 125];
        assert!(matches!(
            vol.write("HUGE", &too_big),
            Err(Error::DiskFull {
                needed: 707,
                free: 706
            })
        ));
        assert_eq!(*vol.image(), before);
        // Replacing a file can use the file's own sectors.
        vol.set_overwrite(true);
        vol.write("FIRST", &too_big).unwrap();
        assert_eq!(vol.free_sectors(), 0);
        assert_eq!(vol.read("FIRST").unwrap(), too_big);
    }

    #[test]
    fn the_directory_holds_64_files() {
        let mut vol = blank();
        for i in 0..64 {
            vol.write(&format!("F{i}"), b"x").unwrap();
        }
        assert!(matches!(
            vol.write("ONEMORE", b"x"),
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
        vol.write("BIG", &data).unwrap();
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
        vol.write("SMALL", b"x").unwrap();
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
        assert!(matches!(vol.make_dir("GAMES"), Err(Error::Unsupported(_))));
        assert!(matches!(vol.list("GAMES"), Err(Error::Unsupported(_))));
    }
}
