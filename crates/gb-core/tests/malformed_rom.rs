// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! A file a player opened is untrusted input, and must not panic the core.
//!
//! In a libretro core a panic is not an error return, it is the whole app
//! going down. Found by sweeping a full 5344-ROM set: the three Game Boy boot
//! ROMs in it are 256 bytes each and every one of them crashed the loader
//! while reading the cartridge title at $0134.

use gb_core::{rom_is_loadable, GameBoy, MIN_ROM_LEN};

#[test]
fn a_file_too_short_to_hold_a_header_is_not_loadable() {
    // The header ends at $014F, so $0150 is the first loadable length.
    assert_eq!(MIN_ROM_LEN, 0x0150);
    assert!(!rom_is_loadable(&[]));
    assert!(!rom_is_loadable(&vec![0u8; 256])); // a boot ROM, the real case
    assert!(!rom_is_loadable(&vec![0u8; 0x014F]));
    assert!(rom_is_loadable(&vec![0u8; 0x0150]));
}

#[test]
fn building_a_machine_from_a_short_file_does_not_panic() {
    // Deliberately NOT going through rom_is_loadable first. The frontend guard
    // is a courtesy; the library still has to survive being called directly,
    // which is how the runner tools and any future embedder reach it.
    for len in [0usize, 1, 16, 256, 0x0100, 0x0134, 0x014F] {
        let gb = GameBoy::new(vec![0u8; len]);
        // And it has to keep running, not merely construct. A header made of
        // padding still has to drive a CPU that fetches from an empty ROM.
        let mut gb = gb;
        for _ in 0..3 {
            gb.step_frame();
        }
        assert_eq!(gb.framebuffer().len(), 160 * 144, "len {len}");
    }
}

#[test]
fn an_out_of_range_rom_size_code_does_not_overflow_the_shift() {
    // $0148 is a size code from the file and it is used as a SHIFT. The
    // largest defined value is $08, 512 banks. An undefined one used to shift
    // a usize by up to 255, which panics in debug and is meaningless in
    // release. No ROM in the 5344-cartridge set carries a bad one, so this is
    // hardening rather than a reported bug, but the byte is attacker-supplied
    // in exactly the same way the length was.
    for code in [0x09u8, 0x20, 0x52, 0x7F, 0xFF] {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0148] = code;
        let mut gb = GameBoy::new(rom);
        gb.step_frame();
    }
}
