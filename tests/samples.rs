//! Real disk images from `samples/<DOS>/` (not in git; each missing folder
//! is skipped). Every image must read and write back byte for byte.

use std::path::Path;

use atari_disk::{DiskImage, Dos2};

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
        }
        let all = (0..=image.sector_count().min(1023))
            .filter(|&n| fs.is_free(n))
            .count() as u32;
        assert_eq!(all, fs.free_sectors(), "{name}");
    }
}
