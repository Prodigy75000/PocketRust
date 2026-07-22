//! Headless screenshot tool: run a ROM for N frames, then write the framebuffer
//! to a PNG. Handy for eyeballing the PPU and for regression baselines.
//!
//!   cargo run --release -p gb-runner --bin shot -- <rom.gb> <frames> <out.png>

use gb_core::{GameBoy, SCREEN_H, SCREEN_W};
use std::fs::File;
use std::io::BufWriter;

fn main() {
    let mut args = std::env::args().skip(1);
    let rom_path = args.next().expect("usage: shot <rom.gb> <frames> <out.png>");
    let frames: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(600);
    let out = args.next().unwrap_or_else(|| "shot.png".into());

    let rom = std::fs::read(&rom_path).expect("failed to read ROM");
    let mut gb = GameBoy::new(rom);
    match std::env::var("GBCOLOR").as_deref() {
        Ok("auto") => gb.set_colorization(gb_core::Colorize::Auto),
        Ok("grayscale") => gb.set_colorization(gb_core::Colorize::Grayscale),
        _ => {}
    }
    println!("Loaded '{}', running {frames} frames...", gb.title());

    let debug = std::env::var("GBDEBUG").is_ok();
    let mut serial = String::new();
    for i in 0..frames {
        gb.step_frame();
        let bytes = gb.take_serial();
        if !bytes.is_empty() {
            serial.push_str(&String::from_utf8_lossy(&bytes));
        }
        if debug && i % 200 == 0 {
            let (pc, lcdc, ly, halted) = gb.debug_state();
            println!("frame {i}: PC={pc:04X} LCDC={lcdc:02X} LY={ly} halted={halted}");
        }
    }
    if debug && !serial.is_empty() {
        println!("--- serial ---\n{serial}\n--------------");
    }
    if debug {
        // Blargg in-memory result protocol at 0xA000.
        let sig = [gb.peek(0xA001), gb.peek(0xA002), gb.peek(0xA003)];
        println!(
            "0xA000: result={:02X} sig={:02X} {:02X} {:02X}",
            gb.peek(0xA000),
            sig[0],
            sig[1],
            sig[2]
        );
        let mut text = String::new();
        for a in 0xA004u16..0xA044 {
            let b = gb.peek(a);
            if b == 0 {
                break;
            }
            text.push(b as char);
        }
        println!("0xA004 text: {text:?}");
    }

    // The framebuffer is 0x00RRGGBB; unpack to RGB bytes for the PNG.
    let frame = gb.framebuffer();
    let mut rgb = Vec::with_capacity(SCREEN_W * SCREEN_H * 3);
    for &px in frame {
        rgb.push((px >> 16) as u8);
        rgb.push((px >> 8) as u8);
        rgb.push(px as u8);
    }

    let file = File::create(&out).expect("create png");
    let mut encoder = png::Encoder::new(BufWriter::new(file), SCREEN_W as u32, SCREEN_H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgb)
        .unwrap();
    println!("Wrote {out}");
}
