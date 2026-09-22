//! What does an SGB cartridge do, and does it keep running? Diagnostic only.
use gb_core::GameBoy;
use std::collections::HashSet;

fn main() {
    let mut a = std::env::args().skip(1);
    let rom = std::fs::read(a.next().expect("usage: sgbprobe <rom>")).unwrap();
    let mut gb = GameBoy::new(rom);
    gb.set_sgb(std::env::var_os("SGB").is_some());
    println!("declares SGB = {}", gb.supports_sgb());

    for f in 0..900u32 {
        gb.step_frame();
        if f % 150 == 149 {
            let shades: HashSet<u32> = gb.framebuffer().iter().map(|p| p & 0xFF_FFFF).collect();
            let (cmds, active, _) = gb.sgb_debug();
            let mut seen: Vec<String> = cmds
                .iter()
                .map(|(c, n)| format!("${c:02X}x{n}"))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            seen.sort();
            println!(
                "frame {f}: shades={} mask={} active={active} cmds=[{}]",
                shades.len(),
                gb.sgb_mask(),
                seen.join(" ")
            );
        }
    }
}
