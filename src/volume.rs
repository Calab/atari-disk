//! [`Volume`]: one disk and its file system, behind the calls a program
//! uses (list, read, write, rename, delete, lock, make a directory,
//! format, ...), whichever DOS the disk is for.

use std::ops::Range;

use crate::atascii::atascii_to_ascii;
use crate::check::{self, Problem, SectorUse};
use crate::dos2::{self, Dos2};
use crate::error::{Error, Result};
use crate::image::{Container, DiskImage};
use crate::names::{self, Name};
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
    /// MyDOS 4.x: DOS 2's file system with subdirectories and disks of up
    /// to 65535 sectors.
    MyDos,
    /// No file system this crate knows (or none at all): only the sector
    /// calls work ([`Volume::read_sector`], [`Volume::write_sector`]);
    /// file calls give [`Error::Unsupported`].
    Raw,
}

/// What a file system can do, from [`FileSystem::features`] or
/// [`Volume::features`], e.g. to show only the buttons that work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Features {
    /// Files at all (list, read, write, rename, delete): false only for
    /// [`FileSystem::Raw`].
    pub files: bool,
    /// Directories: `>` paths, [`Volume::make_dir`], [`Volume::move_to`].
    pub directories: bool,
    /// Locked files: [`Volume::lock`], [`Volume::unlock`].
    pub locking: bool,
    /// Dates on files and directories: [`Volume::set_timestamp`], and
    /// the `when` of [`Volume::write`].
    pub dates: bool,
    /// [`Volume::set_hidden`] and [`Volume::set_archived`].
    pub hidden_and_archived: bool,
    /// A volume name: [`Volume::set_volume_name`].
    pub volume_name: bool,
    /// Can be made bootable with [`Volume::make_bootable`].
    pub bootable: bool,
    /// The most entries one directory holds, if there's a fixed limit
    /// (64 on DOS 2 and MyDOS).
    pub max_entries: Option<u32>,
    /// New names may use `_` (SpartaDOS).
    pub underscore_in_names: bool,
    /// New names may start with a digit (SpartaDOS).
    pub digit_first_in_names: bool,
}

impl FileSystem {
    /// What it can do. [`Volume::features`] also knows the disk: a
    /// 512-byte-sector disk can't be bootable.
    pub fn features(self) -> Features {
        let sparta = self.is_sparta();
        let files = self != FileSystem::Raw;
        Features {
            files,
            directories: self.has_directories(),
            locking: files,
            dates: sparta,
            hidden_and_archived: sparta,
            volume_name: sparta,
            // SpartaDOS X boots from its cartridge.
            bootable: files && self != FileSystem::SpartaDosX,
            max_entries: (files && !sparta).then_some(64),
            underscore_in_names: sparta,
            digit_first_in_names: sparta,
        }
    }

    /// Does it have directories (so `>` paths, [`Volume::make_dir`] and
    /// [`Volume::move_to`])?
    pub fn has_directories(self) -> bool {
        matches!(
            self,
            FileSystem::SpartaDos | FileSystem::SpartaDosX | FileSystem::MyDos
        )
    }

    fn is_sparta(self) -> bool {
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
    /// MyDOS single density: 720 sectors of 128 bytes, 708 free.
    MyDosSingle,
    /// MyDOS on 1040 sectors of 128 bytes, 1027 free.
    MyDosEnhanced,
    /// MyDOS double density: 720 sectors of 256 bytes, 708 free.
    MyDosDouble,
    /// MyDOS with any number of sectors (369 to 65535) of 128 or 256
    /// bytes, e.g. 65535 × 256 for a hard disk partition.
    MyDosCustom { sectors: u32, sector_size: usize },
    /// SpartaDOS double-sided double density, as on an XF551: 1440
    /// sectors of 256 bytes (360 KB), 40 tracks a side.
    SpartaDoubleSided,
    /// SpartaDOS double-sided double density with 80 tracks a side: 2880
    /// sectors of 256 bytes (720 KB).
    SpartaDoubleSided80,
    /// MyDOS double-sided double density, as on an XF551: 1440 sectors of
    /// 256 bytes.
    MyDosDoubleSided,
    /// MyDOS double-sided double density with 80 tracks a side: 2880
    /// sectors of 256 bytes.
    MyDosDoubleSided80,
    /// No file system: `sectors` sectors of `sector_size` bytes (128, 256
    /// or 512), all zeros, for programs that write their own sectors.
    Raw { sectors: u32, sector_size: usize },
}

impl Layout {
    /// The file system a disk formatted this way has.
    pub fn file_system(self) -> FileSystem {
        match self {
            Layout::Dos2Single | Layout::Dos2Double => FileSystem::Dos2,
            Layout::Dos25Enhanced => FileSystem::Dos25Enhanced,
            Layout::SpartaCustom {
                sector_size: 512, ..
            } => FileSystem::SpartaDosX,
            Layout::SpartaSingle
            | Layout::SpartaEnhanced
            | Layout::SpartaDouble
            | Layout::SpartaCustom { .. }
            | Layout::SpartaDoubleSided
            | Layout::SpartaDoubleSided80 => FileSystem::SpartaDos,
            Layout::MyDosSingle
            | Layout::MyDosEnhanced
            | Layout::MyDosDouble
            | Layout::MyDosCustom { .. }
            | Layout::MyDosDoubleSided
            | Layout::MyDosDoubleSided80 => FileSystem::MyDos,
            Layout::Raw { .. } => FileSystem::Raw,
        }
    }

    /// The sectors and their size: `(count, bytes)`.
    pub fn geometry(self) -> (u32, usize) {
        match self {
            Layout::Dos2Single | Layout::SpartaSingle | Layout::MyDosSingle => (720, 128),
            Layout::Dos2Double | Layout::SpartaDouble | Layout::MyDosDouble => (720, 256),
            Layout::Dos25Enhanced | Layout::SpartaEnhanced | Layout::MyDosEnhanced => (1040, 128),
            Layout::SpartaDoubleSided | Layout::MyDosDoubleSided => (1440, 256),
            Layout::SpartaDoubleSided80 | Layout::MyDosDoubleSided80 => (2880, 256),
            Layout::SpartaCustom {
                sectors,
                sector_size,
            }
            | Layout::MyDosCustom {
                sectors,
                sector_size,
            }
            | Layout::Raw {
                sectors,
                sector_size,
            } => (sectors, sector_size),
        }
    }
}

/// Is a directory entry a file or a directory?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryKind {
    File,
    /// Only on file systems with directories (SpartaDOS, MyDOS).
    Directory,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirEntry {
    /// The name as stored, `NAME.EXT` in ATASCII (no dot when there's no
    /// extension): give it back in a path to reach the entry. See
    /// [`DirEntry::name_text`] to show it.
    pub name: Vec<u8>,
    pub kind: EntryKind,
    /// The length in bytes.
    pub size: u32,
    /// The sectors it takes up (on SpartaDOS, its sector maps included).
    pub sectors: u32,
    pub locked: bool,
    /// Left out of DIR listings (SpartaDOS only).
    pub hidden: bool,
    /// SpartaDOS X's archive flag (SpartaDOS only).
    pub archived: bool,
    /// When it was last written, if the file system keeps the date
    /// (SpartaDOS does, DOS 2 doesn't) and it's set.
    pub modified: Option<Timestamp>,
    /// The file system's records for it don't add up, so reading it will
    /// fail (and `size` is only what could be followed).
    pub damaged: bool,
}

impl DirEntry {
    /// The name as ASCII text to show, with any character ASCII doesn't
    /// have as `?` (see [`crate::atascii::atascii_to_ascii`]).
    pub fn name_text(&self) -> String {
        atascii_to_ascii(&self.name)
    }

    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

/// A disk image and the file system on it. The volume owns the image;
/// [`Volume::image`] gives it back to save, e.g. with
/// [`DiskImage::to_bytes`].
///
/// Paths are ATASCII bytes (see [`crate::atascii`]), with `>` between
/// directories as SpartaDOS writes them: `"GAMES>PACMAN.COM"`, with no
/// `D:` and no leading `>`. Ordinary names can be given as Rust strings.
/// On DOS 2, which has no directories, a path is just a file name.
///
/// Every change either succeeds completely or leaves the disk as it was.
/// On SpartaDOS, changes are refused with [`Error::Damaged`] if any file
/// or directory on the disk can't be followed, and new files never take a
/// sector something else uses, even if the bitmap says it's free.
///
/// The ATR write-protect flag is only reported
/// ([`DiskImage::write_protected`]): changes are still made, and the
/// program decides whether to allow them.
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

    fn is_dir(&self) -> bool {
        match self {
            Found::Dos2(e) => e.is_dir(),
            Found::Sparta(e) => e.is_dir(),
        }
    }

    fn check_unlocked(&self) -> Result<()> {
        let locked = match self {
            Found::Dos2(e) => e.locked(),
            Found::Sparta(e) => e.locked(),
        };
        if locked {
            Err(Error::Locked(self.name()))
        } else {
            Ok(())
        }
    }

    fn sparta(self) -> Option<sparta::Entry> {
        match self {
            Found::Sparta(e) => Some(e),
            Found::Dos2(_) => None,
        }
    }

    fn dos2(self) -> Option<dos2::Entry> {
        match self {
            Found::Dos2(e) => Some(e),
            Found::Sparta(_) => None,
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
        Some(match fs.flavour() {
            dos2::Flavour::Dos2 => FileSystem::Dos2,
            dos2::Flavour::Enhanced => FileSystem::Dos25Enhanced,
            dos2::Flavour::MyDos => FileSystem::MyDos,
        })
    }

    /// Open the file system on `image`. When [`Volume::detect`] finds
    /// none, the volume is [`FileSystem::Raw`]: the sector calls work and
    /// the file calls give [`Error::Unsupported`]. (Ask
    /// [`Volume::detect`] first to tell a disk with no file system from
    /// one with.)
    pub fn open(image: DiskImage) -> Result<Self> {
        let fs = Self::detect(&image).unwrap_or(FileSystem::Raw);
        Ok(Self::with(image, fs))
    }

    /// Open `image` as [`FileSystem::Raw`] whatever it holds: sectors
    /// only, e.g. to repair a damaged disk by hand.
    pub fn open_raw(image: DiskImage) -> Self {
        Self::with(image, FileSystem::Raw)
    }

    fn with(image: DiskImage, fs: FileSystem) -> Self {
        Self {
            image,
            fs,
            uppercase: false,
            overwrite: false,
        }
    }

    /// What this disk's file system can do (see [`Features`]).
    pub fn features(&self) -> Features {
        let mut f = self.fs.features();
        if self.image.sector_size() > 256 {
            f.bootable = false;
        }
        f
    }

    /// A blank, formatted data disk. A SpartaDOS one has no volume name
    /// until [`Volume::set_volume_name`] gives it one.
    pub fn format(layout: Layout, container: Container) -> Result<Self> {
        let image = match layout {
            Layout::Dos2Single => dos2::format(container, dos2::Kind::Single)?,
            Layout::Dos2Double => dos2::format(container, dos2::Kind::Double)?,
            Layout::Dos25Enhanced => dos2::format(container, dos2::Kind::Enhanced)?,
            Layout::SpartaSingle => sparta::format(container, 128, 720, b"")?,
            Layout::SpartaEnhanced => sparta::format(container, 128, 1040, b"")?,
            Layout::SpartaDouble => sparta::format(container, 256, 720, b"")?,
            Layout::SpartaCustom {
                sectors,
                sector_size,
            } => sparta::format(container, sector_size, sectors, b"")?,
            Layout::MyDosSingle => mydos(container, 128, 720)?,
            Layout::MyDosEnhanced => mydos(container, 128, 1040)?,
            Layout::MyDosDouble => mydos(container, 256, 720)?,
            Layout::MyDosCustom {
                sectors,
                sector_size,
            } => mydos(container, sector_size, sectors)?,
            Layout::SpartaDoubleSided => sparta::format(container, 256, 1440, b"")?,
            Layout::SpartaDoubleSided80 => sparta::format(container, 256, 2880, b"")?,
            Layout::MyDosDoubleSided => mydos(container, 256, 1440)?,
            Layout::MyDosDoubleSided80 => mydos(container, 256, 2880)?,
            Layout::Raw {
                sectors,
                sector_size,
            } => {
                return Ok(Self::open_raw(DiskImage::new(
                    container,
                    sector_size,
                    sectors,
                )?));
            }
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

    /// Set or clear the ATR write-protect flag (see
    /// [`DiskImage::set_write_protected`]).
    pub fn set_write_protected(&mut self, on: bool) -> Result<()> {
        self.image.set_write_protected(on)
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

    /// Is `name` allowed as a new file or directory name on this disk?
    /// `Ok`, or [`Error::BadName`] saying why not. (A name only, no `>`.
    /// Lower case passes only with [`Volume::set_auto_uppercase`] on.)
    pub fn check_name(&self, name: impl AsRef<[u8]>) -> Result<()> {
        self.parse_name(name.as_ref()).map(|_| ())
    }

    /// The volume name as stored (ATASCII), on file systems that have one
    /// (SpartaDOS).
    pub fn volume_name(&self) -> Option<Vec<u8>> {
        self.sparta().ok().map(|fs| fs.volume_name())
    }

    /// Set the volume name: up to 8 printable ATASCII characters
    /// (SpartaDOS only; DOS 2 disks have none, [`Error::Unsupported`]).
    pub fn set_volume_name(&mut self, name: impl AsRef<[u8]>) -> Result<()> {
        self.need_sparta("volume names")?;
        self.change_sparta(|w| w.set_volume_name(name.as_ref()))
    }

    /// Free sectors, as the disk's DOS counts them.
    pub fn free_sectors(&self) -> u32 {
        if self.fs.is_sparta() {
            self.sparta().map_or(0, |fs| fs.free_sectors())
        } else {
            self.dos2().map_or(0, |fs| fs.free_sectors())
        }
    }

    /// Free space in bytes: about how long a file could be written. DOS 2
    /// keeps 3 bytes of each sector for itself; SpartaDOS needs a sector
    /// map for every 62 data sectors (126 on 256-byte sectors, 254 on
    /// 512). A new file may also need a sector for its directory, which
    /// this doesn't count; [`Volume::fits`] gives an exact answer.
    pub fn free_bytes(&self) -> u32 {
        let free = self.free_sectors();
        let size = self.image.sector_size() as u32;
        if self.fs.is_sparta() {
            let per_map = (size - 4) / 2;
            (free - free.div_ceil(per_map + 1)) * size
        } else {
            free * (size - 3)
        }
    }

    /// The entries in directory `dir`, in directory order: `""` for the
    /// top one, or e.g. `"GAMES>OLD"` on SpartaDOS (a trailing `>` is
    /// fine). Fails with [`Error::NotFound`] if there's no such
    /// directory, or [`Error::NotADirectory`] if it's a file.
    pub fn list(&self, dir: impl AsRef<[u8]>) -> Result<Vec<DirEntry>> {
        let dir = dir.as_ref();
        if self.fs.is_sparta() {
            let entries = self.sparta()?.list_path(dir, |e, n| self.same(e, n))?;
            return Ok(entries.iter().map(sparta_entry).collect());
        }
        if !dir.is_empty() {
            self.need_dirs()?;
        }
        let fs = self.dos2()?;
        let first = fs.dir_first(dir, |e, n| self.same_dos2(e, n))?;
        Ok(fs.list_dir(first)?.iter().map(dos2_entry).collect())
    }

    /// The entry at `path`, without listing its whole directory.
    pub fn entry(&self, path: impl AsRef<[u8]>) -> Result<DirEntry> {
        Ok(match self.find(path.as_ref())? {
            Found::Dos2(e) => dos2_entry(&e),
            Found::Sparta(e) => sparta_entry(&e),
        })
    }

    /// Is there a file or directory at `path`?
    pub fn exists(&self, path: impl AsRef<[u8]>) -> bool {
        self.find(path.as_ref()).is_ok()
    }

    /// A file's contents. Fails with [`Error::IsDirectory`] for a
    /// directory.
    pub fn read(&self, path: impl AsRef<[u8]>) -> Result<Vec<u8>> {
        match self.find(path.as_ref())? {
            Found::Dos2(e) => self.dos2()?.read(&e),
            Found::Sparta(e) if e.is_dir() => Err(Error::IsDirectory(e.file_name())),
            Found::Sparta(e) => self.sparta()?.read(&e),
        }
    }

    /// Bytes `range` of a file (e.g. `100..200`); a range running past
    /// the end gives what there is, and one starting past it gives none.
    pub fn read_range(&self, path: impl AsRef<[u8]>, range: Range<usize>) -> Result<Vec<u8>> {
        let data = self.read(path)?;
        let end = range.end.min(data.len());
        let start = range.start.min(end);
        Ok(data[start..end].to_vec())
    }

    /// Write a new file. If one of that name is already there this fails
    /// with [`Error::AlreadyExists`], unless [`Volume::set_overwrite`] is
    /// on; then it's replaced, unless it's locked ([`Error::Locked`]).
    ///
    /// `when` is the date and time to give it, on file systems that keep
    /// one (SpartaDOS). `None` leaves a new file without a date, and keeps
    /// the date of a file it replaces. DOS 2 ignores it.
    pub fn write(
        &mut self,
        path: impl AsRef<[u8]>,
        data: &[u8],
        when: Option<Timestamp>,
    ) -> Result<()> {
        let path = path.as_ref();
        let old = match self.find(path) {
            Ok(old) => Some(old),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        if let Some(old) = &old {
            if !self.overwrite || old.is_dir() {
                return Err(Error::AlreadyExists(old.name()));
            }
            old.check_unlocked()?;
        }
        let (dir, name) = names::split_path(path);
        if self.fs.is_sparta() {
            let when = when.map(Timestamp::to_bytes).transpose()?;
            let old = old.and_then(Found::sparta);
            let stored = match &old {
                // Keep the name exactly as it is on the disk.
                Some(e) => stored_of(&e.name, &e.ext),
                None => self.parse_name(name)?,
            };
            let map = self.sparta()?.dir_map(dir, |e, n| self.same(e, n))?;
            return self.change_sparta(|w| w.write(map, old.as_ref(), &stored, data, when));
        }
        let old = old.and_then(Found::dos2);
        let stored = match &old {
            Some(e) => stored_of(&e.name, &e.ext),
            None => self.parse_name(name)?,
        };
        let first = self.dos2()?.dir_first(dir, |e, n| self.same_dos2(e, n))?;
        self.change(|image| dos2::write(image, first, old.as_ref(), &stored, data))
    }

    /// Would [`Volume::write`] of `len` bytes to `path` fit? `Ok(false)`
    /// when the disk or directory is too full; other problems (a bad
    /// name, a locked file, ...) are errors, as `write` would give them.
    pub fn fits(&self, path: impl AsRef<[u8]>, len: usize) -> Result<bool> {
        let mut trial = self.clone();
        match trial.write(path, &vec![0; len], None) {
            Ok(()) => Ok(true),
            Err(Error::DiskFull { .. } | Error::DirectoryFull) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Give a file (or directory) the name `new_name` (a name only: it
    /// stays in its directory; see [`Volume::move_to`]). Fails with
    /// [`Error::Locked`] if it's locked, or [`Error::AlreadyExists`] if
    /// another entry has that name.
    pub fn rename(&mut self, path: impl AsRef<[u8]>, new_name: impl AsRef<[u8]>) -> Result<()> {
        let (path, new_name) = (path.as_ref(), new_name.as_ref());
        let entry = self.find(path)?;
        entry.check_unlocked()?;
        let name = self.parse_name(new_name)?;
        let (dir, _) = names::split_path(path);
        let sibling = [dir, if dir.is_empty() { b"" } else { b">" }, new_name].concat();
        if let Ok(other) = self.find(&sibling) {
            let same_entry = match (&other, &entry) {
                (Found::Dos2(a), Found::Dos2(b)) => (a.dir, a.index) == (b.dir, b.index),
                (Found::Sparta(a), Found::Sparta(b)) => (a.parent, a.index) == (b.parent, b.index),
                _ => false,
            };
            if !same_entry {
                return Err(Error::AlreadyExists(other.name()));
            }
        }
        match entry {
            Found::Dos2(e) => self.change(|image| dos2::rename(image, &e, &name)),
            Found::Sparta(e) => self.change_sparta(|w| w.rename(&e, &name)),
        }
    }

    /// Move a file (or directory) into directory `dir` (`""` for the top
    /// one), keeping its name and contents. Only on file systems with
    /// directories ([`Error::Unsupported`] otherwise, or for moving a
    /// directory into itself). Fails with [`Error::Locked`] if it's
    /// locked, or [`Error::AlreadyExists`] if `dir` has that name already.
    pub fn move_to(&mut self, path: impl AsRef<[u8]>, dir: impl AsRef<[u8]>) -> Result<()> {
        let (path, dir) = (path.as_ref(), dir.as_ref());
        self.need_dirs()?;
        let entry = self.find(path)?;
        entry.check_unlocked()?;
        let entry = match entry {
            Found::Sparta(e) => e,
            Found::Dos2(e) => {
                let fs = self.dos2()?;
                let target = fs.dir_first(dir, |e, n| self.same_dos2(e, n))?;
                if target == e.dir {
                    return Ok(());
                }
                let stored = e.stored_name();
                if let Some(other) = fs
                    .list_dir(target)?
                    .into_iter()
                    .find(|o| self.same_dos2(o, &stored))
                {
                    return Err(Error::AlreadyExists(other.file_name()));
                }
                return self.change(|image| dos2::move_entry(image, &e, target));
            }
        };
        let fs = self.sparta()?;
        let target = fs.dir_map(dir, |e, n| self.same(e, n))?;
        if target == entry.parent {
            return Ok(());
        }
        let stored = entry.stored_name();
        if let Some(other) = fs
            .list_dir(target)?
            .into_iter()
            .find(|e| self.same(e, &stored))
        {
            return Err(Error::AlreadyExists(other.file_name()));
        }
        self.change_sparta(|w| w.move_entry(&entry, target))
    }

    /// Delete a file, or an empty directory. Fails with [`Error::Locked`]
    /// if it's locked, or [`Error::DirectoryNotEmpty`].
    pub fn delete(&mut self, path: impl AsRef<[u8]>) -> Result<()> {
        let entry = self.find(path.as_ref())?;
        entry.check_unlocked()?;
        match entry {
            Found::Dos2(e) => self.change(|image| dos2::delete(image, &e)),
            Found::Sparta(e) => self.change_sparta(|w| w.delete(&e)),
        }
    }

    /// Lock a file (or directory), so it can't be written, renamed, moved
    /// or deleted.
    pub fn lock(&mut self, path: impl AsRef<[u8]>) -> Result<()> {
        self.set_locked(path.as_ref(), true)
    }

    /// Unlock a file (or directory).
    pub fn unlock(&mut self, path: impl AsRef<[u8]>) -> Result<()> {
        self.set_locked(path.as_ref(), false)
    }

    fn set_locked(&mut self, path: &[u8], locked: bool) -> Result<()> {
        match self.find(path)? {
            Found::Dos2(e) => self.change(|image| dos2::set_locked(image, &e, locked)),
            Found::Sparta(e) => self.change_sparta(|w| w.set_locked(&e, locked)),
        }
    }

    /// Set the date and time of a file or directory (`None` clears it),
    /// without changing anything else. SpartaDOS only
    /// ([`Error::Unsupported`] otherwise).
    pub fn set_timestamp(&mut self, path: impl AsRef<[u8]>, when: Option<Timestamp>) -> Result<()> {
        let entry = self.find_sparta(path.as_ref(), "dates")?;
        let when = when.map(Timestamp::to_bytes).transpose()?;
        self.change_sparta(|w| w.set_timestamp(&entry, when))
    }

    /// Set or clear the hidden flag (SpartaDOS only).
    pub fn set_hidden(&mut self, path: impl AsRef<[u8]>, on: bool) -> Result<()> {
        let entry = self.find_sparta(path.as_ref(), "hidden files")?;
        self.change_sparta(|w| w.set_hidden(&entry, on))
    }

    /// Set or clear SpartaDOS X's archive flag (SpartaDOS only).
    pub fn set_archived(&mut self, path: impl AsRef<[u8]>, on: bool) -> Result<()> {
        let entry = self.find_sparta(path.as_ref(), "archive flags")?;
        self.change_sparta(|w| w.set_archived(&entry, on))
    }

    /// Make an empty directory, e.g. `"GAMES"` or `"GAMES>OLD"` (its
    /// parent must be there). `when` is its date and time, or `None` for
    /// none. Only on file systems with directories; on DOS 2 this fails
    /// with [`Error::Unsupported`].
    pub fn make_dir(&mut self, path: impl AsRef<[u8]>, when: Option<Timestamp>) -> Result<()> {
        let path = path.as_ref();
        self.need_dirs()?;
        match self.find(path) {
            Ok(old) => return Err(Error::AlreadyExists(old.name())),
            Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let when = when.map(Timestamp::to_bytes).transpose()?;
        let (dir, name) = names::split_path(path);
        let name = self.parse_name(name)?;
        if !self.fs.is_sparta() {
            // MyDOS keeps no dates.
            let first = self.dos2()?.dir_first(dir, |e, n| self.same_dos2(e, n))?;
            return self.change(|image| dos2::make_dir(image, first, &name));
        }
        let map = self.sparta()?.dir_map(dir, |e, n| self.same(e, n))?;
        self.change_sparta(|w| w.make_dir(map, &name, when))
    }

    /// Is the disk set up to boot a DOS? Only the boot sector is looked
    /// at, not whether the DOS it points to is really there or works:
    ///
    /// * DOS 2 and MyDOS: sector 1 loads at least one sector, says DOS.SYS
    ///   is present (byte `$0E`), and points to a sector on the disk for
    ///   it (bytes `$0F`-`$10`).
    /// * SpartaDOS: sector 1 loads at least one sector and points to a
    ///   boot file's sector map on the disk (bytes `$28`-`$29`).
    ///
    /// Blank data disks from [`Volume::format`] aren't bootable; nor are
    /// raw disks, disks with 512-byte sectors, or SpartaDOS X disks
    /// (which boot from the cartridge).
    pub fn is_bootable(&self) -> bool {
        if self.fs == FileSystem::Raw || self.image.sector_size() > 256 {
            return false;
        }
        let Ok(s1) = self.image.sector(1) else {
            return false;
        };
        let on_disk = |n: u16| (4..=self.image.sector_count()).contains(&u32::from(n));
        let word = |at: usize| u16::from_le_bytes([s1[at], s1[at + 1]]);
        s1[1] != 0
            && if self.fs.is_sparta() {
                on_disk(word(0x28))
            } else {
                s1[0x0E] != 0 && on_disk(word(0x0F))
            }
    }

    /// Make this disk bootable with the DOS on `source`, a bootable disk
    /// of the same kind (the program loads it; this crate reads no
    /// files). Nothing copyrighted comes with this crate: the DOS is
    /// whatever `source` holds.
    ///
    /// * DOS 2 (2.0S, 2.0D, 2.5) and MyDOS: the boot sectors and the file
    ///   they load (as `DOS.SYS`), and `DUP.SYS` if `source` has one, with
    ///   the boot sector pointed at the new DOS.SYS and set for this
    ///   disk's sector size. (DOS 2.5 itself can't use double density.)
    /// * SpartaDOS 2.x/3.x: the boot code, and the file it loads (e.g.
    ///   `X32G.DOS`) under its own name; this disk keeps its own volume
    ///   information. SpartaDOS X boots from its cartridge, so its disks
    ///   have no boot file to copy.
    ///
    /// The files are written like [`Volume::write`] would, so one that's
    /// already here gives [`Error::AlreadyExists`] unless
    /// [`Volume::set_overwrite`] is on. Fails with [`Error::NotBootable`]
    /// if `source` has no boot file, is for a different DOS, or this disk
    /// has 512-byte sectors (the Atari can't boot from those).
    pub fn make_bootable(&mut self, source: &DiskImage) -> Result<()> {
        let not = |why: &str| Err(Error::NotBootable(why.into()));
        if self.image.sector_size() > 256 {
            return not("the Atari can't boot from 512-byte sectors");
        }
        if !self.features().bootable {
            return not(match self.fs {
                FileSystem::SpartaDosX => "SpartaDOS X boots from its cartridge, not the disk",
                _ => "this disk has no file system",
            });
        }
        let src = Volume::open(source.clone())?;
        if src.fs == FileSystem::Raw {
            return not("the source disk has no DOS 2, MyDOS or SpartaDOS");
        }
        let family = |fs: FileSystem| match fs {
            FileSystem::Dos2 | FileSystem::Dos25Enhanced => 0,
            FileSystem::MyDos => 1,
            _ => 2,
        };
        if family(src.fs) != family(self.fs) {
            return not("the source disk is for a different DOS");
        }
        let boot: Vec<u8> = (1..=3)
            .map(|n| source.sector(n).map(|s| s[..128].to_vec()))
            .collect::<Result<Vec<_>>>()?
            .concat();
        let mut trial = self.clone();
        let mut sectors = boot.clone();
        if self.fs.is_sparta() {
            let fs = src.sparta()?;
            let map = u16::from_le_bytes([boot[0x28], boot[0x29]]);
            if map == 0 {
                return not(
                    "the source disk has no boot file (SpartaDOS X boots from its cartridge)",
                );
            }
            let entry = fs
                .list_dir(fs.root())?
                .into_iter()
                .find(|e| e.map == map && !e.is_dir())
                .ok_or_else(|| {
                    Error::NotBootable("the source's boot file isn't in its main directory".into())
                })?;
            let data = fs.read(&entry)?;
            let name = entry.stored_name();
            trial.write(&name, &data, entry.timestamp())?;
            let new_map = trial.sparta()?.find(&name)?.map;
            // Keep this disk's own volume information ($09-$2A).
            let own = trial.image.sector(1)?;
            sectors[0x09..0x2B].copy_from_slice(&own[0x09..0x2B]);
            sectors[0x28..0x2A].copy_from_slice(&new_map.to_le_bytes());
        } else {
            let fs = src.dos2()?;
            let start = u16::from_le_bytes([boot[0x0F], boot[0x10]]);
            if boot[0x0E] == 0 || start == 0 {
                return not("the source disk has no DOS.SYS to boot");
            }
            // The file the boot sector loads, wherever it is.
            let mut dirs = vec![fs.root()];
            let mut dos = None;
            while let Some(d) = dirs.pop() {
                for e in fs.list_dir(d)? {
                    if e.is_dir() {
                        dirs.push(u32::from(e.start));
                    } else if e.start == start && dos.is_none() {
                        dos = Some(e);
                    }
                }
                if dirs.len() > 4096 {
                    return Err(Error::Damaged(
                        "the source's directories go round in a loop".into(),
                    ));
                }
            }
            let dos = dos.ok_or_else(|| {
                Error::NotBootable("the source's boot sector points at no file".into())
            })?;
            trial.write("DOS.SYS", &fs.read(&dos)?, None)?;
            if let Some(dup) = fs.find("DUP.SYS") {
                trial.write("DUP.SYS", &fs.read(&dup)?, None)?;
            }
            let new = trial.dos2()?.find("DOS.SYS").expect("just written");
            let size = trial.image.sector_size();
            sectors[0x0E] = (size / 128) as u8;
            sectors[0x0F..0x11].copy_from_slice(&new.start.to_le_bytes());
            sectors[0x11] = (size - 3) as u8;
        }
        for n in 1..=3u32 {
            let at = (n as usize - 1) * 128;
            let len = trial.image.sector_len(n);
            let mut sector = trial.image.sector(n)?.to_vec();
            sector[..128].copy_from_slice(&sectors[at..at + 128]);
            sector.truncate(len);
            trial.image.write_sector(n, &sector)?;
        }
        *self = trial;
        Ok(())
    }

    /// What each sector holds: index `n` is sector `n` (index 0, which
    /// doesn't exist, is [`SectorUse::Reserved`]). Where two things claim
    /// a sector it shows the first; [`Volume::validate`] reports both.
    pub fn sector_map(&self) -> Result<Vec<SectorUse>> {
        Ok(self.check()?.0)
    }

    /// Check the whole disk, changing nothing: every file and directory
    /// can be followed, no sector is claimed twice, the bitmap marks
    /// exactly the sectors in use, and the free count matches it. An
    /// empty list means the disk is sound. Fails only if the file system
    /// can't be read at all.
    pub fn validate(&self) -> Result<Vec<Problem>> {
        Ok(self.check()?.1)
    }

    fn check(&self) -> Result<(Vec<SectorUse>, Vec<Problem>)> {
        if self.fs == FileSystem::Raw {
            return Err(raw());
        }
        let result = if self.fs.is_sparta() {
            check::sparta(&self.image)
        } else {
            check::dos2(&self.image)
        };
        result.map_err(|e| Error::Damaged(e.to_string()))
    }

    fn need_dirs(&self) -> Result<()> {
        match self.fs {
            FileSystem::Raw => Err(raw()),
            fs if fs.has_directories() => Ok(()),
            _ => Err(no_dirs()),
        }
    }

    fn need_sparta(&self, what: &str) -> Result<()> {
        match self.fs {
            FileSystem::Raw => Err(raw()),
            fs if fs.is_sparta() => Ok(()),
            FileSystem::MyDos => Err(Error::Unsupported(format!("MyDOS has no {what}"))),
            _ => Err(Error::Unsupported(format!("DOS 2 has no {what}"))),
        }
    }

    fn find_sparta(&self, path: &[u8], what: &str) -> Result<sparta::Entry> {
        self.need_sparta(what)?;
        Ok(self.find(path)?.sparta().expect("a SpartaDOS entry"))
    }

    /// Check a new name against this file system's rules.
    fn parse_name(&self, name: &[u8]) -> Result<Name> {
        let rules = if self.fs.is_sparta() {
            names::SPARTA
        } else {
            names::DOS2
        };
        names::parse(name, self.uppercase, rules)
    }

    fn dos2(&self) -> Result<Dos2<'_>> {
        if self.fs == FileSystem::Raw {
            return Err(raw());
        }
        // A direct sector write may have broken it.
        Dos2::open(&self.image).map_err(|e| Error::Damaged(e.to_string()))
    }

    fn sparta(&self) -> Result<Sparta<'_>> {
        if self.fs == FileSystem::Raw {
            return Err(raw());
        }
        if !self.fs.is_sparta() {
            return Err(Error::Unsupported("not a SpartaDOS disk".into()));
        }
        Sparta::open(&self.image).map_err(|e| Error::Damaged(e.to_string()))
    }

    /// Is `wanted` the stored name of `e`? See [`Volume::same_name`].
    fn same(&self, e: &sparta::Entry, wanted: &[u8]) -> bool {
        self.same_name(&e.name, &e.ext, wanted)
    }

    fn same_dos2(&self, e: &dos2::Entry, wanted: &[u8]) -> bool {
        self.same_name(&e.name, &e.ext, wanted)
    }

    /// Is `wanted` the stored name `name` + `ext`? Names are matched as
    /// stored, so files with names DOS wouldn't make can still be found;
    /// with auto-uppercase on, case is ignored.
    fn same_name(&self, name: &[u8], ext: &[u8], wanted: &[u8]) -> bool {
        let Some(wanted) = names::stored(wanted) else {
            return false;
        };
        let stored = stored_of(name, ext);
        if self.uppercase {
            stored.eq_ignore_ascii_case(&wanted)
        } else {
            stored == wanted
        }
    }

    /// The existing entry at `path`.
    fn find(&self, path: &[u8]) -> Result<Found> {
        let not_found = || Error::NotFound(atascii_to_ascii(path));
        let (dir, name) = names::split_path(path);
        if self.fs.is_sparta() {
            return self
                .sparta()?
                .list_path(dir, |e, n| self.same(e, n))?
                .into_iter()
                .find(|e| self.same(e, name))
                .map(Found::Sparta)
                .ok_or_else(not_found);
        }
        if !dir.is_empty() {
            self.need_dirs()?;
        }
        let fs = self.dos2()?;
        let first = fs.dir_first(dir, |e, n| self.same_dos2(e, n))?;
        fs.list_dir(first)?
            .into_iter()
            .find(|e| self.same_dos2(e, name))
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

fn stored_of(name: &[u8], ext: &[u8]) -> Name {
    let mut out = [b' '; 11];
    out[..8].copy_from_slice(&name[..8]);
    out[8..].copy_from_slice(&ext[..3]);
    out
}

fn dos2_entry(e: &dos2::Entry) -> DirEntry {
    DirEntry {
        name: e.stored_name(),
        kind: if e.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: e.size,
        sectors: u32::from(e.sectors),
        locked: e.locked(),
        hidden: false,
        archived: false,
        modified: None,
        damaged: e.damaged,
    }
}

fn sparta_entry(e: &sparta::Entry) -> DirEntry {
    DirEntry {
        name: e.stored_name(),
        kind: if e.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: e.size,
        sectors: e.sectors,
        locked: e.locked(),
        hidden: e.hidden(),
        archived: e.archived(),
        modified: e.timestamp(),
        damaged: e.damaged,
    }
}

fn mydos(container: Container, size: usize, count: u32) -> Result<DiskImage> {
    dos2::format(container, dos2::Kind::MyDos { size, count })
}

fn raw() -> Error {
    Error::Unsupported("a raw disk has no file system, only sectors".into())
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
        vol.list("")
            .unwrap()
            .into_iter()
            .map(|e| e.name_text())
            .collect()
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
            vol.write(format!("F{i}"), b"x", None).unwrap();
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
        Timestamp::new(2026, 11, day, 9, 5, 30).unwrap()
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
        assert_eq!(vol.validate().unwrap(), []);
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
            assert_eq!(vol.volume_name().as_deref(), Some(&b""[..]));
            check_bitmap(&vol);
            // SpartaDOS's mark, and the boot sector that fails politely.
            assert_eq!(vol.image().sector(1).unwrap()[7], 0x80);
            vol.set_volume_name("My Disk").unwrap();
            assert_eq!(vol.volume_name().as_deref(), Some(&b"My Disk"[..]));
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
        assert_eq!(root[0].name, b"README.TXT");
        assert_eq!(root[0].modified, Some(when(1)));
        assert_eq!(
            (root[1].name_text().as_str(), root[1].kind),
            ("GAMES", EntryKind::Directory)
        );
        assert_eq!(root[1].modified, Some(when(2)));
        let old = vol.list("GAMES>OLD>").unwrap();
        assert_eq!(
            (old[0].name_text().as_str(), old[0].size, old[0].sectors),
            ("PAC_1.COM", 200, 3)
        );
        assert_eq!(vol.read("GAMES>OLD>PAC_1.COM").unwrap(), [7; 200]);
        assert!(matches!(vol.read("GAMES"), Err(Error::IsDirectory(_))));
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
            vol.write(format!("F{i}"), &[i as u8], None).unwrap();
        }
        let names: Vec<String> = vol
            .list("")
            .unwrap()
            .into_iter()
            .map(|e| e.name_text())
            .collect();
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

    // ---- The calls added for general use ----

    #[test]
    fn odd_names_can_be_reached() {
        // A name with an inverse character, as old disks sometimes have.
        let mut vol = sparta_blank();
        vol.write("PLAIN", b"x", None).unwrap();
        let mut image = vol.into_image();
        let index = Sparta::open(&image).unwrap().find("PLAIN").unwrap().index;
        // Patch the stored name in the main directory's data sector (6).
        let mut sector = image.sector(6).unwrap().to_vec();
        let at = index as usize * 23 + 7;
        sector[at] = 0xC1; // inverse A: "P" then inverse "A"
        image.write_sector(6, &sector).unwrap();
        let mut vol = Volume::open(image).unwrap();
        let listed = vol.list("").unwrap();
        assert_eq!(listed[0].name, b"P\xc1AIN");
        assert_eq!(listed[0].name_text(), "P?AIN");
        // The stored bytes reach it; the shown text doesn't.
        assert_eq!(vol.read(&listed[0].name).unwrap(), b"x");
        assert!(vol.read("P?AIN").is_err());
        vol.rename(&listed[0].name, "NORMAL").unwrap();
        assert_eq!(vol.read("NORMAL").unwrap(), b"x");
        // New names still follow the rules.
        assert!(matches!(vol.check_name(b"A\xc1"), Err(Error::BadName(_))));
        assert!(vol.check_name("1ST_ONE.TXT").is_ok());
        assert!(blank().check_name("1ST").is_err());
    }

    #[test]
    fn entry_exists_and_read_range() {
        let mut vol = sparta_blank();
        vol.make_dir("D", None).unwrap();
        let data: Vec<u8> = (0..500).map(|i| i as u8).collect();
        vol.write("D>F", &data, Some(when(3))).unwrap();
        let e = vol.entry("D>F").unwrap();
        assert_eq!(
            (e.size, e.modified, e.is_dir()),
            (500, Some(when(3)), false)
        );
        assert!(vol.entry("D").unwrap().is_dir());
        assert!(vol.exists("D>F") && !vol.exists("D>G") && !vol.exists("X>F"));
        assert_eq!(vol.read_range("D>F", 100..103).unwrap(), [100, 101, 102]);
        assert_eq!(vol.read_range("D>F", 498..600).unwrap(), [242, 243]);
        assert!(vol.read_range("D>F", 900..1000).unwrap().is_empty());
        assert!(matches!(vol.list("D>F"), Err(Error::NotADirectory(_))));
        assert!(matches!(vol.read("D>F>X"), Err(Error::NotADirectory(_))));
        assert!(matches!(vol.read("D"), Err(Error::IsDirectory(_))));
    }

    #[test]
    fn dates_and_flags() {
        let mut vol = sparta_blank();
        vol.write("F", b"x", Some(when(1))).unwrap();
        // Replacing with no date keeps the old one; a new date replaces it.
        vol.set_overwrite(true);
        vol.write("F", b"y", None).unwrap();
        assert_eq!(vol.entry("F").unwrap().modified, Some(when(1)));
        vol.write("F", b"z", Some(when(2))).unwrap();
        assert_eq!(vol.entry("F").unwrap().modified, Some(when(2)));
        vol.set_timestamp("F", Some(when(9))).unwrap();
        assert_eq!(vol.entry("F").unwrap().modified, Some(when(9)));
        vol.set_timestamp("F", None).unwrap();
        assert_eq!(vol.entry("F").unwrap().modified, None);
        vol.make_dir("D", None).unwrap();
        vol.set_timestamp("D", Some(when(5))).unwrap();
        assert_eq!(vol.entry("D").unwrap().modified, Some(when(5)));
        vol.set_hidden("F", true).unwrap();
        vol.set_archived("F", true).unwrap();
        let e = vol.entry("F").unwrap();
        assert!(e.hidden && e.archived && !e.locked);
        vol.set_hidden("F", false).unwrap();
        assert!(!vol.entry("F").unwrap().hidden);
        assert_eq!(vol.read("F").unwrap(), b"z");
        check_bitmap(&vol);
        let mut dos = blank();
        dos.write("F", b"x", None).unwrap();
        assert!(matches!(
            dos.set_timestamp("F", None),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            dos.set_hidden("F", true),
            Err(Error::Unsupported(_))
        ));
        assert!(Timestamp::new(2026, 2, 29, 0, 0, 0).is_err());
        assert!(Timestamp::new(2028, 2, 29, 0, 0, 0).is_ok());
    }

    #[test]
    fn moving_files_and_directories() {
        let mut vol = sparta_blank();
        vol.make_dir("A", None).unwrap();
        vol.make_dir("A>B", None).unwrap();
        vol.make_dir("C", None).unwrap();
        vol.write("A>B>F", b"data", Some(when(4))).unwrap();
        let free = vol.free_sectors();
        vol.move_to("A>B>F", "C").unwrap();
        assert!(!vol.exists("A>B>F"));
        assert_eq!(vol.read("C>F").unwrap(), b"data");
        assert_eq!(vol.entry("C>F").unwrap().modified, Some(when(4)));
        // A whole directory, to the top.
        vol.move_to("A>B", "").unwrap();
        assert!(vol.entry("B").unwrap().is_dir());
        vol.write("B>G", b"g", None).unwrap();
        assert_eq!(vol.read("B>G").unwrap(), b"g");
        // B's header now names the main directory as its parent: deleting
        // works from there.
        vol.delete("B>G").unwrap();
        vol.delete("B").unwrap();
        // Not into itself or below it; not onto a name already there.
        vol.make_dir("C>D", None).unwrap();
        assert!(matches!(
            vol.move_to("C", "C>D"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(vol.move_to("C", "C"), Err(Error::Unsupported(_))));
        vol.write("F", b"top", None).unwrap();
        assert!(matches!(
            vol.move_to("F", "C"),
            Err(Error::AlreadyExists(_))
        ));
        vol.lock("F").unwrap();
        assert!(matches!(vol.move_to("F", "A"), Err(Error::Locked(_))));
        check_bitmap(&vol);
        assert!(vol.free_sectors() < free);
        assert!(matches!(
            blank().move_to("X", ""),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn fits_says_what_write_would_do() {
        let vol = sparta_blank();
        let free = vol.free_bytes() as usize;
        assert!(vol.fits("BIG", free / 2).unwrap());
        assert!(!vol.fits("BIG", free * 2).unwrap());
        assert!(matches!(vol.fits("bad name", 10), Err(Error::BadName(_))));
        let before = vol.image().clone();
        vol.fits("BIG", 100).unwrap();
        assert_eq!(*vol.image(), before);
        let mut dos = blank();
        for i in 0..64 {
            dos.write(format!("F{i}"), b"x", None).unwrap();
        }
        assert!(!dos.fits("ONEMORE", 1).unwrap());
    }

    #[test]
    fn write_protect_flag() {
        let mut vol = blank();
        assert!(!vol.image().write_protected());
        vol.set_write_protected(true).unwrap();
        let bytes = vol.image().to_bytes();
        assert_eq!(bytes[15] & 1, 1);
        assert!(DiskImage::from_bytes(&bytes).unwrap().write_protected());
        vol.set_write_protected(false).unwrap();
        assert!(!vol.image().write_protected());
        let mut xfd = DiskImage::new(Container::Xfd, 128, 720).unwrap();
        assert!(matches!(
            xfd.set_write_protected(true),
            Err(Error::NotRepresentable(_))
        ));
    }

    // ---- MyDOS ----

    #[test]
    fn mydos_formats() {
        for (layout, free, count, size) in [
            (Layout::MyDosSingle, 708, 720, 128),
            (Layout::MyDosEnhanced, 1027, 1040, 128),
            (Layout::MyDosDouble, 708, 720, 256),
            (
                Layout::MyDosCustom {
                    sectors: 65535,
                    sector_size: 256,
                },
                65535 - 3 - 33 - 8,
                65535,
                256,
            ),
        ] {
            let mut vol = Volume::format(layout, Container::Atr).unwrap();
            assert_eq!(vol.file_system(), FileSystem::MyDos, "{layout:?}");
            assert_eq!(vol.free_sectors(), free, "{layout:?}");
            assert_eq!(vol.image().sector_count(), count);
            assert_eq!(vol.image().sector_size(), size);
            let data: Vec<u8> = (0..5000).map(|i| i as u8).collect();
            vol.write("A.DAT", &data, None).unwrap();
            assert_eq!(vol.read("A.DAT").unwrap(), data);
            let fs = Dos2::open(vol.image()).unwrap();
            // 16-bit links on disks past sector 1023.
            assert_eq!(fs.list()[0].big_links(), count > 1023, "{layout:?}");
        }
        assert!(
            Volume::format(
                Layout::MyDosCustom {
                    sectors: 4000,
                    sector_size: 512
                },
                Container::Atr
            )
            .is_err()
        );
    }

    #[test]
    fn mydos_directories() {
        let mut vol = Volume::format(Layout::MyDosSingle, Container::Atr).unwrap();
        vol.write("TOP", b"top", None).unwrap();
        vol.make_dir("GAMES", None).unwrap();
        vol.make_dir("GAMES>OLD", None).unwrap();
        // 8 sectors for each directory, 1 for each small file.
        assert_eq!(vol.free_sectors(), 708 - 1 - 16);
        let data: Vec<u8> = (0..400).map(|i| i as u8).collect();
        vol.write("GAMES>OLD>PAC", &data, None).unwrap();
        assert_eq!(vol.read("GAMES>OLD>PAC").unwrap(), data);
        let games = vol.list("GAMES").unwrap();
        assert_eq!(games[0].name, b"OLD");
        assert!(games[0].is_dir() && games[0].sectors == 8);
        assert!(matches!(vol.read("GAMES"), Err(Error::IsDirectory(_))));
        assert!(matches!(vol.list("TOP"), Err(Error::NotADirectory(_))));
        assert!(matches!(
            vol.delete("GAMES>OLD"),
            Err(Error::DirectoryNotEmpty(_))
        ));
        // Moving changes the file number in every sector (the file goes
        // from place 0 in OLD to place 2 at the top).
        vol.move_to("GAMES>OLD>PAC", "").unwrap();
        assert_eq!(vol.read("PAC").unwrap(), data);
        let fs = Dos2::open(vol.image()).unwrap();
        let pac = fs.find("PAC").unwrap();
        assert_eq!(pac.index, 2);
        assert!(!pac.damaged);
        assert_eq!(vol.validate().unwrap(), []);
        // A directory with what's in it.
        vol.write("GAMES>OLD>X", b"x", None).unwrap();
        vol.move_to("GAMES>OLD", "").unwrap();
        assert_eq!(vol.read("OLD>X").unwrap(), b"x");
        assert!(matches!(
            vol.move_to("GAMES", "GAMES"),
            Err(Error::Unsupported(_))
        ));
        vol.move_to("OLD", "GAMES").unwrap();
        assert!(matches!(
            vol.move_to("GAMES", "GAMES>OLD"),
            Err(Error::Unsupported(_))
        ));
        vol.rename("GAMES", "PLAY").unwrap();
        vol.lock("PLAY>OLD>X").unwrap();
        assert!(vol.list("PLAY>OLD").unwrap()[0].locked);
        assert!(matches!(vol.delete("PLAY>OLD>X"), Err(Error::Locked(_))));
        vol.unlock("PLAY>OLD>X").unwrap();
        vol.delete("PLAY>OLD>X").unwrap();
        vol.delete("PLAY>OLD").unwrap();
        vol.delete("PLAY").unwrap();
        vol.delete("PAC").unwrap();
        vol.delete("TOP").unwrap();
        assert_eq!(vol.free_sectors(), 708);
        assert!(vol.list("").unwrap().is_empty());
        assert_eq!(vol.validate().unwrap(), []);
        // MyDOS keeps no dates.
        assert!(matches!(
            vol.set_timestamp("X", None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn mydos_directory_holds_64() {
        let mut vol = Volume::format(Layout::MyDosDouble, Container::Atr).unwrap();
        vol.make_dir("D", None).unwrap();
        for i in 0..64 {
            vol.write(format!("D>F{i}"), b"x", None).unwrap();
        }
        assert!(matches!(
            vol.write("D>MORE", b"x", None),
            Err(Error::DirectoryFull)
        ));
        assert_eq!(vol.list("D").unwrap().len(), 64);
        assert_eq!(vol.read("D>F63").unwrap(), b"x");
    }

    // ---- Validate and the sector map ----

    #[test]
    fn sector_map_names_every_sector() {
        let mut vol = Volume::format(Layout::MyDosSingle, Container::Atr).unwrap();
        vol.make_dir("D", None).unwrap();
        vol.write("D>F", &[1; 300], None).unwrap();
        let map = vol.sector_map().unwrap();
        assert_eq!(map[1], SectorUse::Boot);
        assert_eq!(map[360], SectorUse::System);
        assert_eq!(map[361], SectorUse::Directory(Vec::new()));
        // The new directory takes the first 8 free sectors, then the file
        // its 3.
        assert_eq!(map[4], SectorUse::Directory(b"D".to_vec()));
        assert_eq!(map[12], SectorUse::File(b"D>F".to_vec()));
        assert_eq!(map[14], SectorUse::File(b"D>F".to_vec()));
        assert_eq!(map[15], SectorUse::Free);
        assert_eq!(map[720], SectorUse::Free);
        let dos = blank();
        assert_eq!(dos.sector_map().unwrap()[720], SectorUse::Reserved);
        let sparta = sparta_blank();
        let map = sparta.sector_map().unwrap();
        assert_eq!(
            (map[4].clone(), map[5].clone()),
            (SectorUse::System, SectorUse::Directory(Vec::new()))
        );
        assert_eq!(map[720], SectorUse::Reserved);
    }

    #[test]
    fn validate_finds_each_kind_of_problem() {
        // DOS 2: point a second file at the first one's sector, mark a
        // used sector free, leave one unowned, and break a chain.
        let mut vol = blank();
        vol.write("A", &[1; 100], None).unwrap();
        vol.write("B", &[2; 100], None).unwrap();
        vol.write("C", &[3; 300], None).unwrap();
        let mut image = vol.into_image();
        // B's entry (index 1, bytes 3-4) now starts at A's sector 4.
        let mut dir = image.sector(361).unwrap().to_vec();
        dir[16 + 3] = 4;
        image.write_sector(361, &dir).unwrap();
        // Sector 5 (B's own) is now used by nothing; sector 6 (C's
        // first) is marked free.
        let mut vtoc = image.sector(360).unwrap().to_vec();
        vtoc[10] |= 0x80 >> 6;
        image.write_sector(360, &vtoc).unwrap();
        let vol = Volume::open(image).unwrap();
        let problems = vol.validate().unwrap();
        let has = |p: &Problem| problems.contains(p);
        // DOS 2's file numbers show B's chain is A's.
        assert!(
            has(&Problem::Damaged {
                path: b"B".to_vec(),
                why: "B: sector 4 belongs to file number 0, not 1".into(),
            }),
            "{problems:?}"
        );
        assert!(has(&Problem::Unowned { sector: 5 }), "{problems:?}");
        assert!(
            has(&Problem::MarkedFree {
                sector: 6,
                owner: SectorUse::File(b"C".to_vec()),
            }),
            "{problems:?}"
        );
        assert!(
            has(&Problem::FreeCount {
                recorded: 702,
                counted: 703
            }),
            "{problems:?}"
        );
        for p in &problems {
            assert!(!p.to_string().is_empty());
        }

        // SpartaDOS: G's entry points at F's sector map, so both claim
        // its sectors, and G's own are lost.
        let mut vol = sparta_blank();
        vol.write("F", &[1; 10], None).unwrap();
        vol.write("G", &[2; 10], None).unwrap();
        let fs = Sparta::open(vol.image()).unwrap();
        let (f, g) = (fs.find("F").unwrap(), fs.find("G").unwrap());
        let mut dir = vol.read_sector(6).unwrap().to_vec();
        let at = g.index as usize * 23 + 1;
        dir[at..at + 2].copy_from_slice(&f.map.to_le_bytes());
        vol.write_sector(6, &dir).unwrap();
        let problems = vol.validate().unwrap();
        assert!(
            problems.contains(&Problem::Shared {
                sector: u32::from(f.map),
                first: SectorUse::File(b"F".to_vec()),
                second: SectorUse::File(b"G".to_vec()),
            }),
            "{problems:?}"
        );
        assert!(problems.contains(&Problem::Unowned {
            sector: u32::from(g.map)
        }));

        // SpartaDOS: a file whose map lists a sector off the disk.
        let mut vol = sparta_blank();
        vol.write("F", &[1; 10], None).unwrap();
        let fs = Sparta::open(vol.image()).unwrap();
        let map = u32::from(fs.find("F").unwrap().map);
        let mut sector = vol.read_sector(map).unwrap().to_vec();
        sector[4..6].copy_from_slice(&9000u16.to_le_bytes());
        vol.write_sector(map, &sector).unwrap();
        let problems = vol.validate().unwrap();
        assert!(
            matches!(&problems[..], [Problem::Damaged { path, .. }, ..] if path == b"F"),
            "{problems:?}"
        );
    }

    // ---- Raw volumes, named layouts, features ----

    #[test]
    fn raw_volumes_have_only_sectors() {
        let mut vol = Volume::format(
            Layout::Raw {
                sectors: 100,
                sector_size: 256,
            },
            Container::Atr,
        )
        .unwrap();
        assert_eq!(vol.file_system(), FileSystem::Raw);
        assert!(!vol.features().files);
        assert_eq!(vol.image().sector_count(), 100);
        for result in [
            vol.list("").map(|_| ()),
            vol.read("X").map(|_| ()),
            vol.write("X", b"x", None),
            vol.make_dir("D", None),
            vol.set_volume_name("V"),
            vol.validate().map(|_| ()),
            vol.make_bootable(&DiskImage::new(Container::Atr, 128, 720).unwrap()),
        ] {
            assert!(
                matches!(result, Err(Error::Unsupported(_) | Error::NotBootable(_))),
                "{result:?}"
            );
        }
        assert_eq!(vol.free_sectors(), 0);
        vol.write_sector(10, &[7; 256]).unwrap();
        assert_eq!(vol.read_sector(10).unwrap(), [7; 256]);
        // An unknown disk opens as raw; a known one can be forced raw.
        let blank = DiskImage::new(Container::Atr, 128, 720).unwrap();
        assert_eq!(Volume::detect(&blank), None);
        assert_eq!(Volume::open(blank).unwrap().file_system(), FileSystem::Raw);
        let dos = blank_image();
        assert_eq!(Volume::open_raw(dos.clone()).file_system(), FileSystem::Raw);
        // Writing a whole file system onto a raw disk makes it that.
        let mut vol = Volume::format(
            Layout::Raw {
                sectors: 720,
                sector_size: 128,
            },
            Container::Atr,
        )
        .unwrap();
        for n in 1..=720 {
            vol.write_sector(n, dos.sector(n).unwrap()).unwrap();
        }
        assert_eq!(vol.file_system(), FileSystem::Dos2);
        assert_eq!(vol.list("").unwrap().len(), 0);
    }

    fn blank_image() -> DiskImage {
        blank().into_image()
    }

    #[test]
    fn named_layouts() {
        for (layout, fs, count, size) in [
            (Layout::SpartaDoubleSided, FileSystem::SpartaDos, 1440, 256),
            (
                Layout::SpartaDoubleSided80,
                FileSystem::SpartaDos,
                2880,
                256,
            ),
            (Layout::MyDosDoubleSided, FileSystem::MyDos, 1440, 256),
            (Layout::MyDosDoubleSided80, FileSystem::MyDos, 2880, 256),
            (Layout::Dos25Enhanced, FileSystem::Dos25Enhanced, 1040, 128),
            (
                Layout::SpartaCustom {
                    sectors: 4000,
                    sector_size: 512,
                },
                FileSystem::SpartaDosX,
                4000,
                512,
            ),
        ] {
            assert_eq!(layout.file_system(), fs, "{layout:?}");
            assert_eq!(layout.geometry(), (count, size), "{layout:?}");
            let mut vol = Volume::format(layout, Container::Atr).unwrap();
            assert_eq!(vol.file_system(), fs, "{layout:?}");
            assert_eq!(vol.image().sector_count(), count);
            assert_eq!(vol.image().sector_size(), size);
            vol.write("A.DAT", &[5; 2000], None).unwrap();
            assert_eq!(vol.validate().unwrap(), []);
        }
        // SpartaDOS's sector 1 says double-sided: tracks byte $1E.
        let vol = Volume::format(Layout::SpartaDoubleSided, Container::Atr).unwrap();
        assert_eq!(vol.read_sector(1).unwrap()[0x1E], 0x80 | 40);
        let vol = Volume::format(Layout::SpartaDoubleSided80, Container::Atr).unwrap();
        assert_eq!(vol.read_sector(1).unwrap()[0x1E], 0x80 | 80);
    }

    #[test]
    fn features_say_what_works() {
        let dos = FileSystem::Dos2.features();
        assert!(dos.files && dos.locking && dos.bootable);
        assert!(!dos.directories && !dos.dates && !dos.volume_name);
        assert_eq!(dos.max_entries, Some(64));
        let mydos = FileSystem::MyDos.features();
        assert!(mydos.directories && !mydos.dates && mydos.bootable);
        let sparta = FileSystem::SpartaDos.features();
        assert!(sparta.directories && sparta.dates && sparta.hidden_and_archived);
        assert!(sparta.volume_name && sparta.bootable && sparta.underscore_in_names);
        assert_eq!(sparta.max_entries, None);
        assert!(!FileSystem::SpartaDosX.features().bootable);
        let raw = FileSystem::Raw.features();
        assert!(!raw.files && !raw.locking && !raw.bootable);
        // A 512-byte disk can't boot.
        let big = Volume::format(
            Layout::MyDosCustom {
                sectors: 1000,
                sector_size: 256,
            },
            Container::Atr,
        )
        .unwrap();
        assert!(big.features().bootable);
        // The features match what the calls do.
        let mut vol = blank();
        assert_eq!(vol.features(), FileSystem::Dos2.features());
        assert!(vol.make_dir("D", None).is_err());
        let mut sdx = Volume::format(
            Layout::SpartaCustom {
                sectors: 2000,
                sector_size: 512,
            },
            Container::Atr,
        )
        .unwrap();
        assert!(!sdx.features().bootable);
        assert!(matches!(
            sdx.make_bootable(blank().image()),
            Err(Error::NotBootable(_))
        ));
    }

    #[test]
    fn is_bootable_reads_the_boot_sector() {
        for layout in [
            Layout::Dos2Single,
            Layout::MyDosDouble,
            Layout::SpartaSingle,
            Layout::Raw {
                sectors: 720,
                sector_size: 128,
            },
        ] {
            let vol = Volume::format(layout, Container::Atr).unwrap();
            assert!(!vol.is_bootable(), "{layout:?}");
        }
        // A DOS 2 boot sector pointing at sector 4.
        let mut vol = blank();
        let mut s1 = vol.read_sector(1).unwrap().to_vec();
        s1[1] = 3;
        s1[0x0E] = 1;
        s1[0x0F] = 4;
        vol.write_sector(1, &s1).unwrap();
        assert!(vol.is_bootable());
        // Pointing off the disk, or with "no DOS", it isn't.
        s1[0x10] = 0xFF;
        vol.write_sector(1, &s1).unwrap();
        assert!(!vol.is_bootable());
        s1[0x10] = 0;
        s1[0x0E] = 0;
        vol.write_sector(1, &s1).unwrap();
        assert!(!vol.is_bootable());
        // SpartaDOS: the boot file's map.
        let mut vol = sparta_blank();
        let mut s1 = vol.read_sector(1).unwrap().to_vec();
        s1[0x28] = 5;
        vol.write_sector(1, &s1).unwrap();
        assert!(vol.is_bootable());
    }
}
