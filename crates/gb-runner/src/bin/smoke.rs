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

const FRAMES: u32 = 600; // ~10s at 60 fps

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
        // Blindly pulse Start then A each 64-frame window to advance intros.
        let phase = f % 64;
        gb.set_button(Button::Start, phase < 6);
        gb.set_button(Button::A, (32..38).contains(&phase));
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

fn main() {
    let dir = std::env::args().nth(1).expect("usage: smoke <dir>");
    let mut roms: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gb")))
        .collect();
    roms.sort();

    panic::set_hook(Box::new(|_| {})); // silence per-ROM panic spew
    let (mut panics, mut unsup, mut blank, mut silent) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut clean = 0u32;

    for (i, p) in roms.iter().enumerate() {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(p).unwrap();
        let ctype = *bytes.get(0x147).unwrap_or(&0);
        eprintln!("[{}/{}] {}", i + 1, roms.len(), name); // progress => pinpoints a hang
        if !core_supports_mbc(ctype) {
            unsup.push(format!("{} [{}]", name, mbc_name(ctype)));
            continue;
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run_one(bytes))) {
            Err(_) => panics.push(name),
            Ok((b, s)) => {
                if b {
                    blank.push(name.clone());
                }
                if s {
                    silent.push(name.clone());
                }
                if !b && !s {
                    clean += 1;
                }
            }
        }
    }
    let _ = panic::take_hook();

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
