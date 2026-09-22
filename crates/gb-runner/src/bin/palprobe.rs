//! Diagnostic: SGB border palettes, tilemap, and any holes in the artwork.
//!
//! Written to chase black rectangles in Pokemon Blue's border, and it found
//! two separate causes. The palette dump showed only three of the four
//! palettes were being decoded. The hole list showed the remaining gaps sat at
//! tile columns 0, 8, 16 and 24 of the top five rows, which is every 16th byte
//! of the map data; 16 bytes is one tile of the 4 KiB capture and its first
//! two bytes are row 0, so the real fault was a scanline rather than anything
//! in this decoder.
//!
//! `CMDLOG=1` prints the cartridge's whole SGB command sequence instead. That
//! is what showed the mask being raised before the transfers and lifted by a
//! flag bit inside PAL_SET rather than by a MASK_EN of its own.
//!
//!   cargo run -p gb-runner --bin palprobe -- <rom>
//!   CMDLOG=1 cargo run -p gb-runner --bin palprobe -- <rom>
use gb_core::GameBoy;

fn main() {
    let rom = std::env::args().nth(1).expect("usage: palprobe <rom>");
    let mut gb = GameBoy::new(std::fs::read(&rom).unwrap());
    gb.set_sgb(true);
    for _ in 0..1400 {
        gb.step_frame();
    }
    for (i, pal) in gb.sgb_border_palettes().iter().enumerate() {
        print!("pal {}: ", i + 4);
        for c in pal {
            print!("{:06X} ", c & 0xFFFFFF);
        }
        println!();
    }
    if std::env::var("CMDLOG").is_ok() {
        let (log, active, _) = gb.sgb_debug();
        println!("active={active} commands={}", log.len());
        let names = |c: u8| match c {
            0x00 => "PAL01", 0x01 => "PAL23", 0x02 => "PAL03", 0x03 => "PAL12",
            0x04 => "ATTR_BLK", 0x05 => "ATTR_LIN", 0x06 => "ATTR_DIV",
            0x07 => "ATTR_CHR", 0x0A => "PAL_SET", 0x0B => "PAL_TRN",
            0x11 => "MLT_REQ", 0x13 => "CHR_TRN", 0x14 => "PCT_TRN",
            0x15 => "ATTR_TRN", 0x16 => "ATTR_SET", 0x17 => "MASK_EN",
            0x19 => "PAL_PRI", _ => "?",
        };
        for (i, (c, n)) in log.iter().enumerate() {
            println!("  {i:3} {c:02X} {:10} {n} bytes", names(*c));
        }
        return;
    }
    let border = gb.sgb_border().expect("needs a border");
    let map = gb.sgb_border_map();

    // A tile that draws nothing at all, outside the centre window where that
    // is expected. Each one is a hole in the artwork.
    println!("blank tiles outside the screen window:");
    for ty in 0..28usize {
        for tx in 0..32usize {
            let centre = (6..26).contains(&tx) && (5..23).contains(&ty);
            if centre {
                continue;
            }
            let opaque = (0..8)
                .flat_map(|r| (0..8).map(move |c| (r, c)))
                .filter(|(r, c)| border[(ty * 8 + r) * 256 + tx * 8 + c].is_some())
                .count();
            if opaque == 0 {
                let e = map[ty * 32 + tx];
                println!(
                    "  tile ({tx:2},{ty:2}) map={e:04X} index={:3} pal={} flips={}{}",
                    e & 0xFF,
                    (e >> 10) & 7,
                    if e & 0x4000 != 0 { 'X' } else { '-' },
                    if e & 0x8000 != 0 { 'Y' } else { '-' },
                );
            }
        }
    }
}
