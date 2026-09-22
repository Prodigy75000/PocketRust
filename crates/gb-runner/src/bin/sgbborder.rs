//! Dump the SGB border a cartridge transfers, as a PNG. Diagnostic only:
//! nothing in the core displays this.
use gb_core::GameBoy;
use std::fs::File;
use std::io::BufWriter;

fn main() {
    let mut a = std::env::args().skip(1);
    let rom_path = a.next().expect("usage: sgbborder <rom> <out.png> [frames]");
    let out = a.next().expect("need an output path");
    let frames: u32 = a.next().and_then(|v| v.parse().ok()).unwrap_or(1200);

    let mut gb = GameBoy::new(std::fs::read(&rom_path).unwrap());
    gb.set_sgb(true);
    for _ in 0..frames {
        gb.step_frame();
    }
    let Some(border) = gb.sgb_border() else {
        println!("no border transferred in {frames} frames");
        std::process::exit(1);
    };

    // Transparent pixels are where the Game Boy screen shows through. Drawn as
    // magenta so the hole is obvious rather than looking like black artwork.
    let mut rgb = Vec::with_capacity(256 * 224 * 3);
    let mut solid = 0usize;
    for p in &border {
        match p {
            Some(c) => {
                solid += 1;
                rgb.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, *c as u8]);
            }
            None => rgb.extend_from_slice(&[255, 0, 255]),
        }
    }
    let w = &mut BufWriter::new(File::create(&out).unwrap());
    let mut enc = png::Encoder::new(w, 256, 224);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!(
        "wrote {out}: {solid} of {} pixels are border art, the rest is the hole",
        border.len()
    );
}
