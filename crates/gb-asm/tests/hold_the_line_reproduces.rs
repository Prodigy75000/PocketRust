// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The reproducibility claim for HOLD THE LINE, made checkable.
//!
//! The cartridge is CC0 and is meant to be handed to people on its own, away
//! from this repository, so anyone has to be able to confirm the binary is
//! exactly what the published source produces. This test is that confirmation:
//! it reassembles the committed source with the committed assembler and
//! compares the result byte for byte.
//!
//! The header checks below matter more than they look. The header checksum is
//! the field with teeth: a boot ROM refuses to hand over control if it is
//! wrong, so a cartridge with a bad one is dead on real hardware and on any
//! emulator that boots properly, while looking perfectly fine in a hex editor.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/gb-asm.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn assemble() -> Vec<u8> {
    let src = repo_root().join("roms/hold-the-line/src/main.s");
    let opts = gb_asm::asm::Options { symbol_file: false };
    gb_asm::asm::assemble(&src, &opts)
        .unwrap_or_else(|e| panic!("HOLD THE LINE no longer assembles: {e}"))
        .rom
}

fn committed() -> Vec<u8> {
    let path = repo_root().join("roms/hold-the-line/hold-the-line.gbc");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn the_committed_rom_is_what_the_committed_source_assembles_to() {
    let built = assemble();
    let shipped = committed();

    assert_eq!(
        built.len(),
        shipped.len(),
        "the ROM changed size: source produces {} bytes, the committed image is {}",
        built.len(),
        shipped.len()
    );

    if let Some(at) = built.iter().zip(&shipped).position(|(a, b)| a != b) {
        panic!(
            "the committed ROM and the committed source have drifted apart.\n\
             First difference at ${at:04X}: source says ${:02X}, the image says ${:02X}.\n\
             Rebuild with scripts/build-hold-the-line.sh, or find out what changed.",
            built[at], shipped[at]
        );
    }
}

#[test]
fn the_committed_sym_file_describes_the_committed_rom() {
    // crates/gb-core/tests/hold_the_line.rs reads work RAM addresses out of this
    // file rather than hard-coding them. A stale .sym would make those tests
    // read the wrong bytes and still pass or fail for reasons of their own, so
    // the listing has to be rebuilt alongside the image.
    let path = repo_root().join("roms/hold-the-line/hold-the-line.sym");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let addr_of = |name: &str| -> u16 {
        text.lines()
            .find_map(|line| {
                let mut it = line.split_whitespace();
                let addr = it.next()?;
                (it.next()? == name).then(|| u16::from_str_radix(addr, 16).ok())?
            })
            .unwrap_or_else(|| panic!("{name} is not in {}", path.display()))
    };

    // `start` is the reset routine, and main.s puts it at $0150 with a .org, so
    // this compares the listing against a number the source states outright.
    assert_eq!(addr_of("start"), 0x0150, "the reset vector moved");

    // The work RAM the core tests read has to actually be in work RAM.
    for name in ["wCells", "wPath", "wPathLen", "wCurX", "wCurY"] {
        let a = addr_of(name);
        assert!(
            (0xC000..0xE000).contains(&a),
            "{name} is listed at ${a:04X}, which is not work RAM"
        );
    }

    // The object buffer has to start on a page boundary, because the DMA
    // controller takes only the high byte of the source address. main.s asserts
    // this too; it is repeated here because this is the copy that is checked
    // against the image someone actually has.
    assert_eq!(addr_of("wOam") & 0xFF, 0, "the object buffer is not page aligned");
}

#[test]
fn the_header_is_the_one_a_game_boy_would_accept() {
    let rom = committed();
    assert_eq!(rom.len(), 32 * 1024, "a mapper-less cartridge is 32 KB");

    let mut x: u8 = 0;
    for &b in &rom[0x0134..=0x014c] {
        x = x.wrapping_sub(b).wrapping_sub(1);
    }
    assert_eq!(rom[0x014d], x, "header checksum");

    // The global checksum is not enforced by anything, but a zero there is a
    // tell that the tool skipped it.
    let sum = rom
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 0x014e && *i != 0x014f)
        .fold(0u16, |acc, (_, &b)| acc.wrapping_add(b as u16));
    assert_eq!(
        u16::from_be_bytes([rom[0x014e], rom[0x014f]]),
        sum,
        "global checksum"
    );

    assert_eq!(&rom[0x0134..0x0141], b"HOLD THE LINE", "title");
    assert_eq!(
        rom[0x0143], 0x80,
        "colour-enhanced, still runs on a monochrome Game Boy"
    );
    assert_eq!(rom[0x0147], 0x00, "no mapper");
    assert_eq!(rom[0x0148], 0x00, "32 KB");
    assert_eq!(rom[0x0149], 0x00, "no cartridge RAM");

    // The entry point at $0100 is `nop` then a jump into the cartridge proper.
    assert_eq!(rom[0x0100], 0x00);
    assert_eq!(rom[0x0101], 0xc3);
}

#[test]
fn there_is_room_left_to_finish_the_game() {
    // gb-asm has no bank support at all, so 32 KB is not a budget that can be
    // raised by editing a number: outgrowing it means teaching the assembler
    // about banks first. That is worth knowing early rather than on the day the
    // last feature does not fit, which is the whole reason this test exists.
    let rom = committed();
    let used = rom.iter().rposition(|&b| b != 0xFF).map_or(0, |i| i + 1);
    assert!(
        used < 30 * 1024,
        "the cartridge is using {used} of 32768 bytes. gb-asm cannot bank, so \
         the next step is bank support in the assembler, not squeezing."
    );
    println!("HOLD THE LINE: {used} of 32768 bytes used");
}
