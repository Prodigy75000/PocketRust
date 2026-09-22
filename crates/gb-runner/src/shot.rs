//! Headless screenshot tool: run a ROM for N frames, then write the framebuffer
//! to a PNG. Handy for eyeballing the PPU and for regression baselines.
//!
//!   cargo run --release -p gb-runner --bin shot -- <rom.gb> <frames> <out.png> [keys]
//!
//! `keys` is an optional comma-separated script, played before the N frames, so
//! that a screenshot of something behind a menu can be taken again later without
//! anyone having to remember which buttons they pressed:
//!
//!   a b up down left right start select   tap it (six frames down, six up)
//!   +a                                    hold it down and leave it there
//!   -a                                    let it go
//!   w30                                   run thirty frames
//!
//!   ... -- demo.gbc 60 shot.png down,down,a,w120

use gb_core::{GameBoy, SCREEN_H, SCREEN_W};
use gb_runner::play;
use std::fs::File;
use std::io::BufWriter;

fn main() {
    let mut args = std::env::args().skip(1);
    let rom_path = args.next().expect("usage: shot <rom.gb> <frames> <out.png>");
    let frames: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(600);
    let out = args.next().unwrap_or_else(|| "shot.png".into());
    let keys = args.next().unwrap_or_default();
    // GBCAMERA=<file.png> points the Game Boy Camera's sensor at an image, so
    // the sensor model can be developed against a file instead of a phone.
    let camera = std::env::var("GBCAMERA").ok();

    let rom = std::fs::read(&rom_path).expect("failed to read ROM");
    let mut gb = GameBoy::new(rom);
    match std::env::var("GBCOLOR").as_deref() {
        Ok("auto") => gb.set_colorization(gb_core::Colorize::Auto),
        Ok("grayscale") => gb.set_colorization(gb_core::Colorize::Grayscale),
        _ => {}
    }
    println!("Loaded '{}', running {frames} frames...", gb.title());
    if let Some(path) = &camera {
        let frame = load_grayscale(path);
        assert!(
            gb.set_camera_frame(&frame),
            "this cartridge has no camera, or the frame is the wrong size"
        );
        println!("Camera: {path}");
    }

    if !keys.is_empty() {
        play(&mut gb, &keys);
    }

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

/// Load any PNG and squash it to the sensor's 128x112 greyscale.
///
/// Nearest-neighbour and a flat luminance average: this is a development hook,
/// not the real capture path, and a better resampler here would only hide how
/// the sensor model behaves on hard edges.
fn load_grayscale(path: &str) -> Vec<u8> {
    let decoder = png::Decoder::new(File::open(path).expect("open camera image"));
    let mut reader = decoder.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("png data");
    let (sw, sh) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();

    let mut out = vec![0u8; gb_core::CAMERA_W * gb_core::CAMERA_H];
    for y in 0..gb_core::CAMERA_H {
        for x in 0..gb_core::CAMERA_W {
            let sx = x * sw / gb_core::CAMERA_W;
            let sy = y * sh / gb_core::CAMERA_H;
            let at = (sy * sw + sx) * ch;
            let v = match ch {
                1 | 2 => buf[at] as u32,
                _ => (buf[at] as u32 + buf[at + 1] as u32 + buf[at + 2] as u32) / 3,
            };
            out[y * gb_core::CAMERA_W + x] = v as u8;
        }
    }
    out
}
