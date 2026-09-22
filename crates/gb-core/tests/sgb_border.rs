// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The SGB border a cartridge transfers. Decode only: nothing displays it.
//!
//! Skipped when the ROMs are absent, since commercial cartridges cannot be
//! committed.

use gb_core::GameBoy;
use std::path::PathBuf;

fn load(rel: &str) -> Option<GameBoy> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(rel);
    if !path.exists() {
        eprintln!("skipping: needs {rel}");
        return None;
    }
    let mut gb = GameBoy::new(std::fs::read(&path).ok()?);
    gb.set_sgb(true);
    for _ in 0..1400 {
        gb.step_frame();
    }
    Some(gb)
}

#[test]
fn a_cartridge_border_decodes_to_artwork_with_a_hole_in_it() {
    let roms = [
        "dumps/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb",
        "dumps/roms/Kirby's Dream Land 2 (USA, Europe) (SGB Enhanced).gb",
    ];
    for rel in roms {
        let Some(gb) = load(rel) else { continue };
        let border = gb
            .sgb_border()
            .unwrap_or_else(|| panic!("{rel} sends CHR_TRN and PCT_TRN, so a border must decode"));
        assert_eq!(border.len(), 256 * 224);

        let solid = border.iter().filter(|p| p.is_some()).count();
        let holes = border.len() - solid;

        // Both halves matter. Artwork everywhere would mean the transparent
        // colour is not being honoured and the Game Boy screen would be
        // covered; nothing anywhere would mean the transfer decoded to zeroes,
        // which is exactly how the first attempt at reading these failed.
        assert!(
            solid > 10_000,
            "{rel}: only {solid} pixels of artwork, transfer probably decoded to nothing"
        );
        assert!(
            holes > 20_000,
            "{rel}: only {holes} transparent pixels, the Game Boy screen would be covered"
        );

        // And it must be a picture rather than a flat fill. The bound is
        // measured, not guessed: Pokemon Blue's border uses 7 distinct colours
        // and Kirby's Dream Land 2's uses 18. An SGB border has 48 available
        // and most use very few, so a threshold picked for how rich a border
        // "ought" to look fails on real artwork. This one only has to separate
        // a picture from a flat fill.
        let distinct: std::collections::HashSet<u32> = border.iter().flatten().copied().collect();
        assert!(
            distinct.len() >= 4,
            "{rel}: {} distinct colours is a fill, not artwork",
            distinct.len()
        );
    }
}
