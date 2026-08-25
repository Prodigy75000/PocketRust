// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The core, driven by the demo cartridge in `roms/pocketrust-demo/`.
//!
//! Every other integration test here needs a ROM the person running it has to
//! supply. This one does not: the cartridge is ours, it is checked in, and it
//! is dedicated to the public domain, so these tests run on a fresh clone
//! anywhere.
//!
//! What they measure is deliberately not "the screen looks right". They pick
//! the four things on that cartridge that are hard for an emulator and easy to
//! state exactly: the ten-objects-per-scanline limit, a raster split that has
//! to land between two named scanlines, a save state that has to replay, and an
//! audio channel that has to fall silent rather than merely quiet.

use gb_core::{Button, GameBoy};

const SCREEN_W: usize = 160;

fn rom() -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("roms/pocketrust-demo/pocketrust-demo.gbc");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn boot() -> GameBoy {
    let mut gb = GameBoy::new(rom());
    // Reset clears 8 KB of work RAM and copies the tile set, which takes longer
    // than one frame; by 120 the menu is up and settled.
    run(&mut gb, 120);
    gb
}

fn run(gb: &mut GameBoy, frames: usize) {
    for _ in 0..frames {
        gb.step_frame();
    }
}

fn tap(gb: &mut GameBoy, b: Button) {
    gb.set_button(b, true);
    run(gb, 6);
    gb.set_button(b, false);
    run(gb, 6);
}

/// Walk from the menu into screen n (1 to 5).
fn enter_screen(gb: &mut GameBoy, n: usize) {
    for _ in 0..(n - 1) {
        tap(gb, Button::Down);
    }
    tap(gb, Button::A);
    run(gb, 20);
}

/// The background map as the twenty by eighteen characters actually on screen.
/// Tile indices below $40 are the font, and the font is laid out so that a tile
/// index is its character code minus $20.
fn screen_text(gb: &GameBoy) -> Vec<String> {
    let vram = gb.vram();
    (0..18)
        .map(|row| {
            (0..20)
                .map(|col| {
                    let t = vram[0x1800 + row * 32 + col];
                    if t < 0x40 {
                        (t + 0x20) as char
                    } else {
                        '#'
                    }
                })
                .collect()
        })
        .collect()
}

fn scanline(gb: &GameBoy, y: usize) -> &[u32] {
    &gb.framebuffer()[y * SCREEN_W..(y + 1) * SCREEN_W]
}

#[test]
fn it_boots_to_its_menu_and_names_the_machine_it_is_running_on() {
    let gb = boot();
    let text = screen_text(&gb);
    assert_eq!(text[1].trim(), "POCKETRUST");
    assert_eq!(text[6].trim(), "1 SPRITES");
    assert_eq!(text[10].trim(), "5 INPUT");
    // The cartridge asks the hardware which machine this is, so this line is
    // the core's answer rather than the cartridge's.
    assert_eq!(text[17].trim(), "MODE: CGB");
    assert!(gb.is_cgb());
}

#[test]
fn a_monochrome_game_boy_gets_the_monochrome_half_of_the_cartridge() {
    // The cartridge is marked $80 at $0143: colour where there is colour, still
    // a Game Boy game where there is not. Clearing that byte is exactly what
    // makes a real Game Boy Color run it in monochrome mode, so it is how the
    // other half of the cartridge is reachable from here.
    let mut rom = rom();
    rom[0x0143] = 0x00;
    let mut x: u8 = 0;
    for &b in &rom[0x0134..=0x014c] {
        x = x.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x014d] = x;

    let mut gb = GameBoy::new(rom);
    assert!(!gb.is_cgb());
    run(&mut gb, 120);
    assert_eq!(screen_text(&gb)[17].trim(), "MODE: DMG");

    // Its colour screen shows the four shades rather than seven palettes.
    enter_screen(&mut gb, 3);
    let text = screen_text(&gb);
    assert_eq!(text[0].trim(), "3 COLORS");
    // Four blocks on rows 4 to 6, and their labels on row 7.
    assert_eq!(text[7].trim(), "0    1    2    3");
}

#[test]
fn every_screen_is_reachable_and_b_comes_back_to_a_clean_menu() {
    // The menu that comes back has to be the menu that booted, character for
    // character. Each screen is drawn over the last one with the screen off,
    // and a screen that draws less than the one before it leaves the
    // difference behind unless the map is cleared first.
    let clean = screen_text(&boot());
    let titles = ["1 SPRITES", "2 SCROLL SPLIT", "3 COLORS", "4 AUDIO", "5 INPUT"];
    for (i, title) in titles.iter().enumerate() {
        let mut gb = boot();
        enter_screen(&mut gb, i + 1);
        let heading = screen_text(&gb)[0].trim().to_string();
        assert!(
            heading.starts_with(title),
            "menu entry {} opened {heading:?} instead of {title:?}",
            i + 1
        );
        tap(&mut gb, Button::B);
        run(&mut gb, 20);
        let back = screen_text(&gb);
        for row in 0..18 {
            assert_eq!(
                back[row],
                clean[row],
                "menu row {row} came back dirty from screen {}",
                i + 1
            );
        }
    }
}

#[test]
fn the_object_limit_drops_everything_past_the_tenth_on_a_line() {
    // The second arrangement puts twenty objects on one scanline, eight pixels
    // apart from x=4. The hardware draws ten of them and drops the rest, and
    // which ten is decided by position in the object buffer, not by x, so the
    // ten that survive are the ten on the left.
    let mut gb = boot();
    enter_screen(&mut gb, 1);
    tap(&mut gb, Button::A); // rings -> grid
    run(&mut gb, 10);
    assert!(screen_text(&gb)[1].contains("GRID"), "the grid arrangement is not up");

    // The background on this screen is blank, so any pixel that is not the
    // background colour on this line came from an object.
    let line = scanline(&gb, 64);
    let background = line[159];
    let drawn = |from: usize, to: usize| line[from..to].iter().filter(|p| **p != background).count();

    // Ten objects, eight pixels wide, laid end to end from x=4: eighty pixels,
    // exactly, and this row of the tile has no transparent pixel in it.
    assert_eq!(
        drawn(4, 84),
        80,
        "the ten objects that should be drawn are not all there"
    );
    assert_eq!(
        drawn(84, 160),
        0,
        "objects past the tenth on this line were drawn; the per-line limit is not being applied"
    );
}

#[test]
fn the_raster_split_holds_the_status_bar_still_while_the_playfield_moves() {
    let mut gb = boot();
    enter_screen(&mut gb, 2);

    // Scanline 31 is the last line of the status bar and scanline 32 is the
    // first line of the sky, so the two must not be the same colour and each
    // must be flat: that is what puts the seam between them and nowhere else.
    let bar_line: Vec<u32> = scanline(&gb, 31).to_vec();
    let sky_line: Vec<u32> = scanline(&gb, 32).to_vec();
    assert!(bar_line.windows(2).all(|w| w[0] == w[1]), "scanline 31 is not flat");
    assert!(sky_line.windows(2).all(|w| w[0] == w[1]), "scanline 32 is not flat");
    assert_ne!(bar_line[0], sky_line[0], "the bar and the sky look the same");

    // The bar rows the scroll must not reach. Map rows 1 and 2 carry readouts
    // that change with the speed, so the witness is map row 0: static text,
    // which shifts visibly if the scroll ever reaches it.
    const BAR: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
    let still_before: Vec<Vec<u32>> = BAR.iter().map(|y| scanline(&gb, *y).to_vec()).collect();
    let brick_before: Vec<u32> = scanline(&gb, 100).to_vec();

    for _ in 0..3 {
        tap(&mut gb, Button::Right); // speed +3
    }
    run(&mut gb, 37); // an odd number of frames, so the scroll is not a whole tile

    assert_ne!(
        scanline(&gb, 100).to_vec(),
        brick_before,
        "the playfield did not scroll at all, so nothing here is being tested"
    );
    for (i, y) in BAR.iter().enumerate() {
        assert_eq!(
            scanline(&gb, *y).to_vec(),
            still_before[i],
            "scanline {y} moved with the playfield; the split leaked into the status bar"
        );
    }

    // ...and with the split switched off the bar has to move, or the test above
    // would pass on a core that simply never scrolled anything.
    tap(&mut gb, Button::A);
    run(&mut gb, 20);
    assert_ne!(
        scanline(&gb, 4).to_vec(),
        still_before[4],
        "with the split off the whole screen should scroll, including the bar"
    );
}

#[test]
fn two_instances_of_the_same_cartridge_stay_identical() {
    let mut a = GameBoy::new(rom());
    let mut b = GameBoy::new(rom());
    for _ in 0..400 {
        a.step_frame();
        b.step_frame();
    }
    assert_eq!(a.framebuffer(), b.framebuffer(), "the video diverged");
    assert_eq!(a.wram(), b.wram(), "work RAM diverged");
    assert_eq!(a.take_audio(), b.take_audio(), "the audio diverged");
}

#[test]
fn a_save_state_round_trips_and_then_replays_identically() {
    let mut gb = boot();
    enter_screen(&mut gb, 1);
    run(&mut gb, 33);

    // Drain first, so both machines start counting audio from the same point.
    gb.take_audio();
    let state = gb.save_state();

    // What the original does next, from here.
    let mut reference = Vec::new();
    for _ in 0..90 {
        gb.step_frame();
        reference.push(gb.framebuffer().to_vec());
    }
    let audio_after = gb.take_audio();

    let mut restored = GameBoy::new(rom());
    assert!(restored.load_state(&state), "the state was refused");
    restored.take_audio();
    for (i, want) in reference.iter().enumerate() {
        restored.step_frame();
        assert_eq!(
            restored.framebuffer(),
            want.as_slice(),
            "frame {i} after the restore does not match the original"
        );
    }
    assert_eq!(restored.take_audio(), audio_after, "the audio did not replay");
}

#[test]
fn a_state_that_is_not_a_state_is_refused_rather_than_misread() {
    let mut gb = boot();
    let good = gb.save_state();

    assert!(!gb.load_state(&[]), "an empty blob was accepted");
    assert!(!gb.load_state(b"PRGB2"), "a header with no body was accepted");
    assert!(
        !gb.load_state(&good[..good.len() / 2]),
        "a truncated state was accepted"
    );
    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    assert!(!gb.load_state(&wrong_magic), "a foreign state was accepted");

    // ...and after all that refusing, the machine is still the one it was.
    assert!(gb.load_state(&good), "a good state was refused");
}

#[test]
fn the_audio_screen_makes_sound_and_stops_making_it_on_the_way_out() {
    let mut gb = boot();
    enter_screen(&mut gb, 4);
    run(&mut gb, 40);
    gb.take_audio();
    run(&mut gb, 40);
    let playing = gb.take_audio();
    assert!(!playing.is_empty(), "no audio was produced at all");
    let peak = playing.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 200, "the pattern is playing but is nearly silent (peak {peak})");

    // Leaving powers the sound hardware down. Silence has to be a run of zeroes
    // and not a held level: a direct-current offset is inaudible right up until
    // the stream stops, and then it is a click.
    tap(&mut gb, Button::B);
    run(&mut gb, 30);
    gb.take_audio();
    run(&mut gb, 30);
    let silence = gb.take_audio();
    assert!(!silence.is_empty());
    assert!(
        silence.iter().all(|s| *s == 0),
        "the menu is not silent: peak {}",
        silence.iter().map(|s| s.unsigned_abs()).max().unwrap()
    );
}

#[test]
fn muting_a_channel_takes_it_out_of_the_mix() {
    let mut gb = boot();
    enter_screen(&mut gb, 4);
    run(&mut gb, 40);
    gb.take_audio();
    run(&mut gb, 60);
    let full = gb.take_audio();
    let energy = |s: &[i16]| s.iter().map(|v| (*v as i64).abs()).sum::<i64>();

    // Mute all four, one at a time.
    for _ in 0..4 {
        tap(&mut gb, Button::A);
        tap(&mut gb, Button::Down);
    }
    let text = screen_text(&gb);
    for (i, row) in [4usize, 6, 8, 10].iter().enumerate() {
        assert!(
            text[*row].contains("MUTE"),
            "channel {} did not mute: {:?}",
            i + 1,
            text[*row]
        );
    }
    run(&mut gb, 30);
    gb.take_audio();
    run(&mut gb, 60);
    let muted = gb.take_audio();
    assert!(energy(&full) > 0, "nothing was playing to begin with");
    assert_eq!(
        energy(&muted),
        0,
        "a channel whose envelope byte is zero is off, not quiet"
    );
}
