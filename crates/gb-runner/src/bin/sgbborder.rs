//! Dump a cartridge's SGB frame, border composed over the screen, as a PNG.
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
    let solid = border.iter().filter(|p| p.is_some()).count();

    // The composed 256x224 frame: border art over the live screen, which is
    // what the display path actually presents. The raw decode above is only
    // consulted for the count, so that a run still reports how much of the
    // picture is artwork rather than passthrough.
    let mut out_px = vec![0u32; 256 * 224];
    assert!(gb.sgb_compose(&mut out_px), "border decoded but compose declined");
    let mut rgb = Vec::with_capacity(256 * 224 * 3);
    for c in &out_px {
        rgb.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, *c as u8]);
    }
    let w = &mut BufWriter::new(File::create(&out).unwrap());
    let mut enc = png::Encoder::new(w, 256, 224);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!(
        "wrote {out}: {solid} of {} pixels are border art, the rest is screen or backdrop",
        border.len()
    );
}
