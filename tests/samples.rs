//! Real disk images from `samples/<DOS>/` (not in git; each missing folder
//! is skipped). Every image must read and write back byte for byte.

use std::path::Path;

use atari_disk::DiskImage;

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
