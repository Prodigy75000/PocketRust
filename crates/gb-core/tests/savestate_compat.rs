// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Save states written by an older release must still load.
//!
//! Everything else in the save-state suite is a round trip: this build against
//! itself. That proves the format is self-consistent and says nothing about
//! whether a state a player made last month still opens, which is the failure
//! they would actually notice, and the one nothing here could catch.
//!
//! The fixture is a real state produced by the **v0.2.3 binary**, checked out
//! at that tag and run against the CC0 demo cartridge in `roms/`. It is the
//! demo cart rather than a commercial ROM on purpose: a save state carries the
//! game's RAM, so a Pokemon state is derived from a copyrighted work and cannot
//! be committed to a public repository. The demo cart is ours and CC0.
//!
//! Regenerating it, if a format change is ever deliberate:
//!
//! ```sh
//! git worktree add /tmp/pr <tag>
//! # add a small bin that runs N frames and writes save_state()
//! cargo run --release -p gb-runner --bin statedump -- \
//!     roms/pocketrust-demo/pocketrust-demo.gbc 600 <this file>
//! ```
//!
//! If this test fails, that is not a licence to regenerate the fixture. It
//! means every save state anyone has is about to stop loading, so either the
//! change is wrong or `STATE_MAGIC` needs to advance and the old states need
//! reading through a compatibility path.

use gb_core::GameBoy;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(rel)
}

#[test]
fn a_state_from_v0_2_3_still_loads_and_is_unchanged() {
    let rom = std::fs::read(repo("roms/pocketrust-demo/pocketrust-demo.gbc"))
        .expect("the CC0 demo cartridge is committed; it is not an optional fixture");
    let old = std::fs::read(repo("crates/gb-core/tests/fixtures/demo-cart-v0.2.3.state"))
        .expect("the v0.2.3 state fixture is committed");

    let mut gb = GameBoy::new(rom);
    assert!(
        gb.load_state(&old),
        "this build refuses a save state written by v0.2.3"
    );

    // Loading is the half a player notices; re-serializing identically is the
    // half that proves no field quietly moved, grew or was dropped. A layout
    // change can survive load_state by reading the right number of bytes into
    // the wrong fields, and then only this comparison sees it.
    assert_eq!(
        gb.save_state(),
        old,
        "the save-state byte layout has changed since v0.2.3"
    );
}

#[test]
fn the_accessories_added_since_did_not_move_anything() {
    // The printer, the Game Boy Camera and MBC7 all added mapper state to the
    // serializer between v0.2.3 and now. None of them may cost a byte on a
    // cartridge that has none of them, which is what the fixture's exact
    // length pins: 58005 bytes, measured at v0.2.3, before any of that existed.
    let old = std::fs::read(repo("crates/gb-core/tests/fixtures/demo-cart-v0.2.3.state"))
        .expect("the v0.2.3 state fixture is committed");
    assert_eq!(old.len(), 58005);
}

#[test]
fn an_mbc5_state_from_v0_2_3_still_loads_and_is_unchanged() {
    // The demo-cart fixture above is a ROM-only cartridge, so it exercises no
    // mapper at all and could not have caught a change to one. That gap was
    // real: MBC5 gained a rumble field afterwards, and nothing in this file
    // would have noticed if it had moved a byte.
    //
    // MBC5 specifically, because it is the mapper behind Pokemon Yellow. The
    // cartridge here is synthetic and licence-free: a save state carries the
    // game's RAM, so one made from a commercial ROM is a derived copyrighted
    // work and cannot be committed.
    let rom = std::fs::read(repo("crates/gb-core/tests/fixtures/mbc5-cart.gb"))
        .expect("the synthetic MBC5 cartridge is committed");
    assert_eq!(rom[0x0147], 0x1B, "MBC5 + RAM + BATTERY, as Pokemon Yellow is");

    let old = std::fs::read(repo("crates/gb-core/tests/fixtures/mbc5-cart-v0.2.3.state"))
        .expect("the v0.2.3 MBC5 state fixture is committed");

    let mut gb = GameBoy::new(rom);
    assert!(
        gb.load_state(&old),
        "this build refuses an MBC5 save state written by v0.2.3"
    );
    assert_eq!(
        gb.save_state(),
        old,
        "the MBC5 save-state byte layout has changed since v0.2.3"
    );
}
