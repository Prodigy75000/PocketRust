// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The SGB border a cartridge transfers, decoded and then composed with the
//! Game Boy screen into the 256x224 frame the core presents.
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

#[test]
fn the_screen_is_laid_into_the_border_at_the_centre_of_the_snes_frame() {
    // Two cartridges on purpose. A composed pixel comes from one of three
    // places and Kirby alone only proves two of them: its border is fully
    // opaque outside the screen window, so nothing there falls through to the
    // backdrop. Checked by deliberately filling the backdrop magenta, which
    // Kirby passed. Pokemon Blue's border has gaps and is what exercises it.
    let roms = [
        "dumps/roms/Kirby's Dream Land 2 (USA, Europe) (SGB Enhanced).gb",
        "dumps/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb",
    ];

    // Absolute, not derived from BORDER_ORIGIN_*, so that moving the constant
    // breaks this instead of moving with it. (256-160)/2 and (224-144)/2.
    const OX: usize = 48;
    const OY: usize = 40;

    let mut ran = 0usize;
    let mut total_backdrop = 0usize;
    for rel in roms {
        let Some(gb) = load(rel) else { continue };
        ran += 1;

        let mut out = vec![0xDEAD_BEEFu32; 256 * 224];
        assert!(gb.sgb_compose(&mut out), "{rel} has a border, so compose must draw");
        let border = gb.sgb_border().expect("same cartridge, same border");
        let screen = gb.framebuffer();
        let backdrop = gb.sgb_backdrop();

        // The full specification of a composed frame, per pixel: border art
        // wins, the screen shows through where the border is transparent and
        // the screen reaches, and everything else is the SNES backdrop.
        let (mut from_art, mut from_screen, mut from_backdrop) = (0usize, 0usize, 0usize);
        for y in 0..224 {
            for x in 0..256 {
                let px = y * 256 + x;
                let want = match border[px] {
                    Some(c) => {
                        from_art += 1;
                        c
                    }
                    None if (OX..OX + 160).contains(&x) && (OY..OY + 144).contains(&y) => {
                        from_screen += 1;
                        screen[(y - OY) * 160 + (x - OX)]
                    }
                    None => {
                        from_backdrop += 1;
                        backdrop
                    }
                };
                assert_eq!(out[px], want, "pixel ({x},{y}) of {rel}");
            }
        }

        // Every cartridge here leaves the whole screen window open, so all
        // 23040 pixels come through it. That is what makes the placement check
        // bite: a one-pixel error in either origin misaligns all of them.
        assert_eq!(from_screen, 160 * 144, "{rel}: the screen window is not fully open");
        assert!(from_art > 10_000, "{rel}: only {from_art} pixels of art");
        total_backdrop += from_backdrop;
    }

    if ran == 0 {
        return; // no ROMs on this machine; the skips were already reported
    }
    // Measured: Kirby contributes 0 and Pokemon Blue 3601. If this is ever 0
    // with ROMs present, the backdrop arm above is untested and a wrong fill
    // colour would ship green.
    assert!(
        ran < 2 || total_backdrop > 0,
        "no cartridge exercised the backdrop, so that branch is unproven"
    );

    // And the backdrop has to actually come from the Game Boy palette. Filling
    // it with black passes every check above, because the assertion would then
    // compare black against black, so this pins the one cartridge where the
    // answer is known: Pokemon Blue's colour 0 is white, and that white is what
    // fills its Pokeballs and its corner medallions. Black there was the bug.
    let blue = "dumps/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb";
    if let Some(gb) = load(blue) {
        assert_ne!(
            gb.sgb_backdrop() & 0xFF_FFFF,
            0x00_0000,
            "{blue}: a black backdrop means the fill is hardcoded, not read from the palette"
        );
    }
}

#[test]
fn a_cartridge_with_sgb_switched_off_is_never_composed() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("dumps/roms/Kirby's Dream Land 2 (USA, Europe) (SGB Enhanced).gb");
    if !path.exists() {
        return;
    }
    // The same cartridge that DOES compose in the test above, so this is about
    // the switch and not about the ROM. Without SGB the core never answers the
    // handshake, no transfer ever arrives, and presenting 256x224 would frame
    // the picture in a border that was never sent.
    let mut gb = GameBoy::new(std::fs::read(&path).unwrap());
    gb.set_sgb(false);
    for _ in 0..1400 {
        gb.step_frame();
    }
    let mut out = vec![0u32; 256 * 224];
    assert!(!gb.sgb_compose(&mut out), "SGB is off, so there is nothing to compose");
    assert!(out.iter().all(|p| *p == 0), "compose declined but still wrote pixels");
}
