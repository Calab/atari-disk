//! Disk image containers: the sectors of a disk, as stored in an `.ATR`
//! or `.XFD` file. Nothing here knows about DOS. The layouts are
//! described in [`crate::formats`].

use crate::error::{Error, Result};

/// The ATR signature, `$0296`, as stored (low byte first).
const ATR_MAGIC: [u8; 2] = [0x96, 0x02];
const ATR_HEADER: usize = 16;
/// The boot sectors are always 128 bytes.
const BOOT_SECTORS: u32 = 3;
const BOOT_LEN: usize = 128;

/// The file format holding the sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Container {
    /// `.ATR`: a 16-byte header, then the sectors.
    Atr,
    /// `.XFD` (the XFormer format): only the sectors.
    Xfd,
}

/// A common name for the disk's layout, for showing to people.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Density {
    /// 720 sectors of 128 bytes (810 and 1050 drives).
    Single,
    /// 1040 sectors of 128 bytes (the 1050's "dual" or medium density).
    Enhanced,
    /// 720 sectors of 256 bytes.
    Double,
    /// Any other layout, e.g. a hard-disk partition.
    Other,
}

/// A disk's sectors, in memory. Changes are made here and written out
/// with [`DiskImage::to_bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskImage {
    container: Container,
    sector_size: usize,
    count: u32,
    /// How the boot sectors are stored (see the module notes).
    boot: Boot,
    /// ATR header bytes 7 to 15, kept as read.
    atr_extra: [u8; 9],
    /// Every sector, each in a slot `sector_size` long (the boot sectors
    /// use only their first 128 bytes).
    data: Vec<u8>,
}

impl DiskImage {
    /// A blank disk: every sector filled with zeros. `sector_size` is 128
    /// or 256.
    pub fn new(container: Container, sector_size: usize, count: u32) -> Result<Self> {
        check_sector_size(sector_size)?;
        if count == 0 {
            return Err(Error::Unsupported("a disk with no sectors".into()));
        }
        let image = Self {
            container,
            sector_size,
            count,
            boot: Boot::Short,
            atr_extra: [0; 9],
            data: vec![0; sector_size * count as usize],
        };
        if container == Container::Xfd {
            image.check_xfd()?;
        }
        Ok(image)
    }

    /// Read an image, telling ATR from XFD by the ATR signature.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() >= ATR_HEADER && bytes[..2] == ATR_MAGIC {
            Self::from_atr(bytes)
        } else {
            Self::from_xfd(bytes)
        }
    }

    /// Read an `.ATR` file.
    pub fn from_atr(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < ATR_HEADER || bytes[..2] != ATR_MAGIC {
            return Err(Error::NotAnImage("no ATR header".into()));
        }
        let word = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]) as usize;
        let paragraphs = word(2) | (bytes[6] as usize) << 16;
        let sector_size = word(4);
        check_sector_size(sector_size)?;
        // Some tools write a wrong size; trust the file when it's shorter
        // and ignore anything after the declared size.
        let body = &bytes[ATR_HEADER..];
        let len = (paragraphs * 16).min(body.len());
        let (count, boot) = layout(&body[..len], sector_size)?;
        let mut atr_extra = [0; 9];
        atr_extra.copy_from_slice(&bytes[7..ATR_HEADER]);
        Ok(Self::load(
            Container::Atr,
            sector_size,
            count,
            boot,
            atr_extra,
            &body[..len],
        ))
    }

    /// Read an `.XFD` file.
    pub fn from_xfd(bytes: &[u8]) -> Result<Self> {
        let len = bytes.len();
        let (sector_size, count, boot) = match len {
            // 720 double-density sectors, with and without padded boot
            // sectors.
            183_936 | 184_320 => {
                let (count, boot) = layout(bytes, 256)?;
                (256, count, boot)
            }
            0 => return Err(Error::NotAnImage("the file is empty".into())),
            _ if len.is_multiple_of(128) => (128, (len / 128) as u32, Boot::Short),
            _ => {
                return Err(Error::NotAnImage(format!(
                    "{len} bytes is not a whole number of 128-byte sectors"
                )));
            }
        };
        Ok(Self::load(
            Container::Xfd,
            sector_size,
            count,
            boot,
            [0; 9],
            bytes,
        ))
    }

    /// Spread `body` (the sector data as stored) into one slot per sector.
    fn load(
        container: Container,
        sector_size: usize,
        count: u32,
        boot: Boot,
        atr_extra: [u8; 9],
        body: &[u8],
    ) -> Self {
        let mut image = Self {
            container,
            sector_size,
            count,
            boot,
            atr_extra,
            data: vec![0; sector_size * count as usize],
        };
        for n in 1..=count {
            let (at, len) = (image.stored_at(n), image.stored_len(n));
            let end = (at + len).min(body.len());
            if at < end {
                let slot = image.slot(n);
                image.data[slot..slot + (end - at)].copy_from_slice(&body[at..end]);
            }
        }
        image
    }

    /// The image as file bytes, in its container's format.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body_len = self.stored_at(self.count) + self.stored_len(self.count);
        let mut out = Vec::with_capacity(ATR_HEADER + body_len);
        if self.container == Container::Atr {
            let paragraphs = body_len / 16;
            out.extend_from_slice(&ATR_MAGIC);
            out.extend_from_slice(&(paragraphs as u16).to_le_bytes());
            out.extend_from_slice(&(self.sector_size as u16).to_le_bytes());
            out.push((paragraphs >> 16) as u8);
            out.extend_from_slice(&self.atr_extra);
        }
        // Gaps (packed boot sectors' padding) stay zero.
        let start = out.len();
        out.resize(start + body_len, 0);
        for n in 1..=self.count {
            let (at, len, slot) = (start + self.stored_at(n), self.stored_len(n), self.slot(n));
            out[at..at + len].copy_from_slice(&self.data[slot..slot + len]);
        }
        out
    }

    /// ATR or XFD: the format [`DiskImage::to_bytes`] writes.
    pub fn container(&self) -> Container {
        self.container
    }

    /// Store the image as `container` from now on. XFD can only hold
    /// layouts its file size identifies.
    pub fn set_container(&mut self, container: Container) -> Result<()> {
        let old = self.container;
        self.container = container;
        if container == Container::Xfd
            && let Err(e) = self.check_xfd()
        {
            self.container = old;
            return Err(e);
        }
        Ok(())
    }

    /// The size of the disk's sectors: 128 or 256 (the boot sectors are
    /// 128 bytes either way; see [`DiskImage::sector_len`]).
    pub fn sector_size(&self) -> usize {
        self.sector_size
    }

    /// How many sectors the disk has.
    pub fn sector_count(&self) -> u32 {
        self.count
    }

    /// The common name for the disk's layout.
    pub fn density(&self) -> Density {
        match (self.sector_size, self.count) {
            (128, 720) => Density::Single,
            (128, 1040) => Density::Enhanced,
            (256, 720) => Density::Double,
            _ => Density::Other,
        }
    }

    /// The ATR header's write-protect flag. Emulators that honour it won't
    /// let the Atari write to the disk; this crate reports it and leaves
    /// the decision to the program using it.
    pub fn write_protected(&self) -> bool {
        self.atr_extra[15 - 7] & 1 != 0
    }

    /// Set or clear the ATR write-protect flag (bit 0 of header byte 15).
    /// XFD files have no header to keep it in, so they give
    /// [`Error::NotRepresentable`].
    pub fn set_write_protected(&mut self, on: bool) -> Result<()> {
        if self.container == Container::Xfd {
            return Err(Error::NotRepresentable(
                "an XFD file has no write-protect flag".into(),
            ));
        }
        if on {
            self.atr_extra[15 - 7] |= 1;
        } else {
            self.atr_extra[15 - 7] &= !1;
        }
        Ok(())
    }

    /// The number of data bytes in sector `n`: 128 for the boot sectors
    /// of a disk with 256-byte sectors, otherwise the sector size.
    pub fn sector_len(&self, n: u32) -> usize {
        if n <= BOOT_SECTORS && self.sector_size == 256 {
            BOOT_LEN
        } else {
            self.sector_size
        }
    }

    /// Sector `n`'s data.
    pub fn sector(&self, n: u32) -> Result<&[u8]> {
        self.check_sector(n)?;
        let slot = self.slot(n);
        Ok(&self.data[slot..slot + self.sector_len(n)])
    }

    /// Sector `n`'s data, to change in place.
    pub fn sector_mut(&mut self, n: u32) -> Result<&mut [u8]> {
        self.check_sector(n)?;
        let (slot, len) = (self.slot(n), self.sector_len(n));
        Ok(&mut self.data[slot..slot + len])
    }

    /// Replace sector `n`'s data; `bytes` must be exactly
    /// [`DiskImage::sector_len`] long.
    pub fn write_sector(&mut self, n: u32, bytes: &[u8]) -> Result<()> {
        let expected = self.sector_len(n);
        let sector = self.sector_mut(n)?;
        if bytes.len() != expected {
            return Err(Error::WrongSectorLength {
                sector: n,
                expected,
                got: bytes.len(),
            });
        }
        sector.copy_from_slice(bytes);
        Ok(())
    }

    fn check_sector(&self, n: u32) -> Result<()> {
        if n == 0 || n > self.count {
            return Err(Error::SectorOutOfRange {
                sector: n,
                count: self.count,
            });
        }
        Ok(())
    }

    /// Where sector `n`'s slot starts in `data`.
    fn slot(&self, n: u32) -> usize {
        (n as usize - 1) * self.sector_size
    }

    /// Where sector `n` starts in the file, after any header.
    fn stored_at(&self, n: u32) -> usize {
        let n = n as usize;
        let boot = BOOT_SECTORS as usize;
        match self.boot {
            _ if self.sector_size != 256 => (n - 1) * self.sector_size,
            Boot::Short if n <= boot => (n - 1) * BOOT_LEN,
            Boot::Short => boot * BOOT_LEN + (n - boot - 1) * self.sector_size,
            Boot::Packed if n <= boot => (n - 1) * BOOT_LEN,
            Boot::Packed | Boot::Slots => (n - 1) * self.sector_size,
        }
    }

    /// How many bytes of sector `n` are copied between the file and its
    /// slot. Boot sectors in slots keep the whole slot, so whatever is in
    /// the unused half survives a round trip.
    fn stored_len(&self, n: u32) -> usize {
        if self.boot == Boot::Slots {
            self.sector_size
        } else {
            self.sector_len(n)
        }
    }

    /// Can this image be saved as an XFD and read back the same?
    fn check_xfd(&self) -> Result<()> {
        let ok = match self.sector_size {
            128 => true,
            // Only the standard double-density disk has a recognisable size.
            256 => self.count == 720,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(Error::NotRepresentable(format!(
                "an XFD file can't hold {} sectors of {} bytes; use ATR",
                self.count, self.sector_size
            )))
        }
    }
}

fn check_sector_size(size: usize) -> Result<()> {
    match size {
        128 | 256 | 512 => Ok(()),
        _ => Err(Error::NotAnImage(format!("{size}-byte sectors"))),
    }
}

/// How the boot sectors are stored on a disk with 256-byte sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boot {
    /// Three 128-byte sectors, then sector 4 (the usual way).
    Short,
    /// Three 128-byte sectors, 384 bytes of zeros, then sector 4.
    Packed,
    /// Each in the first half of a full 256-byte sector.
    Slots,
}

/// The sector count of `body` (the sector data, without a header) and how
/// its boot sectors are stored.
fn layout(body: &[u8], sector_size: usize) -> Result<(u32, Boot)> {
    let len = body.len();
    let boot = BOOT_SECTORS as usize * BOOT_LEN;
    let (count, layout) = if sector_size != 256 {
        (len.div_ceil(sector_size), Boot::Short)
    } else if len < boot {
        (len.div_ceil(BOOT_LEN), Boot::Short)
    } else if len.is_multiple_of(sector_size) {
        // Three 128-byte boot sectors would leave half a sector over, so
        // a whole number of full sectors means padded boot sectors.
        let gap_empty = body
            .get(boot..2 * boot)
            .is_some_and(|g| g.iter().all(|&b| b == 0));
        let layout = if gap_empty { Boot::Packed } else { Boot::Slots };
        (len / sector_size, layout)
    } else {
        (
            BOOT_SECTORS as usize + (len - boot).div_ceil(sector_size),
            Boot::Short,
        )
    };
    if count == 0 {
        return Err(Error::NotAnImage("the image has no sectors".into()));
    }
    Ok((count as u32, layout))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct pattern for each sector, to spot misplaced data.
    fn fill(image: &mut DiskImage) {
        for n in 1..=image.sector_count() {
            let len = image.sector_len(n);
            let bytes: Vec<u8> = (0..len).map(|i| (n as usize * 7 + i) as u8).collect();
            image.write_sector(n, &bytes).unwrap();
        }
    }

    #[test]
    fn single_density_atr_round_trips() {
        let mut image = DiskImage::new(Container::Atr, 128, 720).unwrap();
        fill(&mut image);
        let bytes = image.to_bytes();
        assert_eq!(bytes.len(), 16 + 720 * 128);
        // $0296, 5760 paragraphs ($1680), 128-byte sectors.
        assert_eq!(bytes[..7], [0x96, 0x02, 0x80, 0x16, 0x80, 0x00, 0x00]);
        let back = DiskImage::from_bytes(&bytes).unwrap();
        assert_eq!(back, image);
        assert_eq!(back.density(), Density::Single);
    }

    #[test]
    fn double_density_boot_sectors_are_short() {
        let mut image = DiskImage::new(Container::Atr, 256, 720).unwrap();
        fill(&mut image);
        assert_eq!(image.sector(3).unwrap().len(), 128);
        assert_eq!(image.sector(4).unwrap().len(), 256);
        let bytes = image.to_bytes();
        assert_eq!(bytes.len(), 16 + 3 * 128 + 717 * 256);
        // Sector 4 follows straight after the three short boot sectors.
        assert_eq!(bytes[16 + 384], 4 * 7);
        assert_eq!(DiskImage::from_bytes(&bytes).unwrap(), image);
    }

    /// An ATR file with 256-byte sectors around `body`.
    fn dd_atr(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x96, 0x02];
        bytes.extend_from_slice(&((body.len() / 16) as u16).to_le_bytes());
        bytes.extend_from_slice(&256u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 10]);
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn packed_boot_sectors() {
        // Three 128-byte boot sectors, 384 bytes of zeros, then sector 4
        // (as in the MyDOS 4.5D sample).
        let mut body = vec![0u8; 720 * 256];
        body[0] = 0xB0;
        body[128] = 0xB1;
        body[256] = 0xB2;
        body[768] = 0x44;
        let bytes = dd_atr(&body);
        let image = DiskImage::from_bytes(&bytes).unwrap();
        assert_eq!(image.sector_count(), 720);
        let first = |n| image.sector(n).unwrap()[0];
        assert_eq!(
            [first(1), first(2), first(3), first(4)],
            [0xB0, 0xB1, 0xB2, 0x44]
        );
        assert_eq!(image.to_bytes(), bytes);
    }

    #[test]
    fn boot_sectors_in_slots() {
        // Each boot sector in the first half of 256 bytes; the second
        // halves hold leftovers, which are kept.
        let mut body = vec![0u8; 720 * 256];
        body[0] = 0xB0;
        body[200] = 0xEE;
        body[256] = 0xB1;
        body[512] = 0xB2;
        body[768] = 0x44;
        let bytes = dd_atr(&body);
        let image = DiskImage::from_bytes(&bytes).unwrap();
        assert_eq!(image.sector(1).unwrap().len(), 128);
        let first = |n| image.sector(n).unwrap()[0];
        assert_eq!(
            [first(1), first(2), first(3), first(4)],
            [0xB0, 0xB1, 0xB2, 0x44]
        );
        assert_eq!(image.to_bytes(), bytes);
    }

    #[test]
    fn header_extras_and_write_protect_are_kept() {
        let mut bytes = DiskImage::new(Container::Atr, 128, 720).unwrap().to_bytes();
        bytes[15] = 1;
        bytes[8] = 0x5A;
        let image = DiskImage::from_bytes(&bytes).unwrap();
        assert!(image.write_protected());
        assert_eq!(image.to_bytes(), bytes);
    }

    #[test]
    fn short_atr_files_are_padded_with_zeros() {
        // The header says 720 sectors but the file stops partway through
        // sector 2.
        let mut bytes = DiskImage::new(Container::Atr, 128, 720).unwrap().to_bytes();
        bytes.truncate(16 + 128 + 10);
        let image = DiskImage::from_bytes(&bytes).unwrap();
        assert_eq!(image.sector_count(), 2);
        assert_eq!(image.sector(2).unwrap().len(), 128);
    }

    #[test]
    fn xfd_sizes() {
        let sd = DiskImage::from_bytes(&vec![0; 92_160]).unwrap();
        assert_eq!(
            (sd.container(), sd.density()),
            (Container::Xfd, Density::Single)
        );
        let ed = DiskImage::from_bytes(&vec![0; 133_120]).unwrap();
        assert_eq!(ed.density(), Density::Enhanced);
        let dd = DiskImage::from_bytes(&vec![0; 183_936]).unwrap();
        assert_eq!(dd.density(), Density::Double);
        let padded = DiskImage::from_bytes(&vec![0; 184_320]).unwrap();
        assert_eq!(padded.density(), Density::Double);
        assert_eq!(padded.to_bytes().len(), 184_320);
        assert!(DiskImage::from_bytes(&[0; 100]).is_err());
        assert!(DiskImage::from_bytes(&[]).is_err());
    }

    #[test]
    fn xfd_refuses_layouts_it_cant_identify() {
        let mut image = DiskImage::new(Container::Atr, 256, 1000).unwrap();
        assert!(matches!(
            image.set_container(Container::Xfd),
            Err(Error::NotRepresentable(_))
        ));
        assert_eq!(image.container(), Container::Atr);
        assert!(DiskImage::new(Container::Xfd, 128, 1040).is_ok());
    }

    #[test]
    fn sector_numbers_and_lengths_are_checked() {
        let mut image = DiskImage::new(Container::Atr, 128, 720).unwrap();
        assert!(matches!(
            image.sector(0),
            Err(Error::SectorOutOfRange { .. })
        ));
        assert!(image.sector(721).is_err());
        assert!(matches!(
            image.write_sector(5, &[0; 127]),
            Err(Error::WrongSectorLength { .. })
        ));
    }

    #[test]
    fn sector_sizes_512_and_others() {
        // 512-byte sectors: every sector full length, boot sectors too.
        let mut image = DiskImage::new(Container::Atr, 512, 8).unwrap();
        fill(&mut image);
        assert_eq!(image.sector(1).unwrap().len(), 512);
        let bytes = image.to_bytes();
        assert_eq!(bytes.len(), 16 + 8 * 512);
        assert_eq!(bytes[16 + 512], 2 * 7);
        assert_eq!(DiskImage::from_bytes(&bytes).unwrap(), image);
        assert!(matches!(
            image.set_container(Container::Xfd),
            Err(Error::NotRepresentable(_))
        ));
        // Anything else isn't an Atari sector size.
        let mut bytes = vec![0x96, 0x02, 0x00, 0x01, 0x00, 0x04];
        bytes.extend_from_slice(&[0; 10 + 4096]);
        assert!(matches!(
            DiskImage::from_bytes(&bytes),
            Err(Error::NotAnImage(_))
        ));
    }
}
