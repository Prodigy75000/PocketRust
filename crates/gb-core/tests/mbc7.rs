// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! MBC7 against the real cartridge.
//!
//! Skipped when the ROM is absent, since a commercial cartridge cannot be
//! committed. Put it at `dumps/roms/Kirby - Tilt 'n' Tumble (USA).gbc`.
//!
//! These drive the game through its own calibration screen and into a level,
//! because that path is what actually exercises the mapper: the EEPROM is read
//! at the file select and the accelerometer is latched about once a frame in
//! play. A test that only booted would have passed throughout the three bugs
//! found while writing this.

use gb_core::GameBoy;
use std::path::PathBuf;

const ROM: &str = "dumps/roms/Kirby - Tilt 'n' Tumble (USA).gbc";

/// Calibration screen, past it, title, file select, file 1, into level 1-1.
const TO_GAMEPLAY: &str = "w600,a,w120,a,w500,start,w200,start,w200,a,w400,a,w300,a,w300,w600";

fn rom() -> Option<Vec<u8>> {
    // Relative to the MANIFEST, not the cwd. Cargo runs a test with the cwd at
    // the crate root, so a bare relative path resolves under crates/gb-core,
    // finds nothing, and every test in this file skips while reporting pass.
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
    std::fs::read(&path).ok()
}

/// Minimal script player: this crate cannot depend on gb-runner.
fn play(gb: &mut GameBoy, script: &str) {
    use gb_core::Button;
    for token in script.split(',') {
        if let Some(n) = token.strip_prefix('w') {
            for _ in 0..n.parse::<u32>().unwrap() {
                gb.step_frame();
            }
            continue;
        }
        let b = match token {
            "a" => Button::A,
            "start" => Button::Start,
            other => panic!("no such button {other:?}"),
        };
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

fn scroll(gb: &GameBoy) -> (u8, u8) {
    (gb.peek(0xFF43), gb.peek(0xFF42))
}

#[test]
fn cartridge_has_a_tilt_sensor() {
    let Some(rom) = rom() else { return };
    let gb = GameBoy::new(rom);
    assert!(gb.has_tilt(), "cart type $22 must report a tilt sensor");
    assert!(!gb.has_camera(), "and must not be confused with the Camera");
}

#[test]
fn eeprom_is_the_save_and_is_256_bytes() {
    let Some(rom) = rom() else { return };
    let gb = GameBoy::new(rom);
    // The header declares NO cartridge RAM; the save is the 93LC56. Sized
    // exactly, because this is the file the frontend writes and every other
    // emulator of this mapper produces 256 bytes.
    assert_eq!(gb.sram().len(), 256);
    assert!(gb.has_battery());
}

#[test]
fn the_game_can_tell_a_blank_eeprom_from_a_written_one() {
    let Some(rom) = rom() else { return };
    let to_file_select = "w600,a,w120,a,w500,start,w200,start,w200";
    let shot = |fill: u8| {
        let mut gb = GameBoy::new(rom.clone());
        gb.load_sram(&vec![fill; 256]);
        play(&mut gb, to_file_select);
        gb.framebuffer().to_vec()
    };
    // Not a claim about which fill is right, but that the game READS the
    // EEPROM: its file select must render differently for two different saves.
    // If the read path were dead, both would be the same screen and every
    // other EEPROM test here could still pass.
    assert_ne!(
        shot(0x00),
        shot(0xFF),
        "the file select must reflect what is in the EEPROM"
    );
}

#[test]
fn a_fresh_cartridge_reads_as_empty_rather_than_as_data() {
    let Some(rom) = rom() else { return };
    let gb = GameBoy::new(rom);
    // Measured, not assumed. An erased 93LC56 reads $FF, which looks like the
    // honest hardware answer; filled with $FF this game shows three fabricated
    // "LEVEL 8-4 255%" saves and does not offer to format them, while $00
    // gives "NO DATA". So zero is what a new cartridge must look like, and
    // invented 100%-complete progress is exactly what false-unlocks cheevos.
    assert!(gb.sram().iter().all(|&b| b == 0x00));
}

#[test]
fn tilting_right_and_left_rolls_the_ball_opposite_ways() {
    let Some(rom) = rom() else { return };
    let run = |x: f32| {
        let mut gb = GameBoy::new(rom.clone());
        play(&mut gb, TO_GAMEPLAY);
        gb.set_tilt(x, 0.0);
        for _ in 0..240 {
            gb.step_frame();
        }
        scroll(&gb)
    };
    let flat = run(0.0);
    let right = run(1.0);
    let left = run(-1.0);

    assert_ne!(right, flat, "tilting right must move the ball");
    assert_ne!(left, flat, "tilting left must move the ball");
    // Direction, not just movement. The sign convention here is screen
    // coordinates, and the hardware register's own convention is the opposite
    // one, so a core that plays perfectly backwards is a real and easy outcome.
    assert!(
        right.0 > flat.0,
        "positive x must scroll RIGHT: flat {flat:?}, right {right:?}"
    );
    assert!(
        left.0 < flat.0,
        "negative x must scroll LEFT: flat {flat:?}, left {left:?}"
    );
}

#[test]
fn the_accelerometer_must_be_latched_before_it_reads_anything() {
    let Some(rom) = rom() else { return };
    let mut gb = GameBoy::new(rom);
    // Enable both halves of the RAM enable, which MBC7 needs together.
    gb.poke(0x0000, 0x0A);
    gb.poke(0x4000, 0x40);
    gb.set_tilt(1.0, -1.0);
    // Read without latching: must be the erased value $8000, NOT the resting
    // $81D0 and certainly not the tilt. The two differ by 464 counts, which is
    // four g, so confusing them leaves a mapper that works and a game that
    // never responds.
    assert_eq!(gb.peek(0xA020), 0x00);
    assert_eq!(gb.peek(0xA030), 0x80);

    gb.poke(0xA000, 0x55);
    gb.poke(0xA010, 0xAA);
    let x = u16::from(gb.peek(0xA020)) | u16::from(gb.peek(0xA030)) << 8;
    let y = u16::from(gb.peek(0xA040)) | u16::from(gb.peek(0xA050)) << 8;
    assert_eq!(x, 0x81D0 - 0x70);
    assert_eq!(y, 0x81D0 + 0x70);
}

#[test]
fn the_register_block_needs_both_ram_enables() {
    let Some(rom) = rom() else { return };
    let mut gb = GameBoy::new(rom);
    gb.poke(0x0000, 0x0A); // only the first
    assert_eq!(gb.peek(0xA030), 0xFF, "one enable alone must not open it");
    gb.poke(0x4000, 0x40); // now both
    assert_eq!(gb.peek(0xA030), 0x80);
}

#[test]
fn a_save_state_carries_the_mapper_and_replays_identically() {
    let Some(rom) = rom() else { return };
    let mut gb = GameBoy::new(rom.clone());
    play(&mut gb, TO_GAMEPLAY);
    gb.set_tilt(0.8, -0.4);
    for _ in 0..120 {
        gb.step_frame();
    }
    let state = gb.save_state();

    // Carry on from here, and from a restore of the same moment. Both must
    // land in the same place: the accelerometer's latched values and the
    // EEPROM's bit-level position are mapper state, and a state taken midway
    // through an EEPROM word has to come back midway through that word or the
    // game's next clock edge lands somewhere else entirely.
    for _ in 0..120 {
        gb.step_frame();
    }
    let expected = gb.save_state();

    let mut other = GameBoy::new(rom);
    assert!(other.load_state(&state), "state must load");
    other.set_tilt(0.8, -0.4);
    for _ in 0..120 {
        other.step_frame();
    }
    assert_eq!(
        other.save_state(),
        expected,
        "a restored MBC7 state must replay identically"
    );
}

#[test]
fn a_save_state_restores_the_latched_tilt_rather_than_the_live_one() {
    let Some(rom) = rom() else { return };
    let mut gb = GameBoy::new(rom);
    gb.poke(0x0000, 0x0A);
    gb.poke(0x4000, 0x40);
    gb.set_tilt(1.0, 0.0);
    gb.poke(0xA000, 0x55);
    gb.poke(0xA010, 0xAA);
    let state = gb.save_state();

    // Latch something else, then restore. The reading the game can see must be
    // the one that was latched when the state was taken.
    gb.set_tilt(-1.0, 0.0);
    gb.poke(0xA000, 0x55);
    gb.poke(0xA010, 0xAA);
    assert_eq!(gb.peek(0xA030), 0x82);

    assert!(gb.load_state(&state));
    let x = u16::from(gb.peek(0xA020)) | u16::from(gb.peek(0xA030)) << 8;
    assert_eq!(x, 0x81D0 - 0x70, "the latched tilt must come back with it");
}
