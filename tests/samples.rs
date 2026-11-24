//! Real disk images from `samples/<DOS>/` (not in git; each missing folder
//! is skipped). Every image must read and write back byte for byte.

use std::path::Path;

use atari_disk::{Container, DiskImage, Dos2, EntryKind, Error, Layout, Sparta, Timestamp, Volume};

fn images() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in ["DOS 2", "SpartaDOS", "MyDOS"] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("skipping {} (not found)", dir.display());
            continue;
        };
        out.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("atr") || e.eq_ignore_ascii_case("xfd"))
        }));
    }
    out.sort();
    out
}

#[test]
fn every_sample_round_trips() {
    for path in images() {
        let bytes = std::fs::read(&path).unwrap();
        let image =
            DiskImage::from_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        println!(
            "{}: {:?} {:?}, {} sectors of {} bytes{}",
            path.file_name().unwrap().to_string_lossy(),
            image.container(),
            image.density(),
            image.sector_count(),
            image.sector_size(),
            if image.write_protected() {
                ", write-protected"
            } else {
                ""
            },
        );
        assert!(image.to_bytes() == bytes, "{} changed", path.display());
    }
}

/// Every file on the DOS 2 (and DOS 2-style MyDOS) samples reads back
/// whole, and the free-sector counts agree with the bitmap.
#[test]
fn dos2_samples_read() {
    for path in images() {
        let folder = path.parent().unwrap().file_name().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let image = DiskImage::from_bytes(&bytes).unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        if folder == "SpartaDOS" {
            assert!(Dos2::open(&image).is_err(), "{name} read as DOS 2");
            continue;
        }
        let fs = Dos2::open(&image).unwrap_or_else(|e| panic!("{name}: {e}"));
        println!("{name}: {} free", fs.free_sectors());
        for entry in fs.list() {
            let data = fs.read(&entry).unwrap_or_else(|e| panic!("{name}: {e}"));
            let per_sector = image.sector_size() - 3;
            println!(
                "  {:12} {:4} sectors {:7} bytes{}",
                entry.file_name(),
                entry.sectors,
                data.len(),
                if entry.locked() { " locked" } else { "" }
            );
            assert_eq!(data.len() as u32, entry.size);
            assert!(data.len() <= usize::from(entry.sectors) * per_sector);
        }
        if fs.enhanced() {
            let below_720 = (0..720).filter(|&n| fs.is_free(n)).count() as u32;
            assert_eq!(below_720, fs.vtoc_free_sectors(), "{name}");
            // The second VTOC repeats the bitmap for sectors 48-719.
            let vtoc2 = image.sector(1024).unwrap();
            assert_eq!(image.sector(360).unwrap()[16..100], vtoc2[..84]);
            // Where nothing is written above 719, that part of the bitmap
            // is as DOS 2.5 formats it: compare with ours.
            let blank = Volume::format(Layout::Dos25Enhanced, Container::Atr).unwrap();
            if fs.free_sectors() - fs.vtoc_free_sectors() == 303 {
                assert_eq!(blank.image().sector(1024).unwrap()[84..], vtoc2[84..]);
            }
        }
        let all = (0..=image.sector_count().min(1023))
            .filter(|&n| fs.is_free(n))
            .count() as u32;
        assert_eq!(all, fs.free_sectors(), "{name}");
    }
}

/// Writing a file to each DOS 2 sample and deleting it again leaves the
/// free count as it was, and doesn't disturb the files already there.
#[test]
fn dos2_samples_write_and_delete() {
    for path in images() {
        let image = DiskImage::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        if !Volume::detect(&image).is_some_and(|fs| !fs.has_directories()) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        let mut vol = Volume::open(image).unwrap();
        let before: Vec<_> = vol.list("").unwrap();
        let contents: Vec<_> = before.iter().map(|e| vol.read(&e.name).unwrap()).collect();
        let free = vol.free_sectors();
        let data: Vec<u8> = (0..vol.free_bytes().min(20_000)).map(|i| i as u8).collect();
        vol.write("TEST.DAT", &data, None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(vol.read("TEST.DAT").unwrap(), data, "{name}");
        vol.delete("TEST.DAT").unwrap();
        assert_eq!(vol.free_sectors(), free, "{name}");
        assert_eq!(vol.list("").unwrap(), before, "{name}");
        for (entry, old) in before.iter().zip(&contents) {
            assert_eq!(&vol.read(&entry.name).unwrap(), old, "{name}");
        }
    }
}

/// Every SpartaDOS sample is found as SpartaDOS, every file in every
/// directory reads back at its length, and the bitmap agrees with the
/// free count and with the sectors the files and directories use.
#[test]
fn sparta_samples_read() {
    for path in images() {
        let folder = path.parent().unwrap().file_name().unwrap();
        let image = DiskImage::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if folder != "SpartaDOS" {
            assert!(Sparta::open(&image).is_err(), "{name} read as SpartaDOS");
            continue;
        }
        if image.sector(1).unwrap().iter().all(|&b| b == 0) {
            println!("{name}: skipped, never formatted (sector 1 is all zeros)");
            continue;
        }
        let fs = Sparta::open(&image).unwrap_or_else(|e| panic!("{name}: {e}"));
        let total = fs.total_sectors();
        let marked = (0..=total).filter(|&n| fs.is_free(n)).count() as u32;
        assert_eq!(marked, fs.free_sectors(), "{name}");
        let used = fs
            .sectors_in_use()
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let wrong: Vec<u32> = (1..=total)
            .filter(|&n| used[n as usize] && fs.is_free(n))
            .collect();
        println!(
            "{name}: {:?} \"{}\", {} free, used but marked free: {wrong:?}",
            Volume::detect(&image).unwrap(),
            fs.volume_name(),
            fs.free_sectors(),
        );
        // This one's main directory grew to 2 sectors, but whatever made
        // the image left the second marked free.
        let expected: &[u32] = if name.starts_with("Turbo-Basic XL 3.2q") {
            &[7]
        } else {
            &[]
        };
        assert_eq!(wrong, expected, "{name}");

        let mut vol = Volume::open(image.clone()).unwrap();
        vol.set_auto_uppercase(true);
        let mut dirs = vec![String::new()];
        let mut files = 0;
        while let Some(dir) = dirs.pop() {
            for entry in vol.list(&dir).unwrap() {
                let path = format!("{dir}{}", entry.name);
                assert!(!entry.damaged, "{name}: {path}");
                if entry.kind == EntryKind::Directory {
                    dirs.push(path + ">");
                    continue;
                }
                let data = vol.read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(data.len() as u32, entry.size, "{name}: {path}");
                files += 1;
            }
        }
        println!("  {files} files read");

        // Write into a new directory, and take it all away again.
        let free = vol.free_sectors();
        let before = vol.list("").unwrap();
        let data: Vec<u8> = (0..vol.free_bytes().min(40_000) - 2000)
            .map(|i| i as u8)
            .collect();
        let when = Timestamp {
            year: 2026,
            month: 11,
            day: 24,
            hour: 15,
            minute: 30,
            second: 0,
        };
        vol.make_dir("NEWDIR", Some(when))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        vol.write("NEWDIR>TEST.DAT", &data, Some(when))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(vol.read("NEWDIR>TEST.DAT").unwrap(), data, "{name}");
        let listed = &vol.list("NEWDIR").unwrap()[0];
        assert_eq!(
            (listed.size, listed.modified),
            (data.len() as u32, Some(when))
        );
        let fs = Sparta::open(vol.image()).unwrap();
        let used = fs.sectors_in_use().unwrap();
        let marked = (0..=total).filter(|&n| fs.is_free(n)).count() as u32;
        assert_eq!(marked, fs.free_sectors(), "{name}");
        let wrong: Vec<u32> = (1..=total)
            .filter(|&n| used[n as usize] && fs.is_free(n))
            .collect();
        assert_eq!(wrong, expected, "{name}");
        assert!(matches!(
            vol.delete("NEWDIR"),
            Err(Error::DirectoryNotEmpty(_))
        ));
        vol.delete("NEWDIR>TEST.DAT").unwrap();
        vol.delete("NEWDIR").unwrap();
        assert_eq!(vol.free_sectors(), free, "{name}");
        assert_eq!(vol.list("").unwrap(), before, "{name}");
    }
}

/// Disks for Steve to check in AltirraSDL, written to `tmp/altirra/`.
/// Run with: cargo test --test samples -- --ignored make_test_disks
#[test]
#[ignore]
fn make_test_disks() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/altirra");
    std::fs::create_dir_all(&out).unwrap();
    let save = |vol: &Volume, file: &str| {
        std::fs::write(out.join(file), vol.image().to_bytes()).unwrap();
        println!("{file}: {} free", vol.free_sectors());
    };
    // A BASIC program to ENTER, and a text file; lines end in ATASCII EOL.
    let lines =
        |text: &str| -> Vec<u8> { text.lines().flat_map(|l| l.bytes().chain([0x9B])).collect() };
    let program = lines("10 PRINT \"HELLO FROM ATARI-DISK\"\n20 PRINT \"IT WORKS\"");
    let text = lines("LINE ONE\nLINE TWO\nLINE THREE");
    // Each byte is its position, so a copy can be checked against it.
    let pattern = |len: usize| -> Vec<u8> { (0..len).map(|i| i as u8).collect() };

    for (layout, file) in [
        (Layout::Dos2Single, "blank-sd.atr"),
        (Layout::Dos2Double, "blank-dd.atr"),
        (Layout::Dos25Enhanced, "blank-ed.atr"),
    ] {
        save(&Volume::format(layout, Container::Atr).unwrap(), file);
    }
    // For comparison: the same, with the boot sector all zeros.
    let mut image = Volume::format(Layout::Dos2Single, Container::Atr)
        .unwrap()
        .into_image();
    image.write_sector(1, &[0; 128]).unwrap();
    save(&Volume::open(image).unwrap(), "blank-sd-zero-boot.atr");

    let mut vol = Volume::format(Layout::Dos2Single, Container::Atr).unwrap();
    vol.write("HELLO.LST", &program, None).unwrap();
    vol.write("TEXT.TXT", &text, None).unwrap();
    vol.write("GONE.DAT", &pattern(5000), None).unwrap();
    vol.write("BIG.DAT", &pattern(30_000), None).unwrap();
    vol.write("OLDNAME.DAT", &pattern(100), None).unwrap();
    vol.rename("OLDNAME.DAT", "NEWNAME.DAT").unwrap();
    vol.lock("TEXT.TXT").unwrap();
    vol.delete("GONE.DAT").unwrap();
    // Goes into GONE.DAT's place in the directory and its sectors.
    vol.write("EMPTY.DAT", b"", None).unwrap();
    save(&vol, "files-sd.atr");

    let mut vol = Volume::format(Layout::Dos2Double, Container::Atr).unwrap();
    vol.write("HELLO.LST", &program, None).unwrap();
    vol.write("BIG.DAT", &pattern(60_000), None).unwrap();
    save(&vol, "files-dd.atr");

    // On the DOS 2.5 enhanced sample: a file that runs on from the
    // sectors below 720 into those only the second VTOC knows.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("samples/DOS 2/DOS 2.5 (1984)(Atari)(US)[!].atr");
    if let Ok(bytes) = std::fs::read(path) {
        let mut vol = Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap();
        vol.write("HELLO.LST", &program, None).unwrap();
        vol.write("BIG.DAT", &pattern(500 * 125), None).unwrap();
        save(&vol, "dos25-ed-files.atr");
    }

    // SpartaDOS.
    let when = |day: u8, hour: u8| {
        Some(Timestamp {
            year: 2026,
            month: 11,
            day,
            hour,
            minute: 30,
            second: 0,
        })
    };
    for (layout, file, label) in [
        (Layout::SpartaSingle, "sparta-blank-sd.atr", "BLANK SD"),
        (Layout::SpartaEnhanced, "sparta-blank-ed.atr", "BLANK ED"),
        (Layout::SpartaDouble, "sparta-blank-dd.atr", "BLANK DD"),
    ] {
        let mut vol = Volume::format(layout, Container::Atr).unwrap();
        vol.set_volume_name(label).unwrap();
        save(&vol, file);
    }
    let fill = |vol: &mut Volume| {
        vol.write("HELLO.LST", &program, when(24, 10)).unwrap();
        vol.write("TEXT.TXT", &text, when(24, 11)).unwrap();
        vol.lock("TEXT.TXT").unwrap();
        vol.write("GONE.DAT", &pattern(5000), None).unwrap();
        vol.write("BIG.DAT", &pattern(30_000), when(24, 12))
            .unwrap();
        vol.make_dir("GAMES", when(25, 9)).unwrap();
        vol.make_dir("GAMES>OLD", when(25, 10)).unwrap();
        vol.write("GAMES>OLD>OLDNAME.DAT", &pattern(100), when(25, 11))
            .unwrap();
        vol.rename("GAMES>OLD>OLDNAME.DAT", "NEW_NAME.DAT").unwrap();
        vol.delete("GONE.DAT").unwrap();
        vol.write("EMPTY.DAT", b"", when(26, 8)).unwrap();
    };
    for (layout, file) in [
        (Layout::SpartaSingle, "sparta-files-sd.atr"),
        (Layout::SpartaDouble, "sparta-files-dd.atr"),
        (
            Layout::SpartaCustom {
                sectors: 4000,
                sector_size: 512,
            },
            "sparta-files-512.atr",
        ),
    ] {
        let mut vol = Volume::format(layout, Container::Atr).unwrap();
        vol.set_volume_name("ATARIDSK").unwrap();
        fill(&mut vol);
        save(&vol, file);
    }
    // A directory with 100 files: it needs several sectors.
    let mut vol = Volume::format(Layout::SpartaSingle, Container::Atr).unwrap();
    vol.set_volume_name("MANY").unwrap();
    for i in 0..100 {
        vol.write(
            &format!("FILE{i:03}.TXT"),
            &lines(&format!("THIS IS FILE {i}")),
            None,
        )
        .unwrap();
    }
    save(&vol, "sparta-many-files.atr");
    // Steve's samples, with files added.
    let sample = |file: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples/SpartaDOS")
            .join(file);
        let bytes = std::fs::read(path).ok()?;
        Some(Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap())
    };
    if let Some(mut vol) = sample("SpartaDOS HDD.atr") {
        vol.write("HELLO.LST", &program, when(24, 10)).unwrap();
        vol.make_dir("ATARIDSK", when(24, 11)).unwrap();
        vol.write("ATARIDSK>BIG.DAT", &pattern(200_000), when(24, 12))
            .unwrap();
        save(&vol, "sparta-hdd-files.atr");
    }
    if let Some(mut vol) = sample("Turbo-Basic XL 3.2q-SpartaDOS 3.2g.atr") {
        // Sector 7 is in use, though the bitmap says free: it must stay
        // the main directory's.
        vol.write("HELLO.LST", &program, when(24, 10)).unwrap();
        vol.write("BIG.DAT", &pattern(4000), when(24, 11)).unwrap();
        save(&vol, "sparta-tb-files.atr");
    }
}

/// Our blank disks against ones DOS formatted itself (made in AltirraSDL;
/// skipped if missing). The enhanced one has no files, so every sector
/// but the boot sectors must match. The others have DOS.SYS and DUP.SYS:
/// with those deleted, the VTOC must match ours.
#[test]
fn format_matches_dos() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/DOS 2");
    for (file, layout, files) in [
        ("blank DOS2 SD.atr", Layout::Dos2Single, true),
        ("blank DOS2 DD.atr", Layout::Dos2Double, true),
        ("blank DOS2.5 ED.atr", Layout::Dos25Enhanced, false),
    ] {
        let Ok(bytes) = std::fs::read(dir.join(file)) else {
            eprintln!("skipping {file} (not found)");
            continue;
        };
        let mut dos = Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap();
        let ours = Volume::format(layout, Container::Atr).unwrap();
        let (a, b) = (dos.image(), ours.image());
        assert_eq!(a.sector_count(), b.sector_count(), "{file}");
        assert_eq!(a.sector_size(), b.sector_size(), "{file}");
        if files {
            dos.delete("DOS.SYS").unwrap();
            dos.delete("DUP.SYS").unwrap();
            assert_eq!(dos.image().sector(360), ours.image().sector(360), "{file}");
        } else {
            for n in 4..=a.sector_count() {
                assert_eq!(a.sector(n), b.sector(n), "{file}: sector {n}");
            }
        }
    }
}

/// Our SpartaDOS blanks against ones SpartaDOS 3.2g's XINIT formatted
/// in AltirraSDL (skipped if missing). On SD and ED XINIT also wrote
/// X32G.DOS: with the files deleted, the bitmap, free count, main
/// directory's map and sector 1's layout must match ours. The DD one is
/// untouched, so the data search start (`$12`) must match too.
#[test]
fn format_matches_xinit() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/SpartaDOS");
    for (file, layout) in [
        ("blank 32.g SD.atr", Layout::SpartaSingle),
        ("blank 32.g ED.atr", Layout::SpartaEnhanced),
        ("blank 32.g DD.atr", Layout::SpartaDouble),
    ] {
        let Ok(bytes) = std::fs::read(dir.join(file)) else {
            eprintln!("skipping {file} (not found)");
            continue;
        };
        let mut xinit = Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap();
        // Take off whatever was put on it (X32G.DOS, files copied in
        // testing).
        let files = xinit.list("").unwrap();
        for entry in &files {
            xinit.unlock(&entry.name).unwrap();
            xinit.delete(&entry.name).unwrap();
        }
        let had_dos = !files.is_empty();
        let ours = Volume::format(layout, Container::Atr).unwrap();
        assert_eq!(xinit.free_sectors(), ours.free_sectors(), "{file}");
        let (a, b) = (xinit.image(), ours.image());
        let (sa, sb) = (a.sector(1).unwrap(), b.sector(1).unwrap());
        // Root map, sector count, free count, bitmap place and size.
        assert_eq!(sa[0x09..0x12], sb[0x09..0x12], "{file}");
        // Directory search start, tracks, sector size, version.
        assert_eq!(sa[0x14..0x16], sb[0x14..0x16], "{file}");
        assert_eq!(sa[0x1E..0x21], sb[0x1E..0x21], "{file}");
        // SpartaDOS rewrites $21-$25 as it works, so they're as XINIT left
        // them only if nothing has written to the disk since (the sequence
        // number at $26 is still 0 or 1).
        if sa[0x26] <= 1 {
            assert_eq!(sa[0x21..0x26], sb[0x21..0x26], "{file}");
        }
        assert_eq!(sa[7], sb[7], "{file}");
        if !had_dos {
            assert_eq!(sa[0x12..0x14], sb[0x12..0x14], "{file}");
        }
        let (fa, fb) = (Sparta::open(a).unwrap(), Sparta::open(b).unwrap());
        for n in 0..=b.sector_count() {
            assert_eq!(fa.is_free(n), fb.is_free(n), "{file}: sector {n}");
        }
        let root = u32::from(fb.root());
        assert_eq!(a.sector(root), b.sector(root), "{file}");
    }
}

/// DOS 2.5 copied a 500-sector file (each byte its position) onto its
/// own enhanced-density system disk, running past sector 719. Writing the
/// same file to the same disk with this crate must give the same disk,
/// sector for sector: the `$03` flags, the chain and both VTOCs.
#[test]
fn write_matches_dos25() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/DOS 2");
    let load = |file: &str| {
        let bytes = std::fs::read(dir.join(file)).ok()?;
        Some(DiskImage::from_bytes(&bytes).unwrap())
    };
    let (Some(dos), Some(before)) = (
        load("dos25 wrote big.atr"),
        load("DOS 2.5 (1984)(Atari)(US)[!].atr"),
    ) else {
        eprintln!("skipping (samples not found)");
        return;
    };
    let data: Vec<u8> = (0..500 * 125).map(|i| i as u8).collect();
    let mut vol = Volume::open(before).unwrap();
    vol.write("BIG.DAT", &data, None).unwrap();
    let ours = vol.into_image();
    for n in 1..=dos.sector_count() {
        assert_eq!(dos.sector(n), ours.sector(n), "sector {n}");
    }
}
