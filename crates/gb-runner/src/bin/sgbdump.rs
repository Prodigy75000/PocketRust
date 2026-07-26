//! Throwaway: run a ROM headless and dump the SGB commands it emits.
use gb_core::GameBoy;
use std::fs;

fn main() {
    let path = std::env::args().nth(1).expect("rom path");
    let frames: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let rom = fs::read(&path).expect("read rom");
    let mut gb = GameBoy::new(rom);
    for _ in 0..frames {
        gb.step_frame();
    }
    let (log, active, palettes) = gb.sgb_debug();
    println!("active={active} commands={}", log.len());
    for (cmd, len) in &log {
        println!("  cmd=0x{:02X} bytes={}", cmd, len);
    }
    println!("palettes:");
    for (i, p) in palettes.iter().enumerate() {
        println!(
            "  SGB{i}: {:06X} {:06X} {:06X} {:06X}",
            p[0], p[1], p[2], p[3]
        );
    }
}
