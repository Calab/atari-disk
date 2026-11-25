//! List everything on an Atari disk image, then check it.
//!
//! ```text
//! cargo run --example ls -- GAMES.ATR
//! ```
//!
//! Shows the image and its file system, every directory and file with its
//! size, date and flags, and any problems `validate` finds.

use atari_disk::{DiskImage, FileSystem, Volume};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run --example ls -- DISK.ATR");
        std::process::exit(2);
    };
    let image = DiskImage::from_bytes(&std::fs::read(&path)?)?;
    println!(
        "{path}: {:?}, {} sectors of {} bytes{}",
        image.container(),
        image.sector_count(),
        image.sector_size(),
        if image.write_protected() {
            ", write-protected"
        } else {
            ""
        }
    );

    let vol = Volume::open(image)?;
    if vol.file_system() == FileSystem::Raw {
        println!("No DOS 2, MyDOS or SpartaDOS file system: sectors only.");
        return Ok(());
    }
    let name = vol
        .volume_name()
        .map(|n| format!(", volume \"{}\"", atari_disk::atascii::atascii_to_ascii(&n)))
        .unwrap_or_default();
    println!(
        "{:?}{name}, {} sectors free ({} bytes){}",
        vol.file_system(),
        vol.free_sectors(),
        vol.free_bytes(),
        if vol.is_bootable() { ", bootable" } else { "" }
    );
    println!();
    list(&vol, b"", 0)?;

    let problems = vol.validate()?;
    println!();
    if problems.is_empty() {
        println!("No problems found.");
    }
    for problem in problems {
        println!("Problem: {problem}");
    }
    Ok(())
}

/// Print directory `dir` and, indented, the directories inside it.
fn list(vol: &Volume, dir: &[u8], depth: usize) -> atari_disk::Result<()> {
    for entry in vol.list(dir)? {
        let indent = "  ".repeat(depth);
        let date = entry
            .modified
            .map(|t| {
                format!(
                    "{:02}-{:02}-{} {:02}:{:02}",
                    t.day, t.month, t.year, t.hour, t.minute
                )
            })
            .unwrap_or_default();
        let mut flags = String::new();
        for (on, flag) in [
            (entry.locked, " locked"),
            (entry.hidden, " hidden"),
            (entry.archived, " archived"),
            (entry.damaged, " DAMAGED"),
        ] {
            if on {
                flags.push_str(flag);
            }
        }
        if entry.is_dir() {
            let line = format!("{indent}{}>  {date}{flags}", entry.name_text());
            println!("{}", line.trim_end());
            // Paths join names with `>`, as the stored ATASCII bytes.
            let inner = if dir.is_empty() {
                entry.name.clone()
            } else {
                [dir, b">", &entry.name].concat()
            };
            list(vol, &inner, depth + 1)?;
        } else {
            let line = format!(
                "{indent}{:12} {:8} bytes {:4} sectors  {date}{flags}",
                entry.name_text(),
                entry.size,
                entry.sectors
            );
            println!("{}", line.trim_end());
        }
    }
    Ok(())
}
