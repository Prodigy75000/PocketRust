// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The Game Boy Printer, driven by a real game.
//!
//! The unit tests in `src/printer.rs` check the protocol against packets this
//! repository builds itself, which proves the state machine agrees with the
//! specification and nothing more. This file is the other half: Pokemon Yellow,
//! from a save state sitting on the Pokedex with PRNT on screen, actually
//! printing. A protocol that is right on paper and wrong on the wire looks
//! exactly like a protocol that is right, until a game refuses to talk to it.
//!
//! **These tests skip when the files are not there**, because neither the ROM
//! nor a save state of it can be committed. To run them, put
//!
//! ```text
//!   dumps/roms/Pokemon - Yellow Version - Special Pikachu Edition (USA, Europe) (CGB+SGB Enhanced).gb
//!   dumps/states/yellow-pokedex-prnt.state
//! ```
//!
//! in place. `dumps/` is git-ignored. The save state is one the owner made on
//! the Pokedex with Pikachu selected and the side menu open; the input script
//! below walks from there to PRNT.

use gb_core::{Button, GameBoy, PrinterHandle, Sheet};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

const ROM: &str = "dumps/roms/Pokemon - Yellow Version - Special Pikachu Edition (USA, Europe) (CGB+SGB Enhanced).gb";
const STATE: &str = "dumps/states/yellow-pokedex-prnt.state";

/// Run Yellow from the Pokedex save state through a print, or return `None` if
/// the files to do that with are not on this machine.
fn print_a_pokedex_entry() -> Option<(Vec<Sheet>, Vec<(u8, usize)>)> {
    let rom = repo_root().join(ROM);
    let state = repo_root().join(STATE);
    if !rom.exists() || !state.exists() {
        eprintln!(
            "skipping: needs {} and {}; see the module comment",
            ROM, STATE
        );
        return None;
    }

    let mut gb = GameBoy::new(std::fs::read(&rom).ok()?);
    // The printer goes on before the state does. Restoring a state does not
    // touch the link, so a game already mid-conversation would find nothing.
    let printer = PrinterHandle::new();
    gb.connect_link(Box::new(printer.clone()));
    assert!(
        gb.load_state(&std::fs::read(&state).ok()?),
        "the save state was refused, so it is for another build of the core"
    );

    // From the contents list: A opens the side menu on DATA, three downs reach
    // PRNT, A prints.
    let script = [
        (None, 20),
        (Some(Button::A), 0),
        (None, 40),
        (Some(Button::Down), 0),
        (Some(Button::Down), 0),
        (Some(Button::Down), 0),
        (None, 20),
        (Some(Button::A), 0),
        (None, 60),
    ];
    for (button, wait) in script {
        match button {
            Some(b) => {
                gb.set_button(b, true);
                for _ in 0..6 {
                    gb.step_frame();
                }
                gb.set_button(b, false);
                for _ in 0..6 {
                    gb.step_frame();
                }
            }
            None => {
                for _ in 0..wait {
                    gb.step_frame();
                }
            }
        }
    }
    for _ in 0..3000 {
        gb.step_frame();
    }

    Some((printer.take_sheets(), printer.log()))
}

fn ink(sheet: &Sheet) -> f32 {
    let dark = sheet.pixels.iter().filter(|&&p| p > 0).count();
    dark as f32 / sheet.pixels.len() as f32
}

#[test]
fn pokemon_yellow_prints_a_pokedex_entry() {
    let Some((sheets, log)) = print_a_pokedex_entry() else {
        return;
    };

    // The conversation the game actually had. Asserting the shape of it rather
    // than only the picture means a regression says WHERE it broke: a game that
    // gives up after the init never reaches the data packets.
    let commands: Vec<u8> = log
        .iter()
        .map(|&(c, _)| c)
        .filter(|&c| c != 0x0F) // status polls, of which there are dozens
        .collect();
    assert_eq!(
        commands,
        vec![
            0x01, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x02, // sprite and stats
            0x01, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x02, // the text
        ],
        "Yellow sends initialise, a run of data packets, an empty data packet \
         and then print, twice. Getting a different sequence means the printer \
         answered something the game did not expect."
    );

    // Every data packet except the terminator is a full band.
    for &(cmd, len) in log.iter().filter(|&&(c, _)| c == 0x04) {
        assert!(
            len == 640 || len == 0,
            "a data packet of {len} bytes; they are $280 or empty (cmd ${cmd:02X})"
        );
    }

    assert_eq!(sheets.len(), 2, "a Pokedex entry is two prints");

    let (top, bottom) = (&sheets[0], &sheets[1]);
    assert_eq!((top.width, top.height), (160, 80), "the sprite and stats page");
    assert_eq!((bottom.width, bottom.height), (160, 112), "the text page");

    // The margins are the whole reason the two are one picture.
    assert_eq!(top.margin_after, 0, "the first page does not feed paper after");
    assert_eq!(
        bottom.margin_before, 0,
        "the second does not feed paper before, so they are continuous"
    );
    assert_eq!(bottom.margin_after, 3, "and the job ends with a feed");

    for s in &sheets {
        assert_eq!(s.palette, 0xE4, "Yellow prints with the identity palette");
        assert_eq!(s.copies, 1);
        // A blank page and a solid black page are the two ways a renderer fails
        // while still producing an image of exactly the right size.
        let coverage = ink(s);
        assert!(
            (0.05..0.60).contains(&coverage),
            "{:.1}% of the page is ink, which is either blank or solid",
            coverage * 100.0
        );
    }
}

#[test]
fn the_page_uses_more_than_one_shade() {
    // The unit tests in src/printer.rs prove the palette is applied, but they do
    // it with a band of $FF, where every pixel is colour 3. That would pass just
    // as happily if the renderer dropped the high bit plane and read every tile
    // as one bit deep, which is a classic way to get 2bpp wrong.
    //
    // A real Pokedex page has all four shades in it, so counting them here
    // catches exactly that, and the unit tests cannot.
    let Some((sheets, _)) = print_a_pokedex_entry() else {
        return;
    };

    let mut seen = [0usize; 4];
    for s in &sheets {
        for &p in &s.pixels {
            seen[(p & 3) as usize] += 1;
        }
    }
    assert!(
        seen.iter().all(|&n| n > 0),
        "the printed page only uses shades {:?}. A renderer that lost a bit          plane would look like this.",
        seen
    );
    // And the two extremes are the bulk of it, which is what printed Game Boy
    // artwork looks like: mostly paper and ink, with shading at the edges.
    let total: usize = seen.iter().sum();
    assert!(
        seen[0] + seen[3] > total / 2,
        "a printed page should be mostly paper and ink, not mostly midtones"
    );
}
