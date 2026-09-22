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

#[test]
fn the_viewfinder_shows_what_the_sensor_captured() {
    // The path this guards is the whole camera: trigger a capture, develop it
    // into cartridge RAM, have the game read it back and push it to the screen.
    //
    // It exists because of a specific bug. This mapper gates cartridge RAM
    // WRITES only; the published spec says reading is always enabled. Gating
    // reads as well made every read return $FF before the game enabled RAM,
    // which is both bitplanes set, which is colour 3, which is a BLACK
    // viewfinder. It looked exactly like "no camera attached", which is what
    // everyone including me assumed it was.
    let Some(mut gb) = boot() else { return };

    // Main menu, parlor, viewfinder: three presses with time to animate between.
    for _ in 0..3 {
        gb.set_button(gb_core::Button::A, true);
        for _ in 0..8 {
            gb.step_frame();
        }
        gb.set_button(gb_core::Button::A, false);
        for _ in 0..172 {
            gb.step_frame();
        }
    }
    let frame = gb.step_frame();

    // The viewfinder occupies the middle of the screen; the edges are the
    // brightness and contrast sliders, which draw whether or not a capture
    // worked. So only the middle is evidence.
    let mut shades = std::collections::HashSet::new();
    for y in 24..120 {
        for x in 24..120 {
            shades.insert(frame[y * 160 + x]);
        }
    }
    assert!(
        shades.len() > 1,
        "the viewfinder is one flat colour, so no capture reached the screen.          A single shade here is what a black viewfinder looks like, and it is          also what an unread capture looks like."
    );
}

/// Drive to the live viewfinder with the sensor pointed at `gray`.
fn viewfinder_with(gray: &[u8]) -> Option<Vec<u32>> {
    let mut gb = boot()?;
    assert!(gb.set_camera_frame(gray), "the cartridge refused a frame");
    for _ in 0..3 {
        gb.set_button(gb_core::Button::A, true);
        for _ in 0..8 {
            gb.step_frame();
        }
        gb.set_button(gb_core::Button::A, false);
        for _ in 0..172 {
            gb.step_frame();
        }
    }
    Some(gb.step_frame().to_vec())
}

#[test]
fn what_the_sensor_sees_reaches_the_viewfinder() {
    let n = gb_core::CAMERA_W * gb_core::CAMERA_H;
    let Some(dark) = viewfinder_with(&vec![0u8; n]) else {
        return;
    };
    let bright = viewfinder_with(&vec![255u8; n]).unwrap();

    // Compare the middle only. The sliders at the edges draw whether or not a
    // capture worked, so they are not evidence.
    let mid = |f: &[u32]| -> Vec<u32> {
        (24..120)
            .flat_map(|y| (24..120).map(move |x| (y, x)))
            .map(|(y, x)| f[y * 160 + x])
            .collect()
    };
    assert_ne!(
        mid(&dark),
        mid(&bright),
        "pointing the sensor at black and at white produced the same picture,          so the frame is not reaching the develop pipeline"
    );
}

#[test]
fn a_frame_of_the_wrong_size_is_refused() {
    let Some(mut gb) = boot() else { return };
    assert!(gb.has_camera(), "this cartridge should have a sensor");
    assert!(
        !gb.set_camera_frame(&[0u8; 10]),
        "a short frame must be refused rather than read past its end"
    );
    assert!(gb.set_camera_frame(&vec![0u8; gb_core::CAMERA_W * gb_core::CAMERA_H]));
}
