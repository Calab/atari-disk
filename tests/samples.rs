//! Real disk images from `samples/<DOS>/` (not in git; each missing folder
//! is skipped). Every image must read and write back byte for byte.

use std::path::Path;

use atari_disk::{
    Container, DiskImage, Dos2, EntryKind, Error, FileSystem, Layout, Problem, SectorUse, Sparta,
    Timestamp, Volume,
};

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
        let mut dirs = vec![(String::new(), fs.root())];
        while let Some((path, dir)) = dirs.pop() {
            for entry in fs.list_dir(dir).unwrap_or_else(|e| panic!("{name}: {e}")) {
                assert!(!entry.damaged, "{name}: {path}{}", entry.file_name());
                if entry.is_dir() {
                    println!("  {path}{}>", entry.file_name());
                    dirs.push((
                        format!("{path}{}>", entry.file_name()),
                        u32::from(entry.start),
                    ));
                    continue;
                }
                let data = fs.read(&entry).unwrap_or_else(|e| panic!("{name}: {e}"));
                let per_sector = image.sector_size() - 3;
                println!(
                    "  {path}{:12} {:4} sectors {:7} bytes{}",
                    entry.file_name(),
                    entry.sectors,
                    data.len(),
                    if entry.locked() { " locked" } else { "" }
                );
                assert_eq!(data.len() as u32, entry.size);
                assert!(data.len() <= usize::from(entry.sectors) * per_sector);
            }
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
        let last = if fs.mydos() {
            image.sector_count()
        } else {
            image.sector_count().min(1023)
        };
        let all = (0..=last).filter(|&n| fs.is_free(n)).count() as u32;
        assert_eq!(all, fs.free_sectors(), "{name}");
    }
}

/// Writing a file to each DOS 2 sample and deleting it again leaves the
/// free count as it was, and doesn't disturb the files already there.
#[test]
fn dos2_samples_write_and_delete() {
    for path in images() {
        let image = DiskImage::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let Some(fs) = Volume::detect(&image) else {
            continue;
        };
        if matches!(fs, FileSystem::SpartaDos | FileSystem::SpartaDosX) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        let mut vol = Volume::open(image).unwrap();
        let before: Vec<_> = vol.list("").unwrap();
        let contents: Vec<_> = before
            .iter()
            .filter(|e| !e.is_dir())
            .map(|e| vol.read(&e.name).unwrap())
            .collect();
        let free = vol.free_sectors();
        let data: Vec<u8> = (0..vol.free_bytes().min(20_000) - 3000)
            .map(|i| i as u8)
            .collect();
        vol.write("TEST.DAT", &data, None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(vol.read("TEST.DAT").unwrap(), data, "{name}");
        if fs == FileSystem::MyDos {
            // A directory, a file in it, and a move there and back.
            vol.make_dir("NEWDIR", None)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            vol.write("NEWDIR>IN.DAT", &data[..1000], None).unwrap();
            assert_eq!(vol.read("NEWDIR>IN.DAT").unwrap(), data[..1000], "{name}");
            vol.move_to("TEST.DAT", "NEWDIR").unwrap();
            assert_eq!(vol.read("NEWDIR>TEST.DAT").unwrap(), data, "{name}");
            assert!(matches!(
                vol.delete("NEWDIR"),
                Err(Error::DirectoryNotEmpty(_))
            ));
            vol.move_to("NEWDIR>TEST.DAT", "").unwrap();
            assert_eq!(vol.read("TEST.DAT").unwrap(), data, "{name}");
            vol.delete("NEWDIR>IN.DAT").unwrap();
            vol.delete("NEWDIR").unwrap();
        }
        vol.delete("TEST.DAT").unwrap();
        assert_eq!(vol.free_sectors(), free, "{name}");
        assert_eq!(vol.list("").unwrap(), before, "{name}");
        let files = before.iter().filter(|e| !e.is_dir());
        for (entry, old) in files.zip(&contents) {
            assert_eq!(&vol.read(&entry.name).unwrap(), old, "{name}");
        }
    }
}

/// Steve's MyDOS 4.5 disks, emptied, against blanks we format: the VTOC
/// (every sector of it) must match.
#[test]
fn mydos_format_matches() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/MyDOS");
    for (file, layout) in [
        ("SD MyDOS45.atr", Layout::MyDosSingle),
        ("MD MyDOS45.atr", Layout::MyDosEnhanced),
        ("DD MyDOS45.atr", Layout::MyDosDouble),
        (
            "Large 128 MyDOS45.atr",
            Layout::MyDosCustom {
                sectors: 65535,
                sector_size: 128,
            },
        ),
        (
            "Large 256 MyDOS45.atr",
            Layout::MyDosCustom {
                sectors: 65535,
                sector_size: 256,
            },
        ),
    ] {
        let Ok(bytes) = std::fs::read(dir.join(file)) else {
            eprintln!("skipping {file} (not found)");
            continue;
        };
        let mut dos = Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap();
        assert_eq!(dos.file_system(), FileSystem::MyDos, "{file}");
        // Empty it: files in directories first.
        let mut dirs = vec![String::new()];
        let mut order = Vec::new();
        while let Some(d) = dirs.pop() {
            for e in dos.list(&d).unwrap() {
                let path = format!("{d}{}", e.name_text());
                if e.is_dir() {
                    dirs.push(format!("{path}>"));
                }
                order.push(path);
            }
        }
        for path in order.iter().rev() {
            dos.unlock(path).unwrap();
            dos.delete(path)
                .unwrap_or_else(|e| panic!("{file}: {path}: {e}"));
        }
        let ours = Volume::format(layout, Container::Atr).unwrap();
        assert_eq!(dos.free_sectors(), ours.free_sectors(), "{file}");
        let vtoc = Dos2::open(ours.image()).unwrap().vtoc_sectors();
        for n in 360 - vtoc + 1..=360 {
            assert_eq!(
                dos.image().sector(n).unwrap(),
                ours.image().sector(n).unwrap(),
                "{file}: VTOC sector {n}"
            );
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
            atari_disk::atascii::atascii_to_ascii(&fs.volume_name()),
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
                let path = format!("{dir}{}", entry.name_text());
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
        let when = Timestamp::new(2026, 11, 24, 15, 30, 0).unwrap();
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

/// Every sample validates clean, except the Turbo-Basic disk's one
/// known flaw. The sector map agrees with the free counts.
#[test]
fn samples_validate() {
    for path in images() {
        let image = DiskImage::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let vol = Volume::open(image).unwrap();
        if vol.file_system() == FileSystem::Raw {
            println!("{name}: no file system (raw)");
            assert!(matches!(vol.validate(), Err(Error::Unsupported(_))));
            continue;
        }
        let problems = vol.validate().unwrap();
        println!(
            "{name}: {:?}, {} problems",
            vol.file_system(),
            problems.len()
        );
        for p in problems.iter().take(8) {
            println!("  {p}");
        }
        let map = vol.sector_map().unwrap();
        assert_eq!(map.len() as u32, vol.image().sector_count() + 1);
        let free = map.iter().filter(|u| **u == SectorUse::Free).count() as u32;
        if name.starts_with("Turbo-Basic XL 3.2q") {
            assert_eq!(
                problems,
                [Problem::MarkedFree {
                    sector: 7,
                    owner: SectorUse::Directory(Vec::new())
                }]
            );
            assert_eq!(free + 1, vol.free_sectors(), "{name}");
        } else {
            assert!(problems.is_empty(), "{name}: {problems:?}");
            assert_eq!(free, vol.free_sectors(), "{name}");
        }
    }
}

/// Making our blank disks bootable from Steve's DOS disks gives the boot
/// sectors DOS itself wrote on his blanks, and copies of the DOS files.
#[test]
fn bootable_matches_dos() {
    let load = |dir: &str, file: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(dir)
            .join(file);
        Some(DiskImage::from_bytes(&std::fs::read(path).ok()?).unwrap())
    };
    let boot = |image: &DiskImage| -> Vec<u8> {
        (1..=3)
            .flat_map(|n| image.sector(n).unwrap().to_vec())
            .collect()
    };

    // DOS 2.0S onto double density: as 2.0S's own DUP wrote it.
    if let (Some(dos), Some(theirs)) = (
        load("DOS 2", "Dos 2.0s.atr"),
        load("DOS 2", "blank DOS2 DD.atr"),
    ) {
        let mut vol = Volume::format(Layout::Dos2Double, Container::Atr).unwrap();
        assert!(!vol.is_bootable());
        vol.make_bootable(&dos).unwrap();
        assert!(vol.is_bootable());
        assert_eq!(boot(vol.image()), boot(&theirs));
        // DUP writes DOS.SYS from memory, so theirs isn't a byte copy;
        // ours is the source's.
        let src = Volume::open(dos.clone()).unwrap();
        for file in ["DOS.SYS", "DUP.SYS"] {
            assert_eq!(vol.read(file).unwrap(), src.read(file).unwrap(), "{file}");
        }
        assert_eq!(vol.validate().unwrap(), []);
        // Again: DOS.SYS is there now.
        assert!(matches!(
            vol.make_bootable(&dos),
            Err(Error::AlreadyExists(_))
        ));
        // Not from a disk with no DOS, or for another DOS.
        let blank = Volume::format(Layout::Dos2Single, Container::Atr).unwrap();
        assert!(matches!(
            vol.make_bootable(blank.image()),
            Err(Error::NotBootable(_))
        ));
        let mut sparta = Volume::format(Layout::SpartaSingle, Container::Atr).unwrap();
        assert!(matches!(
            sparta.make_bootable(&dos),
            Err(Error::NotBootable(_))
        ));
    }

    // SpartaDOS 3.2g onto single density: as XINIT made it.
    if let (Some(sparta), Some(theirs)) = (
        load("SpartaDOS", "Spartados 3.2g.ATR"),
        load("SpartaDOS", "blank 32.g SD.atr"),
    ) {
        let mut vol = Volume::format(Layout::SpartaSingle, Container::Atr).unwrap();
        vol.make_bootable(&sparta).unwrap();
        let (ours, xinit) = (boot(vol.image()), boot(&theirs));
        for i in (0..384).filter(|i| !(0x09..0x2B).contains(i)) {
            assert_eq!(ours[i], xinit[i], "boot byte {i:#x}");
        }
        // The boot file's sector map: X32G.DOS at sector 32 on both.
        assert_eq!(ours[0x28..0x2A], xinit[0x28..0x2A]);
        let theirs = Volume::open(theirs).unwrap();
        assert_eq!(
            vol.read("X32G.DOS").unwrap(),
            theirs.read("X32G.DOS").unwrap()
        );
        assert_eq!(vol.validate().unwrap(), []);
        // SpartaDOS X disks have no boot file.
        if let Some(sdx) = load("SpartaDOS", "SDX 4.49e Toolkit.atr") {
            let mut vol = Volume::format(Layout::SpartaDouble, Container::Atr).unwrap();
            assert!(matches!(
                vol.make_bootable(&sdx),
                Err(Error::NotBootable(_))
            ));
        }
    }

    // MyDOS 4.5D onto a large disk: the boot sector points at the new
    // DOS.SYS, which has 16-bit links there.
    if let Some(mydos) = load("MyDOS", "MYDOS45D.ATR") {
        let mut vol = Volume::format(
            Layout::MyDosCustom {
                sectors: 4000,
                sector_size: 256,
            },
            Container::Atr,
        )
        .unwrap();
        vol.make_bootable(&mydos).unwrap();
        let ours = boot(vol.image());
        let src = boot(&mydos);
        assert_eq!(ours[..0x0E], src[..0x0E]);
        assert_eq!(ours[0x11..], src[0x11..]);
        let dos = Dos2::open(vol.image()).unwrap().find("DOS.SYS").unwrap();
        assert_eq!(u16::from_le_bytes([ours[0x0F], ours[0x10]]), dos.start);
        assert!(dos.big_links());
        assert!(vol.exists("DUP.SYS"));
        assert_eq!(vol.validate().unwrap(), []);
        let mut small = Volume::format(
            Layout::MyDosCustom {
                sectors: 2000,
                sector_size: 512,
            },
            Container::Atr,
        );
        assert!(small.is_err() || small.as_mut().unwrap().make_bootable(&mydos).is_err());
    }
}

/// `is_bootable` on Steve's samples: his DOS disks are, his blanks and
/// SpartaDOS X disks aren't.
#[test]
fn samples_is_bootable() {
    let expected = [
        ("DOS 2", "Dos 2.0s.atr", true),
        ("DOS 2", "DOS 2.5 (1984)(Atari)(US)[!].atr", true),
        ("DOS 2", "blank DOS2.5 ED.atr", false),
        ("MyDOS", "MYDOS45D.ATR", true),
        ("MyDOS", "SD MyDOS45.atr", true),
        ("SpartaDOS", "Spartados 3.2g.ATR", true),
        ("SpartaDOS", "blank 32.g SD.atr", true),
        ("SpartaDOS", "blank 32.g DD.atr", false),
        ("SpartaDOS", "SDX 4.49e Toolkit.atr", false),
        ("SpartaDOS", "SpartaDOS HDD.atr", false),
        ("SpartaDOS", "SpartaDOS DD formatted.atr", false),
    ];
    for (dir, file, bootable) in expected {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(dir)
            .join(file);
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("skipping {file} (not found)");
            continue;
        };
        let vol = Volume::open(DiskImage::from_bytes(&bytes).unwrap()).unwrap();
        assert_eq!(vol.is_bootable(), bootable, "{file}");
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
    let when = |day: u8, hour: u8| Some(Timestamp::new(2026, 11, day, hour, 30, 0).unwrap());
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
            format!("FILE{i:03}.TXT"),
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

    // MyDOS: files, nested directories and a move, on three layouts.
    for (layout, file) in [
        (Layout::MyDosSingle, "mydos-files-sd.atr"),
        (Layout::MyDosDouble, "mydos-files-dd.atr"),
        (
            Layout::MyDosCustom {
                sectors: 8000,
                sector_size: 256,
            },
            "mydos-files-big.atr",
        ),
    ] {
        let mut vol = Volume::format(layout, Container::Atr).unwrap();
        vol.write("HELLO.LST", &program, None).unwrap();
        vol.write("TEXT.TXT", &text, None).unwrap();
        vol.lock("TEXT.TXT").unwrap();
        vol.write("BIG.DAT", &pattern(30_000), None).unwrap();
        vol.make_dir("GAMES", None).unwrap();
        vol.make_dir("GAMES>OLD", None).unwrap();
        vol.write("GAMES>OLD>MOVED.DAT", &pattern(1000), None)
            .unwrap();
        vol.write("GAMES>STAYS.DAT", &pattern(300), None).unwrap();
        // Moved out of OLD: its sectors get the new file number.
        vol.move_to("GAMES>OLD>MOVED.DAT", "GAMES").unwrap();
        assert_eq!(vol.validate().unwrap(), []);
        save(&vol, file);
    }

    // Bootable disks, from Steve's DOS disks.
    let sample = |dir: &str, file: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(dir)
            .join(file);
        Some(DiskImage::from_bytes(&std::fs::read(path).ok()?).unwrap())
    };
    let boots = [
        (
            "DOS 2",
            "Dos 2.0s.atr",
            Layout::Dos2Single,
            "boot-dos20-sd.atr",
        ),
        (
            "DOS 2",
            "Dos 2.0s.atr",
            Layout::Dos2Double,
            "boot-dos20-dd.atr",
        ),
        (
            "DOS 2",
            "DOS 2.5 (1984)(Atari)(US)[!].atr",
            Layout::Dos25Enhanced,
            "boot-dos25-ed.atr",
        ),
        (
            "MyDOS",
            "MYDOS45D.ATR",
            Layout::MyDosDouble,
            "boot-mydos-dd.atr",
        ),
        (
            "MyDOS",
            "MYDOS45D.ATR",
            Layout::MyDosSingle,
            "boot-mydos-sd.atr",
        ),
        (
            "SpartaDOS",
            "Spartados 3.2g.ATR",
            Layout::SpartaSingle,
            "boot-sparta-sd.atr",
        ),
        (
            "SpartaDOS",
            "Spartados 3.2g.ATR",
            Layout::SpartaDouble,
            "boot-sparta-dd.atr",
        ),
        (
            "SpartaDOS",
            "Spartados 3.2g.ATR",
            Layout::SpartaEnhanced,
            "boot-sparta-ed.atr",
        ),
    ];
    for (dir, source, layout, file) in boots {
        let Some(source) = sample(dir, source) else {
            continue;
        };
        let mut vol = Volume::format(layout, Container::Atr).unwrap();
        if vol.features().volume_name {
            vol.set_volume_name("BOOTABLE").unwrap();
        }
        vol.make_bootable(&source).unwrap();
        vol.write("HELLO.LST", &program, None).unwrap();
        save(&vol, file);
    }

    // Double-sided disks (XF551: 1440 sectors of 256 bytes).
    for (layout, file) in [
        (Layout::SpartaDoubleSided, "sparta-ds.atr"),
        (Layout::MyDosDoubleSided, "mydos-ds.atr"),
    ] {
        let mut vol = Volume::format(layout, Container::Atr).unwrap();
        if vol.features().volume_name {
            vol.set_volume_name("DSDD").unwrap();
        }
        vol.write("HELLO.LST", &program, None).unwrap();
        vol.write("BIG.DAT", &pattern(200_000), None).unwrap();
        save(&vol, file);
    }

    // SpartaDOS flags: hidden and archived files.
    let mut vol = Volume::format(Layout::SpartaSingle, Container::Atr).unwrap();
    vol.set_volume_name("FLAGS").unwrap();
    vol.write("SHOWN.TXT", &text, when(24, 10)).unwrap();
    vol.write("HIDDEN.TXT", &text, when(24, 11)).unwrap();
    vol.set_hidden("HIDDEN.TXT", true).unwrap();
    vol.write("ARCHIVED.TXT", &text, when(24, 12)).unwrap();
    vol.set_archived("ARCHIVED.TXT", true).unwrap();
    vol.set_timestamp("SHOWN.TXT", when(1, 23)).unwrap();
    save(&vol, "sparta-flags.atr");
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
