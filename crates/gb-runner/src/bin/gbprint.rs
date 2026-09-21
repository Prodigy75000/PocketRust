// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Drive a game into printing, with an emulated Game Boy Printer attached, and
//! write what came out.
//!
//!   cargo run --release -p gb-runner --bin gbprint -- <rom.gb> [options]
//!
//!     --state <file>    load a save state first, so a print deep in a game is
//!                       reachable without playing up to it
//!     --keys <script>   the same input script `shot` takes: a, b, up, start,
//!                       w60 to wait sixty frames, +a / -a to hold and release
//!     --frames <n>      how long to run after the script (default 600)
//!     --out <prefix>    where the pages go (default "print")
//!     --shot <file>     also write what is on the screen at the end, which is
//!                       the first thing to look at when nothing printed
//!
//! It prints the packet log as it goes, because when a game says it cannot
//! print, the first thing worth knowing is which packets it actually sent.

use gb_core::{Button, GameBoy, PrinterHandle, Sheet};
use std::fs::File;
use std::io::BufWriter;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut rom_path = None;
    let mut state = None;
    let mut keys = String::new();
    let mut frames = 600u32;
    let mut out = "print".to_string();
    let mut shot: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--state" => state = args.next(),
            "--keys" => keys = args.next().unwrap_or_default(),
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(600),
            "--out" => out = args.next().unwrap_or_else(|| "print".into()),
            "--shot" => shot = args.next(),
            other => rom_path = Some(other.to_string()),
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!("usage: gbprint <rom.gb> [--state f] [--keys s] [--frames n] [--out p]");
        std::process::exit(1);
    };

    let rom = std::fs::read(&rom_path).expect("failed to read ROM");
    let mut gb = GameBoy::new(rom);
    println!("Loaded: {}", gb.title());

    // The printer goes on before the state does. A state restore does not touch
    // the link, and a game that is already mid-conversation with a printer would
    // otherwise find nothing on the wire.
    let printer = PrinterHandle::new();
    gb.connect_link(Box::new(printer.clone()));

    if let Some(path) = &state {
        let data = std::fs::read(path).expect("failed to read save state");
        assert!(gb.load_state(&data), "the save state was refused");
        println!("State: {path}");
    }

    if !keys.is_empty() {
        play(&mut gb, &keys);
    }

    let mut seen = 0usize;
    for _ in 0..frames {
        gb.step_frame();
        let n = printer.log().len();
        if n != seen {
            for (cmd, len) in printer.log().into_iter().skip(seen) {
                println!("  packet {} len {len}", name_of(cmd));
            }
            seen = n;
        }
    }

    if let Some(path) = &shot {
        // The framebuffer is 0x00RRGGBB.
        let mut rgb = Vec::with_capacity(gb_core::SCREEN_W * gb_core::SCREEN_H * 3);
        for &px in gb.framebuffer() {
            rgb.push((px >> 16) as u8);
            rgb.push((px >> 8) as u8);
            rgb.push(px as u8);
        }
        let file = File::create(path).expect("create png");
        let mut e = png::Encoder::new(
            BufWriter::new(file),
            gb_core::SCREEN_W as u32,
            gb_core::SCREEN_H as u32,
        );
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .expect("png header")
            .write_image_data(&rgb)
            .expect("png data");
        println!("screen -> {path}");
    }

    let sheets = printer.take_sheets();
    if sheets.is_empty() {
        println!("\nNothing printed.");
        if seen == 0 {
            println!("The game never sent a packet, so it never tried.");
        }
        return;
    }

    println!("\n{} page(s):", sheets.len());
    for (i, s) in sheets.iter().enumerate() {
        println!(
            "  {i}: {}x{}  copies {}  margins {}/{}  palette ${:02X}  exposure ${:02X}",
            s.width, s.height, s.copies, s.margin_before, s.margin_after, s.palette, s.exposure
        );
        write_png(&format!("{out}-{i}.png"), s.width, s.height, &s.pixels);
    }

    // Real paper is continuous. Consecutive pages with no feed between them are
    // one picture, and saving them separately is what turns a Pokedex entry into
    // a pile of fragments, so the joined version is written too.
    let strips = stitch(&sheets);
    if strips.len() != sheets.len() {
        println!("\n{} strip(s) once the zero margins are joined:", strips.len());
        for (i, (h, px)) in strips.iter().enumerate() {
            println!("  {i}: 160x{h}");
            write_png(&format!("{out}-strip-{i}.png"), 160, *h, px);
        }
    }
}

fn name_of(cmd: u8) -> &'static str {
    match cmd {
        0x01 => "init  ",
        0x02 => "print ",
        0x04 => "data  ",
        0x08 => "break ",
        0x0F => "status",
        _ => "?     ",
    }
}

/// Join runs of pages that the printer was told not to feed paper between.
fn stitch(sheets: &[Sheet]) -> Vec<(usize, Vec<u8>)> {
    let mut out: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut joined_to_previous = false;
    for s in sheets {
        if joined_to_previous && s.margin_before == 0 {
            if let Some(last) = out.last_mut() {
                last.0 += s.height;
                last.1.extend_from_slice(&s.pixels);
                joined_to_previous = s.margin_after == 0;
                continue;
            }
        }
        out.push((s.height, s.pixels.clone()));
        joined_to_previous = s.margin_after == 0;
    }
    out
}

/// The printer's four shades, as the paper actually looks: no ink to full ink.
fn write_png(path: &str, w: usize, h: usize, pixels: &[u8]) {
    const INK: [[u8; 3]; 4] = [
        [0xFF, 0xFF, 0xFF],
        [0xA8, 0xA8, 0xA8],
        [0x54, 0x54, 0x54],
        [0x00, 0x00, 0x00],
    ];
    let mut rgb = Vec::with_capacity(w * h * 3);
    for &p in pixels {
        rgb.extend_from_slice(&INK[(p & 3) as usize]);
    }
    let file = File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgb)
        .expect("png data");
    println!("     wrote {path}");
}

/// The same input script `shot` understands.
fn play(gb: &mut GameBoy, script: &str) {
    for step in script.split(',') {
        let step = step.trim();
        if step.is_empty() {
            continue;
        }
        if let Some(n) = step.strip_prefix('w') {
            for _ in 0..n.parse::<u32>().unwrap_or(0) {
                gb.step_frame();
            }
            continue;
        }
        let (hold, release, name) = match step.strip_prefix('+') {
            Some(rest) => (true, false, rest),
            None => match step.strip_prefix('-') {
                Some(rest) => (false, true, rest),
                None => (false, false, step),
            },
        };
        let Some(b) = button(name) else {
            eprintln!("unknown key {name:?}");
            continue;
        };
        if hold {
            gb.set_button(b, true);
        } else if release {
            gb.set_button(b, false);
        } else {
            gb.set_button(b, true);
            for _ in 0..6 {
                gb.step_frame();
            }
            gb.set_button(b, false);
            for _ in 0..6 {
                gb.step_frame();
            }
        }
    }
}

fn button(name: &str) -> Option<Button> {
    Some(match name {
        "a" => Button::A,
        "b" => Button::B,
        "up" => Button::Up,
        "down" => Button::Down,
        "left" => Button::Left,
        "right" => Button::Right,
        "start" => Button::Start,
        "select" => Button::Select,
        _ => return None,
    })
}
