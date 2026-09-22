//! What does an SGB cartridge do, and does it keep running? Diagnostic only.
use gb_core::GameBoy;
use std::collections::HashSet;

fn main() {
    let mut a = std::env::args().skip(1);
    let rom = std::fs::read(a.next().expect("usage: sgbprobe <rom>")).unwrap();
    let mut gb = GameBoy::new(rom);
    gb.set_sgb(std::env::var_os("SGB").is_some());
    println!("declares SGB = {}", gb.supports_sgb());

    // A PC histogram over one frame at the end: a stuck game spends all of it
    // in a handful of addresses, and those addresses name the wait.
    let total: u32 = std::env::var("FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(900);
    for f in 0..total {
        gb.step_frame();
        if f + 1 == total {
            let shades: HashSet<u32> = gb.framebuffer().iter().map(|p| p & 0xFF_FFFF).collect();
            let (cmds, active, pals) = gb.sgb_debug();
            let mut seen: Vec<String> = cmds
                .iter()
                .map(|(c, n)| format!("${c:02X}x{n}"))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            seen.sort();
            println!(
                "frame {f}: shades={} mask={} lcdc={:02X} bgp={:02X} pkts={} active={active} cmds=[{}]",
                shades.len(),
                gb.sgb_mask(),
                gb.peek(0xFF40),
                gb.peek(0xFF47),
                cmds.len(),
                seen.join(" ")
            );
            println!(
                "         palettes: {}",
                pals.iter()
                    .map(|p| p.iter().map(|c| format!("{c:06X}")).collect::<Vec<_>>().join("/"))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
    }

    use std::collections::HashMap;
    let mut hist: HashMap<u16, u32> = HashMap::new();
    for _ in 0..200_000 {
        gb.step();
        *hist.entry(gb.pc()).or_default() += 1;
    }
    let mut top: Vec<_> = hist.into_iter().collect();
    top.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    println!("hottest PCs (halted={}):", gb.halted());
    for (pc, n) in top.iter().take(12) {
        println!("   {pc:04X} x{n}");
    }
}
