// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The reproducibility claim, made checkable.
//!
//! `roms/pocketrust-demo/pocketrust-demo.gbc` is distributed on its own, away
//! from this repository, and PERMISSION.md says that anyone can confirm the
//! binary is exactly what the published source produces. This test is that
//! confirmation: it reassembles the committed source with the committed
//! assembler and compares the result byte for byte.
//!
//! It fails loudly if a single byte drifts, in either direction.

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
    let src = repo_root().join("roms/pocketrust-demo/src/main.s");
    let opts = gb_asm::asm::Options { symbol_file: false };
    gb_asm::asm::assemble(&src, &opts)
        .unwrap_or_else(|e| panic!("the demo cartridge no longer assembles: {e}"))
        .rom
}

fn committed() -> Vec<u8> {
    let path = repo_root().join("roms/pocketrust-demo/pocketrust-demo.gbc");
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
             Rebuild with scripts/build-demo-rom.sh, or find out what changed.",
            built[at], shipped[at]
        );
    }
}

#[test]
fn the_published_checksum_is_the_one_in_the_file() {
    // SHA256SUMS is what PERMISSION.md points a reader at, so it has to track
    // the image rather than being written once and forgotten.
    let listed = std::fs::read_to_string(repo_root().join("roms/pocketrust-demo/SHA256SUMS"))
        .expect("SHA256SUMS is missing");
    let listed = listed
        .split_whitespace()
        .next()
        .expect("SHA256SUMS is empty")
        .to_ascii_lowercase();
    assert_eq!(
        listed,
        sha256_hex(&committed()),
        "SHA256SUMS does not match the committed ROM"
    );
}

#[test]
fn the_header_is_the_one_a_game_boy_would_accept() {
    let rom = committed();
    assert_eq!(rom.len(), 32 * 1024, "a mapper-less cartridge is 32 KB");

    // The header checksum is the field with teeth: a boot ROM refuses to hand
    // over control if it is wrong, so a demo with a bad one would look like a
    // dead cartridge on real hardware and on any emulator that boots properly.
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
    assert_eq!(u16::from_be_bytes([rom[0x014e], rom[0x014f]]), sum, "global checksum");

    assert_eq!(&rom[0x0134..0x013e], b"POCKETRUST", "title");
    assert_eq!(rom[0x0143], 0x80, "colour-enhanced, still runs on a monochrome Game Boy");
    assert_eq!(rom[0x0147], 0x00, "no mapper");
    assert_eq!(rom[0x0148], 0x00, "32 KB");
    assert_eq!(rom[0x0149], 0x00, "no cartridge RAM");

    // The entry point at $0100 is `nop` then a jump into the cartridge proper.
    assert_eq!(rom[0x0100], 0x00);
    assert_eq!(rom[0x0101], 0xc3);
}

/// SHA-256, written out here rather than pulled in, because the whole point of
/// this directory is that its build and its checks have no dependencies.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[test]
fn the_hash_routine_agrees_with_a_known_answer() {
    // Without this, a broken hash would agree with itself and the checksum test
    // above would pass on nonsense.
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
