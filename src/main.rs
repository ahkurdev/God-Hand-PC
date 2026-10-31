//! godhand-rs — native Rust port of God Hand (PS2, SLES-544.90).
//!
//! This binary is the asset pipeline entry point. It reads the shipped disc
//! artifacts directly and reports what it finds, which is the foundation the
//! engine is built on.
//!
//! ## Usage
//!
//! ```text
//! godhand-rs inspect <disc-dir>          # report disc + AFS + FST structure
//! godhand-rs list <disc-dir> [filter]    # list AFS entries with FST names
//! godhand-rs extract <disc-dir> <index> <out.rgba>
//!                                           # decode one TIM2 to RGBA8
//! godhand-rs dump-strings <disc-dir> <index>
//!                                           # printable strings from an entry
//! ```

mod afs;
mod fst;
mod recomp;
mod tim2;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Default file names inside an extracted God Hand disc.
const AFS_NAME: &str = "_DATA.AFS";
const FST_NAME: &str = "GODHAND.FST";
const ELF_NAME: &str = "SLES_544.90";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            for cause in err.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Dispatch a subcommand, returning an error for unknown or malformed input.
fn run(args: &[String]) -> anyhow::Result<()> {
    match args.get(1).map(|s| s.as_str()) {
        Some("inspect") => {
            let dir = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            cmd_inspect(&dir)
        }
        Some("list") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("usage: list <disc-dir> [filter]"))?;
            let filter = args.get(3).cloned();
            cmd_list(&dir, filter.as_deref())
        }
        Some("extract") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("usage: extract <disc-dir> <index> <out>"))?;
            let index: usize = args
                .get(3)
                .ok_or_else(|| anyhow::anyhow!("missing AFS index"))?
                .parse()
                .map_err(|_| anyhow::anyhow!("index must be a non-negative integer"))?;
            let out = args
                .get(4)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("missing output path"))?;
            cmd_extract(&dir, index, &out)
        }
        Some("dump-strings") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("usage: dump-strings <disc-dir> <index>"))?;
            let index: usize = args
                .get(3)
                .ok_or_else(|| anyhow::anyhow!("missing AFS index"))?
                .parse()
                .map_err(|_| anyhow::anyhow!("index must be a non-negative integer"))?;
            cmd_dump_strings(&dir, index)
        }
        Some("recomp-profile") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("usage: recomp-profile <disc-dir>"))?;
            cmd_recomp_profile(&dir)
        }
        Some("extract-all") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("usage: extract-all <disc-dir> <out-dir>"))?;
            let out_dir = args
                .get(3)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("missing output directory"))?;
            cmd_extract_all(&dir, &out_dir)
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            Err(anyhow::anyhow!("unknown subcommand '{other}'"))
        }
    }
}

/// Print CLI usage.
fn print_usage() {
    println!(
        "godhand-rs — God Hand (PS2) native port, asset pipeline\n\
         \n\
         Usage:\n\
         \x20 inspect <disc-dir>              report disc + AFS + FST structure\n\
         \x20 list <disc-dir> [filter]        list AFS entries with FST names\n\
         \x20 extract <disc-dir> <i> <out>    decode AFS entry <i> as TIM2 to a PNG\n\
         \x20 extract-all <disc-dir> <dir>    decode every TIM2 in the archive into <dir>\n\
         \x20 dump-strings <disc-dir> <i>     printable strings from AFS entry <i>\n\
         \n\
         <disc-dir> is the directory holding {AFS_NAME}, {FST_NAME} and {ELF_NAME}."
    );
}

/// Locate a required disc artifact inside the given directory.
fn disc_file(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(name);
    if !path.is_file() {
        anyhow::bail!("{} not found in {}", name, dir.display());
    }
    Ok(path)
}

/// Report the structure of the disc artifacts.
fn cmd_inspect(dir: &Path) -> anyhow::Result<()> {
    let afs_path = disc_file(dir, AFS_NAME)?;
    let fst_path = disc_file(dir, FST_NAME)?;

    let afs_bytes = fs::read(&afs_path)?;
    let afs = afs::Afs::parse(&afs_bytes)?;

    let fst_bytes = fs::read(&fst_path)?;
    let fst = fst::Fst::parse(&fst_bytes)?;

    println!("=== GOD HAND disc: {} ===", dir.display());
    println!("AFS : {} bytes, {} entries", afs_bytes.len(), afs.len());
    println!(
        "      flags=0x{:04X} version={} align={}",
        afs.flags, afs.version, afs.align
    );
    println!(
        "FST : version={} groups={} strings={}",
        fst.version,
        fst.group_count,
        fst.strings.len()
    );

    let names = fst.file_names();
    println!(
        "      {} file names, {} directories",
        names.len(),
        fst.directories().len()
    );

    let mut hist: Vec<(String, usize)> = fst.extension_histogram().into_iter().collect();
    hist.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("      extensions:");
    for (ext, count) in hist {
        println!("        .{ext:<6} {count}");
    }

    println!("      {} .rel dynamic modules", fst.rel_modules().len());

    // Cross-check: FST names should cover the AFS entry count.
    if names.len() == afs.len() {
        println!("      FST names match AFS entry count exactly");
    } else {
        println!(
            "      note: FST names ({}) differ from AFS entries ({})",
            names.len(),
            afs.len()
        );
    }

    // Largest payloads, which reveal where the disc's weight sits.
    println!("      largest payloads:");
    let mut sized: Vec<(usize, u32)> = afs.iter().map(|(i, e)| (i, e.size)).collect();
    sized.sort_by(|a, b| b.1.cmp(&a.1));
    for (index, size) in sized.iter().take(5) {
        let name = fst.name_for_index(*index).unwrap_or("<unnamed>");
        println!("        [{index:>4}] {name:<24} {:>10} bytes", format_bytes(*size as u64));
    }

    Ok(())
}

/// List AFS entries, optionally filtered by name substring.
fn cmd_list(dir: &Path, filter: Option<&str>) -> anyhow::Result<()> {
    let afs_path = disc_file(dir, AFS_NAME)?;
    let fst_path = disc_file(dir, FST_NAME)?;
    let afs = afs::Afs::open(&afs_path)?;
    let fst = fst::Fst::parse(&fs::read(&fst_path)?)?;

    let mut shown = 0usize;
    for (index, entry) in afs.iter() {
        let name = fst.name_for_index(index).unwrap_or("<unnamed>");
        if let Some(needle) = filter {
            if !name.contains(needle) {
                continue;
            }
        }
        println!(
            "[{index:>4}] {name:<28} off=0x{:08X} size={:>10}",
            entry.offset, entry.size
        );
        shown += 1;
    }
    println!("({shown} entries shown, {} total)", afs.len());
    Ok(())
}

/// Decode one AFS entry as a TIM2 texture and write it as a PNG.
///
/// A PNG is written rather than raw RGBA so the result can be opened directly
/// and the CLUT unswizzle verified by eye.
fn cmd_extract(dir: &Path, index: usize, out: &Path) -> anyhow::Result<()> {
    let afs_path = disc_file(dir, AFS_NAME)?;
    let fst_path = disc_file(dir, FST_NAME)?;
    let afs = afs::Afs::open(&afs_path)?;
    let fst = fst::Fst::parse(&fs::read(&fst_path)?)?;

    let bytes = afs.read_entry(&afs_path, index)?;
    let name = fst.name_for_index(index).unwrap_or("<unnamed>");
    println!("entry [{index}] {name}: {} bytes", bytes.len());

    let texture = tim2::Tim2::parse(&bytes)?;
    let image = texture
        .first()
        .ok_or_else(|| anyhow::anyhow!("TIM2 declares no images"))?;

    println!(
        "  {}x{} psm={:?} mipmaps={}",
        image.width, image.height, image.psm, image.has_mipmaps
    );

    let rgba = image.to_rgba8()?;
    let img = image::RgbaImage::from_raw(image.width, image.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("decoded pixel buffer has wrong length"))?;
    img.save(out)?;

    println!(
        "  wrote {}x{} PNG to {}",
        image.width,
        image.height,
        out.display()
    );
    Ok(())
}

/// Print printable ASCII strings found in one AFS entry.
fn cmd_dump_strings(dir: &Path, index: usize) -> anyhow::Result<()> {
    let afs_path = disc_file(dir, AFS_NAME)?;
    let fst_path = disc_file(dir, FST_NAME)?;
    let afs = afs::Afs::open(&afs_path)?;
    let fst = fst::Fst::parse(&fs::read(&fst_path)?)?;

    let bytes = afs.read_entry(&afs_path, index)?;
    let name = fst.name_for_index(index).unwrap_or("<unnamed>");
    println!("entry [{index}] {name}: {} bytes", bytes.len());

    let mut current = Vec::new();
    for &b in &bytes {
        if (0x20..=0x7E).contains(&b) {
            current.push(b);
        } else {
            if current.len() >= 4 {
                println!("  {}", String::from_utf8_lossy(&current));
            }
            current.clear();
        }
    }
    if current.len() >= 4 {
        println!("  {}", String::from_utf8_lossy(&current));
    }
    Ok(())
}

/// Decode every TIM2 texture in the archive into an output directory.
///
/// Batch extraction is how the pipeline gets proven at scale: a handful of
/// hand-picked textures can look right by luck, but 35 of them decoding
/// without a single failure is a different claim.
fn cmd_extract_all(dir: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let afs_path = disc_file(dir, AFS_NAME)?;
    let fst_path = disc_file(dir, FST_NAME)?;
    let afs = afs::Afs::open(&afs_path)?;
    let fst = fst::Fst::parse(&fs::read(&fst_path)?)?;

    fs::create_dir_all(out_dir)?;

    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for (index, _entry) in afs.iter() {
        let name = fst.name_for_index(index).unwrap_or("<unnamed>");
        let bytes = match afs.read_entry(&afs_path, index) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[{index}] {name}: read failed: {e}");
                failed += 1;
                continue;
            }
        };

        // Only entries that actually parse as TIM2 are textures. The FST names
        // entries by logical role, so a `.tm2` name may hold an AFS, a SES
        // bank, or a REL module instead.
        let texture = match tim2::Tim2::parse(&bytes) {
            Ok(t) => t,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let Some(image) = texture.first() else {
            skipped += 1;
            continue;
        };

        let rgba = match image.to_rgba8() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[{index}] {name}: decode failed: {e}");
                failed += 1;
                continue;
            }
        };
        let Some(img) = image::RgbaImage::from_raw(image.width, image.height, rgba) else {
            eprintln!("[{index}] {name}: pixel buffer has wrong length");
            failed += 1;
            continue;
        };

        // Sanitise the name for the filesystem: keep alphanumerics, dots and
        // dashes, and prefix the index so ordering is stable.
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let out_path = out_dir.join(format!("{index:04}_{safe}.png"));
        if let Err(e) = img.save(&out_path) {
            eprintln!("[{index}] {name}: write failed: {e}");
            failed += 1;
            continue;
        }
        println!("[{index:>4}] {name:<24} {}x{} -> {}", image.width, image.height, out_path.display());
        ok += 1;
    }

    println!();
    println!("decoded {ok} textures, skipped {skipped} non-TIM2 entries, {failed} failed");
    Ok(())
}

/// Format a byte count with a human-readable suffix.
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Run the MIPS-to-x86-64 lifter over the shipped executable's `.text`.
///
/// This measures how much of God Hand's code the current lifter can translate
/// without trapping, which is the practical ceiling on a native port.
fn cmd_recomp_profile(dir: &Path) -> anyhow::Result<()> {
    let elf_path = disc_file(dir, ELF_NAME)?;
    let elf = fs::read(&elf_path)?;

    // .text lives at file offset 0x1000, virtual address 0x00100000, per the
    // verified ELF section table.
    let text_off = 0x1000usize;
    let text_size = 0x2C0760usize;
    if elf.len() < text_off + text_size {
        anyhow::bail!("executable too small to contain the expected .text");
    }
    let text = &elf[text_off..text_off + text_size];
    let text_vaddr = 0x0010_0000u32;

    let mut lifter = recomp::Lifter::new();
    let total = text.len() / 4;
    for i in 0..total {
        let word = u32::from_le_bytes([
            text[i * 4],
            text[i * 4 + 1],
            text[i * 4 + 2],
            text[i * 4 + 3],
        ]);
        let insn = recomp::Insn::decode(word);
        lifter.lift(&insn, text_vaddr + (i as u32) * 4);
    }

    let lifted = lifter.lifted;
    let trapped = lifter.trapped;
    let pct = 100.0 * lifted as f64 / total as f64;

    println!("=== MIPS -> x86-64 lift profile: {} ===", elf_path.display());
    println!("instructions : {total}");
    println!("lifted       : {lifted} ({pct:.1}%)");
    println!("trapped      : {trapped} ({:.1}%)", 100.0 - pct);
    println!(
        "emitted      : {} bytes of x86-64 ({:.2}x source)",
        lifter.code.len(),
        lifter.code.len() as f64 / text.len() as f64
    );

    Ok(())
}
