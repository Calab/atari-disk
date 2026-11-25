//! What every sector of a disk is used for, and the problems found on the
//! way: the work behind [`crate::Volume::sector_map`] and
//! [`crate::Volume::validate`]. Nothing here changes a disk.

use std::fmt;

use crate::atascii::atascii_to_ascii;
use crate::dos2::Dos2;
use crate::error::{Error, Result};
use crate::image::DiskImage;
use crate::sparta::Sparta;

/// What a sector holds, from [`crate::Volume::sector_map`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectorUse {
    /// Marked free, and nothing uses it.
    Free,
    /// Sectors 1–3, which the Atari loads when it boots.
    Boot,
    /// The DOS's own tables: the VTOC (DOS 2, MyDOS) or bitmap
    /// (SpartaDOS).
    System,
    /// Kept out of use by the DOS (e.g. sector 720 under DOS 2), or past
    /// what the file system covers.
    Reserved,
    /// Part of the directory at this path (ATASCII; empty for the main
    /// one): its entries, or on SpartaDOS its sector maps too.
    Directory(Vec<u8>),
    /// Part of the file at this path (ATASCII): its data, or on SpartaDOS
    /// its sector maps too.
    File(Vec<u8>),
    /// Marked in use, but nothing uses it (a "lost" sector).
    Unowned,
}

/// A problem [`crate::Volume::validate`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Problem {
    /// `owner` uses the sector, but the bitmap says it's free, so DOS
    /// could give it to a new file and overwrite `owner`.
    MarkedFree {
        /// The sector.
        sector: u32,
        /// What uses it.
        owner: SectorUse,
    },
    /// Two files or directories claim the same sector.
    Shared {
        /// The sector.
        sector: u32,
        /// The first to claim it.
        first: SectorUse,
        /// The second.
        second: SectorUse,
    },
    /// Marked in use, but nothing uses it: space lost until the bitmap
    /// is rebuilt.
    Unowned {
        /// The sector.
        sector: u32,
    },
    /// The free-sector count the DOS keeps doesn't match its bitmap.
    FreeCount {
        /// What the VTOC or sector 1 says.
        recorded: u32,
        /// The free bits in the bitmap.
        counted: u32,
    },
    /// A file or directory (ATASCII path) can't be followed to its end.
    Damaged {
        /// Its path (ATASCII; empty for the main directory).
        path: Vec<u8>,
        /// What's wrong, in words.
        why: String,
    },
}

impl fmt::Display for SectorUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SectorUse::Free => write!(f, "free"),
            SectorUse::Boot => write!(f, "boot sector"),
            SectorUse::System => write!(f, "DOS's own table"),
            SectorUse::Reserved => write!(f, "reserved"),
            SectorUse::Directory(p) if p.is_empty() => write!(f, "the main directory"),
            SectorUse::Directory(p) => write!(f, "directory {}", atascii_to_ascii(p)),
            SectorUse::File(p) => write!(f, "file {}", atascii_to_ascii(p)),
            SectorUse::Unowned => write!(f, "in use by nothing"),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::MarkedFree { sector, owner } => {
                write!(f, "sector {sector} is marked free but belongs to {owner}")
            }
            Problem::Shared {
                sector,
                first,
                second,
            } => write!(f, "sector {sector} belongs to both {first} and {second}"),
            Problem::Unowned { sector } => {
                write!(f, "sector {sector} is marked in use but nothing uses it")
            }
            Problem::FreeCount { recorded, counted } => write!(
                f,
                "the disk says {recorded} sectors are free, but its bitmap marks {counted}"
            ),
            Problem::Damaged { path, why } => write!(
                f,
                "{} is damaged: {why}",
                if path.is_empty() {
                    "the main directory".into()
                } else {
                    atascii_to_ascii(path)
                }
            ),
        }
    }
}

/// The map being built: who claims each sector, and what's wrong.
struct Map {
    uses: Vec<Option<SectorUse>>,
    problems: Vec<Problem>,
}

impl Map {
    fn new(count: u32) -> Self {
        Self {
            uses: vec![None; count as usize + 1],
            problems: Vec::new(),
        }
    }

    fn claim(&mut self, n: u32, owner: SectorUse) {
        let Some(slot) = self.uses.get_mut(n as usize) else {
            return;
        };
        match slot {
            Some(first) => self.problems.push(Problem::Shared {
                sector: n,
                first: first.clone(),
                second: owner,
            }),
            None => *slot = Some(owner),
        }
    }

    fn damaged(&mut self, path: &[u8], e: Error) {
        let why = match e {
            Error::Damaged(why) => why,
            other => other.to_string(),
        };
        self.problems.push(Problem::Damaged {
            path: path.to_vec(),
            why,
        });
    }

    /// Fill in the sectors nobody claimed, and check the claimed ones
    /// against the bitmap.
    fn finish(
        mut self,
        is_free: impl Fn(u32) -> bool,
        reserved: impl Fn(u32) -> bool,
    ) -> (Vec<SectorUse>, Vec<Problem>) {
        let mut out = vec![SectorUse::Reserved];
        for n in 1..self.uses.len() as u32 {
            let free = is_free(n);
            let used = match self.uses[n as usize].take() {
                Some(owner) => {
                    if free {
                        self.problems.push(Problem::MarkedFree {
                            sector: n,
                            owner: owner.clone(),
                        });
                    }
                    owner
                }
                None if free => SectorUse::Free,
                None if reserved(n) => SectorUse::Reserved,
                None => {
                    self.problems.push(Problem::Unowned { sector: n });
                    SectorUse::Unowned
                }
            };
            out.push(used);
        }
        (out, self.problems)
    }
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    if dir.is_empty() {
        name.to_vec()
    } else {
        [dir, b">", name].concat()
    }
}

/// The sector map and problems of a DOS 2 or MyDOS disk.
pub(crate) fn dos2(image: &DiskImage) -> Result<(Vec<SectorUse>, Vec<Problem>)> {
    let fs = Dos2::open(image)?;
    let mut map = Map::new(image.sector_count());
    for n in 1..=3 {
        map.claim(n, SectorUse::Boot);
    }
    for n in fs.vtoc_list() {
        map.claim(n, SectorUse::System);
    }
    let mut dirs = vec![(Vec::new(), fs.root())];
    let mut seen = Vec::new();
    while let Some((path, first)) = dirs.pop() {
        if seen.contains(&first) {
            map.damaged(&path, Error::Damaged("it's reached twice".into()));
            continue;
        }
        seen.push(first);
        let entries = match fs.list_dir(first) {
            Ok(entries) => entries,
            Err(e) => {
                map.damaged(&path, e);
                continue;
            }
        };
        for n in first..first + 8 {
            map.claim(n, SectorUse::Directory(path.clone()));
        }
        for entry in entries {
            let inner = join(&path, &entry.stored_name());
            if entry.is_dir() {
                dirs.push((inner, u32::from(entry.start)));
                continue;
            }
            let (sectors, result) = fs.chain(&entry);
            for n in sectors {
                map.claim(n, SectorUse::File(inner.clone()));
            }
            if let Err(e) = result {
                map.damaged(&inner, e);
            }
        }
    }
    let last = fs.last_marked();
    let counted = (0..=last).filter(|&n| fs.is_free(n)).count() as u32;
    let recorded = fs.free_sectors();
    if counted != recorded {
        map.problems.push(Problem::FreeCount { recorded, counted });
    }
    Ok(map.finish(|n| n <= last && fs.is_free(n), |n| fs.reserved(n)))
}

/// The sector map and problems of a SpartaDOS disk.
pub(crate) fn sparta(image: &DiskImage) -> Result<(Vec<SectorUse>, Vec<Problem>)> {
    let fs = Sparta::open(image)?;
    let total = fs.total_sectors();
    let mut map = Map::new(image.sector_count());
    for n in 1..=3 {
        map.claim(n, SectorUse::Boot);
    }
    for n in fs.bitmap_list() {
        map.claim(n, SectorUse::System);
    }
    let mut dirs = vec![(Vec::new(), fs.root())];
    let mut seen = Vec::new();
    while let Some((path, dir)) = dirs.pop() {
        if seen.contains(&dir) {
            map.damaged(&path, Error::Damaged("it's reached twice".into()));
            continue;
        }
        seen.push(dir);
        let listed = fs.dir_len(dir).and_then(|len| {
            let sectors = fs.data_sectors(dir, len, "the directory")?;
            Ok((sectors, fs.list_dir(dir)?))
        });
        let ((maps, data), entries) = match listed {
            Ok(listed) => listed,
            Err(e) => {
                map.damaged(&path, e);
                continue;
            }
        };
        for n in maps.into_iter().chain(data) {
            map.claim(n, SectorUse::Directory(path.clone()));
        }
        for entry in entries {
            let inner = join(&path, &entry.stored_name());
            if entry.is_dir() {
                dirs.push((inner, entry.map));
                continue;
            }
            match fs.data_sectors(entry.map, entry.size, &entry.file_name()) {
                Ok((maps, data)) => {
                    for n in maps.into_iter().chain(data) {
                        map.claim(n, SectorUse::File(inner.clone()));
                    }
                }
                Err(e) => map.damaged(&inner, e),
            }
        }
    }
    let counted = (0..=total).filter(|&n| fs.is_free(n)).count() as u32;
    let recorded = fs.free_sectors();
    if counted != recorded {
        map.problems.push(Problem::FreeCount { recorded, counted });
    }
    // SpartaDOS 3.2's XINIT keeps the last sector out of use; sectors
    // past the volume's count aren't its to use.
    let reserved = |n: u32| n >= total;
    Ok(map.finish(|n| fs.is_free(n), reserved))
}
