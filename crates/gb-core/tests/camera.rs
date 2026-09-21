// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The Game Boy Camera's mapper, driven by the cartridge itself.
//!
//! Unlike the printer, the Camera cannot be tested without its ROM: the Camera
//! *is* the ROM. Cart type `$FC` is a 1 MB image with a mapper, 128 KB of
//! battery RAM for the photo album, and an M64282FP sensor on the cartridge
//! bus. So this test **skips when the ROM is absent**; put it at
//!
//! ```text
//!   dumps/roms/Game Boy Camera (USA, Europe) (SGB Enhanced).gb
//! ```
//!
//! `dumps/` is git-ignored.
//!
//! What this guards is the difference between booting and not, because the way
//! this mapper fails is silent. Before it existed, `$FC` fell through to
//! `Unsupported`, was treated as no-mapper, and a 1 MB ROM read as 32 KB gave a
//! blank screen. The first attempt at the mapper *also* gave a blank screen, for
//! a completely different reason, and the two are indistinguishable by eye.

use gb_core::GameBoy;
use std::path::PathBuf;

const ROM: &str = "dumps/roms/Game Boy Camera (USA, Europe) (SGB Enhanced).gb";

fn boot() -> Option<GameBoy> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(ROM);
    if !path.exists() {
        eprintln!("skipping: needs {ROM}");
        return None;
    }
    let mut gb = GameBoy::new(std::fs::read(&path).ok()?);
    for _ in 0..900 {
        gb.step_frame();
    }
    Some(gb)
}

#[test]
fn the_camera_cartridge_boots() {
    let Some(gb) = boot() else { return };
    let (pc, lcdc, _ly, _halted) = gb.debug_state();

    // The screen being ON is the whole claim. A cartridge whose mapper is wrong
    // never gets far enough to turn it on.
    assert!(
        lcdc & 0x80 != 0,
        "the LCD is off after 900 frames (LCDC ${lcdc:02X}, PC ${pc:04X}). The \
         cartridge is not running."
    );

    // And it is executing from somewhere sane rather than having fallen into
    // unmapped space. The first attempt sat at $4B85 forever, polling a busy
    // bit that could never clear.
    assert!(
        pc < 0x8000,
        "PC ${pc:04X} is outside the cartridge, so execution ran away"
    );
}

#[test]
fn the_title_screen_actually_draws() {
    // "The LCD is on" is not the same as "there is a picture". A mapper that
    // gets far enough to enable the screen and then feeds it nothing produces a
    // uniform frame, which is exactly what the broken build looked like.
    let Some(mut gb) = boot() else { return };
    let frame = gb.step_frame();

    let first = frame[0];
    assert!(
        frame.iter().any(|&p| p != first),
        "every pixel on screen is the same colour, so nothing was drawn"
    );

    // A title screen is more than two shades of one thing. Counting distinct
    // colours separates "drew a picture" from "drew a flat panel with a border".
    let mut seen: Vec<u32> = Vec::new();
    for &p in frame.iter() {
        if !seen.contains(&p) {
            seen.push(p);
            if seen.len() > 3 {
                break;
            }
        }
    }
    assert!(
        seen.len() > 3,
        "only {} distinct colours on screen; that is not a title screen",
        seen.len()
    );
}
