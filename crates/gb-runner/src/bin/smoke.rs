//! Batch compatibility smoke-test. Boots every .gb in a directory, runs it for a
//! few seconds with blind Start/A pulses to get past intros, and flags anything
//! that panics, uses an unsupported MBC, renders a blank screen, or stays silent.
//!
//!   cargo run -p gb-runner --release --bin smoke -- <dir>
//!
//! Blank = <=1 distinct colour after the run (strong "didn't boot" signal).
//! Silent = never produced audio (soft signal; many title screens are quiet).

use gb_core::{Button, Colorize, GameBoy};
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const FRAMES: u32 = 900; // ~15s at 60 fps (some intros are slow, e.g. Pokemon)

/// Mirror cartridge.rs: the MBC types the core actually maps to a real mapper.
fn core_supports_mbc(t: u8) -> bool {
    matches!(t, 0x00 | 0x01 | 0x02 | 0x03 | 0x05 | 0x06 | 0x0F..=0x13 | 0x19..=0x1E)
}

fn mbc_name(t: u8) -> &'static str {
    match t {
        0x00 => "ROM",
        0x01..=0x03 => "MBC1",
        0x05 | 0x06 => "MBC2",
        0x08 | 0x09 => "ROM+RAM",
        0x0B..=0x0D => "MMM01",
        0x0F..=0x13 => "MBC3",
        0x19..=0x1E => "MBC5",
        0x20 => "MBC6",
        0x22 => "MBC7",
        0xFC => "Camera",
        0xFD => "TAMA5",
        0xFE => "HuC3",
        0xFF => "HuC1",
        _ => "?",
    }
}

/// Returns (never_rendered, silent). `never_rendered` = the screen never showed
/// more than one colour at ANY sampled point (robust to mid-transition black
/// frames), which is the real "didn't boot" signal.
fn run_one(rom: Vec<u8>) -> (bool, bool) {
    let mut gb = GameBoy::new(rom);
    gb.set_colorization(Colorize::Auto);
    let mut any_audio = false;
    let mut max_colors = 0usize;
    for f in 0..FRAMES {
        // Let the game render its own intro/title uninterrupted for the first
        // two-thirds (a boot test only needs to see it draw *something*, and
        // mashing early skips intros before they render -> false "blank"), then
        // pulse Start in the last third for games parked on a "press start" boot
        // screen. Captures both attract-intro games (Pokemon) and press-start.
        let mashing = f >= FRAMES * 2 / 3;
        gb.set_button(Button::Start, mashing && f % 16 < 4);
        gb.step_frame();
        if !any_audio && gb.take_audio().iter().any(|s| s.unsigned_abs() > 64) {
            any_audio = true;
        }
        if f % 30 == 29 {
            let colors: HashSet<u32> = gb.framebuffer().iter().map(|p| p & 0xFF_FFFF).collect();
            max_colors = max_colors.max(colors.len());
        }
    }
    (max_colors <= 1, !any_audio)
}

/// Recursively collect every .gb / .gbc ROM under `dir` (the SMDB packs nest
/// ROMs several folders deep, by region and SGB/GBC category).
fn collect_roms(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            collect_roms(&p, out);
        } else if p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gb") || e.eq_ignore_ascii_case("gbc"))
        {
            out.push(p);
        }
    }
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: smoke <dir>"));
    let mut roms: Vec<PathBuf> = Vec::new();
    collect_roms(&dir, &mut roms);
    roms.sort();

    panic::set_hook(Box::new(|_| {})); // silence per-ROM panic spew

    #[derive(Default)]
    struct Report {
        panics: Vec<String>,
        unsup: Vec<String>,
        blank: Vec<String>,
        silent: Vec<String>,
        clean: u32,
    }

    let total = roms.len();
    let roms = Arc::new(roms);
    let report = Arc::new(Mutex::new(Report::default()));
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());

    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let (roms, report, next, done) =
                (roms.clone(), report.clone(), next.clone(), done.clone());
            std::thread::spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= total {
                    break;
                }
                let p = &roms[i];
                let name = p.file_stem().unwrap().to_string_lossy().to_string();
                let bytes = std::fs::read(p).unwrap();
                let ctype = *bytes.get(0x147).unwrap_or(&0);
                let outcome = if !core_supports_mbc(ctype) {
                    Err(mbc_name(ctype))
                } else {
                    Ok(panic::catch_unwind(AssertUnwindSafe(|| run_one(bytes))))
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 200 == 0 {
                    eprintln!("[{}/{}]", n, total);
                }
                let mut r = report.lock().unwrap();
                match outcome {
                    Err(mbc) => r.unsup.push(format!("{} [{}]", name, mbc)),
                    Ok(Err(_)) => r.panics.push(name),
                    Ok(Ok((b, s))) => {
                        if b {
                            r.blank.push(name.clone());
                        }
                        if s {
                            r.silent.push(name.clone());
                        }
                        if !b && !s {
                            r.clean += 1;
                        }
                    }
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let _ = panic::take_hook();

    let mut report = Arc::try_unwrap(report).ok().unwrap().into_inner().unwrap();
    for v in [
        &mut report.panics,
        &mut report.unsup,
        &mut report.blank,
        &mut report.silent,
    ] {
        v.sort();
    }
    let (panics, unsup, blank, silent, clean) = (
        report.panics,
        report.unsup,
        report.blank,
        report.silent,
        report.clean,
    );

    let group = |title: &str, v: &[String]| {
        println!("\n{} ({})", title, v.len());
        for n in v {
            println!("  {}", n);
        }
    };
    println!("\n==================== SMOKE REPORT: {} ROMs ====================", roms.len());
    println!("clean (video + audio): {}", clean);
    group("PANIC", &panics);
    group("UNSUPPORTED MBC", &unsup);
    group("BLANK screen", &blank);
    group("SILENT (soft signal)", &silent);
}
