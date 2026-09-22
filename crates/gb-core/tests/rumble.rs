// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! MBC5 rumble. Cart types $1C, $1D and $1E.
//!
//! The motor is bit 3 of the RAM bank register at $4000-$5FFF, and on these
//! cartridges that bit stops being an address line. That is the whole feature
//! and also its one hazard: the same write means different things on two
//! cartridges with the same mapper, so the header flag is load bearing.
//!
//! # Why these build their own cartridges
//!
//! The first version of this file tested against Pokemon Pinball and Pokemon
//! Yellow, and it was worthless in two separate ways that each looked fine.
//!
//! The Yellow path was wrong by a few words of filename, so every test using it
//! skipped while reporting pass. And Pokemon Pinball declares **one** 8 KiB RAM
//! bank, so `bank % banks` folds every bank onto bank 0 and no aliasing test
//! can observe anything on it. Deliberately breaking the mapper to treat all
//! MBC5 carts as rumble carts passed all five tests.
//!
//! A synthetic cartridge has neither problem: it always exists, and it can be
//! given as much RAM as the test needs. The real cartridge is still worth one
//! test, but only for the question a real header answers.

use gb_core::GameBoy;
use std::path::PathBuf;

/// A minimal but valid cartridge: 32 KiB of ROM, the given cart type, and the
/// given RAM size code. Nothing executes; these tests drive the mapper directly.
fn cart(cart_type: u8, ram_code: u8) -> GameBoy {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = cart_type;
    rom[0x0148] = 0x00; // 32 KiB, 2 ROM banks
    rom[0x0149] = ram_code;
    GameBoy::new(rom)
}

/// $1E is MBC5 + RAM + BATTERY + RUMBLE. $04 is 128 KiB, sixteen banks, which
/// is what makes bank 0 and bank 8 distinct memory and the aliasing test real.
fn rumble_cart() -> GameBoy {
    cart(0x1E, 0x04)
}

/// $1B is MBC5 + RAM + BATTERY, no motor. Same RAM so the two are comparable.
fn plain_mbc5() -> GameBoy {
    cart(0x1B, 0x04)
}

#[test]
fn the_header_decides_which_mbc5_carts_have_a_motor() {
    for t in [0x1C, 0x1D, 0x1E] {
        assert!(cart(t, 0x04).has_rumble(), "cart type ${t:02X} has a motor");
    }
    // The pair that matters. "Is it MBC5" and "does it rumble" are different
    // questions, and answering the second with the first would buzz through
    // every Pokemon battle and steal a RAM bank bit from a cartridge that
    // needs it.
    for t in [0x19, 0x1A, 0x1B] {
        assert!(
            !cart(t, 0x04).has_rumble(),
            "cart type ${t:02X} is MBC5 with no motor"
        );
    }
}

#[test]
fn the_motor_follows_bit_3_of_the_ram_bank_register() {
    let mut gb = rumble_cart();
    assert!(!gb.rumble(), "a cartridge starts with the motor off");

    gb.poke(0x4000, 0x08);
    assert!(gb.rumble(), "bit 3 set must start the motor");

    // It latches: one speed, no duty cycle, spins until the game clears it.
    gb.poke(0x4000, 0x08);
    assert!(gb.rumble());

    gb.poke(0x4000, 0x00);
    assert!(!gb.rumble(), "clearing bit 3 must stop it");
}

#[test]
fn bit_3_is_not_part_of_the_bank_number_on_a_rumble_cart() {
    let mut gb = rumble_cart();
    gb.poke(0x0000, 0x0A); // enable RAM
    gb.poke(0x4000, 0x00); // bank 0, motor off
    gb.poke(0xA000, 0x5A);
    assert_eq!(gb.peek(0xA000), 0x5A);

    // Same bank, motor ON. If bit 3 were still an address line this would
    // switch to bank 8, and on a real four-bank cartridge that is a save
    // scribbling on itself every time the ball hits a bumper.
    gb.poke(0x4000, 0x08);
    assert!(gb.rumble());
    assert_eq!(
        gb.peek(0xA000),
        0x5A,
        "turning the motor on must not switch RAM banks"
    );

    gb.poke(0xA000, 0x99);
    gb.poke(0x4000, 0x00);
    assert_eq!(
        gb.peek(0xA000),
        0x99,
        "and writes made while buzzing must land in the same bank"
    );
}

#[test]
fn an_ordinary_mbc5_still_gets_all_sixteen_banks() {
    // Bit 3 is an ordinary address line here, so $08 selects bank 8 and must
    // NOT be swallowed as a motor bit. Masking rumble carts and plain ones the
    // same way is the obvious shortcut, and it silently halves the RAM of every
    // non-rumble MBC5 game.
    let mut gb = plain_mbc5();
    gb.poke(0x0000, 0x0A);
    gb.poke(0x4000, 0x00);
    gb.poke(0xA000, 0x11);
    gb.poke(0x4000, 0x08);
    gb.poke(0xA000, 0x22);
    gb.poke(0x4000, 0x00);
    assert_eq!(
        gb.peek(0xA000),
        0x11,
        "bank 0 and bank 8 must be different memory on a non-rumble MBC5"
    );
    gb.poke(0x4000, 0x08);
    assert_eq!(gb.peek(0xA000), 0x22);
    assert!(!gb.rumble(), "and none of that may start a motor");
}

#[test]
fn the_motor_is_not_part_of_the_save_state() {
    let mut gb = rumble_cart();
    gb.poke(0x4000, 0x08);
    assert!(gb.rumble());
    let buzzing = gb.save_state();

    gb.poke(0x4000, 0x00);
    let quiet = gb.save_state();

    // Byte-identical, because the motor is an output to the frontend rather
    // than machine state. Serializing it would move a byte in every MBC5 save
    // state anybody already has, to carry a bit nothing reads back.
    assert_eq!(
        buzzing, quiet,
        "the motor bit must not appear in the save state"
    );
}

/// Pokemon Pinball, or None when the ROM is absent. A commercial cartridge
/// cannot be committed, so every test using it skips rather than fails.
fn pinball() -> Option<GameBoy> {
    let rel = "dumps/smdb/gb/Game Boy SMDB 2022-05-20/3 GBC with GB Compatibility - Black Carts/1 USA/Pokemon Pinball (USA, Australia) (Rumble Version) (SGB Enhanced) (GB Compatible).gbc";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(rel);
    if !path.exists() {
        eprintln!("skipping: needs Pokemon Pinball");
        return None;
    }
    Some(GameBoy::new(std::fs::read(&path).ok()?))
}

/// Minimal script player; this crate cannot depend on gb-runner.
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

#[test]
fn pokemon_pinball_is_a_rumble_cartridge() {
    // The one question only a real header can answer: that the cart types above
    // are the ones the games people own actually use.
    let Some(gb) = pinball() else { return };
    assert!(gb.has_rumble());
}

#[test]
fn pokemon_pinball_actually_drives_the_motor() {
    // The synthetic tests above prove the mapper does what the spec says. This
    // proves a real game asks for it, which is a different claim and the one
    // that would have caught a correct mapper wired to nothing.
    //
    // It needs the ball IN PLAY. A first attempt sat on the title screen and
    // then in the plunger lane and saw zero edges in sixty seconds, which reads
    // exactly like a broken implementation.
    let Some(mut gb) = pinball() else { return };
    assert!(gb.has_rumble());

    play(&mut gb, "w600,start,w200,a,w200,a,w300,start,w200");

    use gb_core::Button;
    gb.set_button(Button::Down, true); // pull the plunger
    for _ in 0..90 {
        gb.step_frame();
    }
    gb.set_button(Button::Down, false); // and launch

    let (mut on, mut edges, mut buzzing) = (false, 0u32, 0u32);
    for i in 0..3600u32 {
        // Work both flippers so the ball stays up and keeps hitting things.
        let left = i % 40 < 8;
        let right = (20..28).contains(&(i % 40));
        gb.set_button(Button::Left, left);
        gb.set_button(Button::A, left);
        gb.set_button(Button::Right, right);
        gb.set_button(Button::B, right);
        gb.step_frame();
        let now = gb.rumble();
        if now != on {
            edges += 1;
            on = now;
        }
        if now {
            buzzing += 1;
        }
    }

    assert!(
        edges > 0,
        "sixty seconds of pinball produced no motor activity at all"
    );
    // Measured: 74 edges and 107 frames buzzing. Asserted loosely because the
    // ball's path is not something to pin down to a number, but both bounds
    // matter. Zero means nothing fired; permanently on would mean the bit is
    // being read as latched-high, which would buzz a phone flat.
    assert!(
        buzzing > 0 && buzzing < 1800,
        "expected short bursts, got {buzzing} frames of 3600 across {edges} edges"
    );
}
