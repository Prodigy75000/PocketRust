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

#[test]
fn pokemon_pinball_is_a_rumble_cartridge() {
    // The one question only a real header can answer: that the cart types above
    // are the ones actually used by the games people own. Skipped when the ROM
    // is absent, since a commercial cartridge cannot be committed.
    let rel = "dumps/smdb/gb/Game Boy SMDB 2022-05-20/3 GBC with GB Compatibility \
               - Black Carts/1 USA/Pokemon Pinball (USA, Australia) (Rumble Version) \
               (SGB Enhanced) (GB Compatible).gbc";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(rel.replace("               ", ""));
    if !path.exists() {
        eprintln!("skipping: needs Pokemon Pinball");
        return;
    }
    let gb = GameBoy::new(std::fs::read(&path).unwrap());
    assert!(gb.has_rumble());
}
