// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The core, driven by HOLD THE LINE in `roms/hold-the-line/`.
//!
//! Like the demo cartridge, this ROM is ours and is checked in under CC0, so
//! these tests need nothing the person running them has to supply.
//!
//! What they measure is the one thing on that cartridge that is derived rather
//! than written down. The map in `src/data.s` is a picture; the route creeps
//! walk is not in the source at all, it is worked out at load by following path
//! cells from the spawn. `tools/checkmap.py` proves the *picture* is sound, but
//! it is a second implementation in another language, and agreeing with itself
//! is not the claim. The claim is that the code on the cartridge derives the
//! same route, and the only way to check that is to run it and read the result
//! out of work RAM.
//!
//! Addresses come from the committed `.sym` file rather than being written here
//! as constants, because work RAM moves every time the game grows a variable
//! and a test that silently reads the wrong byte is worse than no test.

use gb_core::{Button, GameBoy};
use std::collections::HashMap;
use std::path::PathBuf;

const GRID_W: u8 = 10;
const GRID_H: u8 = 8;

/// Cell kinds, as `src/main.s` numbers them.
const CELL_GROUND: u8 = 0;
const CELL_PATH: u8 = 1;
const CELL_SPAWN: u8 = 2;
const CELL_EXIT: u8 = 3;

fn rom_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("roms/hold-the-line")
}

/// The `.sym` listing gb-asm writes beside the ROM: one `ADDR  name` per line.
fn symbols() -> HashMap<String, u16> {
    let path = rom_dir().join("hold-the-line.sym");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let map: HashMap<String, u16> = text
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let addr = u16::from_str_radix(it.next()?, 16).ok()?;
            Some((it.next()?.to_string(), addr))
        })
        .collect();
    assert!(
        !map.is_empty(),
        "{} has no symbols in it; rebuild with scripts/build-hold-the-line.sh",
        path.display()
    );
    map
}

fn sym(syms: &HashMap<String, u16>, name: &str) -> u16 {
    *syms.get(name).unwrap_or_else(|| {
        panic!("the cartridge no longer has a symbol called {name}; this test is reading a stale map")
    })
}

fn boot() -> (GameBoy, HashMap<String, u16>) {
    let path = rom_dir().join("hold-the-line.gbc");
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut gb = GameBoy::new(rom);
    // Reset clears 8 KB of work RAM and copies the tile set, which takes longer
    // than one frame. By 120 the board is drawn and the route is derived.
    run(&mut gb, 120);
    (gb, symbols())
}

fn run(gb: &mut GameBoy, frames: usize) {
    for _ in 0..frames {
        gb.step_frame();
    }
}

fn tap(gb: &mut GameBoy, b: Button) {
    gb.set_button(b, true);
    run(gb, 4);
    gb.set_button(b, false);
    run(gb, 4);
}

fn read(gb: &GameBoy, addr: u16, len: usize) -> Vec<u8> {
    (0..len).map(|i| gb.peek(addr + i as u16)).collect()
}

/// The board as the cartridge decoded it, and the route as it derived it.
fn board_and_route(gb: &GameBoy, syms: &HashMap<String, u16>) -> (Vec<u8>, Vec<u8>) {
    let cells = read(gb, sym(syms, "wCells"), (GRID_W * GRID_H) as usize);
    let len = gb.peek(sym(syms, "wPathLen")) as usize;
    let route = read(gb, sym(syms, "wPath"), len);
    (cells, route)
}

fn col(idx: u8) -> u8 {
    idx % GRID_W
}
fn row(idx: u8) -> u8 {
    idx / GRID_W
}

#[test]
fn the_picture_decodes_into_the_board_it_draws() {
    let (gb, syms) = boot();
    let (cells, _) = board_and_route(&gb, &syms);

    assert_eq!(cells.len(), 80, "the board is ten cells by eight");
    for (i, &k) in cells.iter().enumerate() {
        assert!(
            k <= CELL_EXIT,
            "cell {i} decoded to kind {k}, which is not a cell kind. A character \
             the map uses is missing from cell_kind in data.s, so it read as zero."
        );
    }
    assert_eq!(
        cells.iter().filter(|&&k| k == CELL_SPAWN).count(),
        1,
        "exactly one spawn"
    );
    assert_eq!(
        cells.iter().filter(|&&k| k == CELL_EXIT).count(),
        1,
        "exactly one exit"
    );
    assert!(
        cells.iter().filter(|&&k| k == CELL_GROUND).count() > 20,
        "a map with nowhere to build is not a tower defence"
    );
}

#[test]
fn the_cartridge_derives_the_route_from_the_picture() {
    let (gb, syms) = boot();
    let (cells, route) = board_and_route(&gb, &syms);

    let walkable = |k: u8| matches!(k, CELL_PATH | CELL_SPAWN | CELL_EXIT);

    assert!(
        !route.is_empty(),
        "the cartridge derived an empty route. It could not find a spawn, or the \
         walk gave up on the first step."
    );

    // The two ends are the two ends.
    assert_eq!(
        cells[route[0] as usize], CELL_SPAWN,
        "the route starts somewhere that is not the spawn"
    );
    assert_eq!(
        cells[*route.last().unwrap() as usize],
        CELL_EXIT,
        "the route stops somewhere that is not the exit. A walk that ran out of \
         room, or one that got stuck, ends exactly like this."
    );

    // Every step is one cell, orthogonally. This is what catches a walk that
    // wrapped round a row edge: index 9 and index 10 differ by one and are on
    // opposite sides of the board.
    for pair in route.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dx = col(a).abs_diff(col(b));
        let dy = row(a).abs_diff(row(b));
        assert_eq!(
            (dx, dy),
            (dx.min(1), dy.min(1)),
            "the route steps from cell {a} ({},{}) to {b} ({},{}), which is not \
             one cell away",
            col(a),
            row(a),
            col(b),
            row(b)
        );
        assert_eq!(
            dx + dy,
            1,
            "the route steps from cell {a} to {b}, which is diagonal or stationary"
        );
    }

    // It never doubles back, and it never visits anything twice.
    let mut seen = vec![false; 80];
    for &idx in &route {
        assert!(
            walkable(cells[idx as usize]),
            "the route walks over cell {idx}, which is ground"
        );
        assert!(
            !seen[idx as usize],
            "the route visits cell {idx} twice, so the walk turned round"
        );
        seen[idx as usize] = true;
    }

    // And it leaves nothing behind: a path cell that is not on the route is a
    // second loop somewhere on the board that no creep would ever reach.
    let stranded: Vec<usize> = (0..80)
        .filter(|&i| walkable(cells[i]) && !seen[i])
        .collect();
    assert!(
        stranded.is_empty(),
        "cells {stranded:?} are path but are not on the route"
    );

    // Map 1's comb, counted off the picture in data.s by hand and by
    // tools/checkmap.py independently. Stated as an absolute number rather than
    // as a length derived from the route, which would agree with anything.
    assert_eq!(route.len(), 36, "map 1 is a 36 cell route");

    // Creeps go in at the top and come out at the top, which is the constraint
    // the whole map shape follows from. It is worth asserting because a map that
    // quietly grew an exit on another edge would still pass every check above:
    // the route would be perfectly well formed and the game would be a
    // different game.
    assert_eq!(row(route[0]), 0, "the spawn is not on the top edge");
    assert_eq!(
        row(*route.last().unwrap()),
        0,
        "the exit is not on the top edge"
    );

    // And the two ends are not the same place, which is the degenerate map the
    // rule above would otherwise allow.
    assert!(
        col(route[0]).abs_diff(col(*route.last().unwrap())) > 4,
        "the two ends are too close together for the route between them to matter"
    );
}

#[test]
fn the_build_cursor_moves_and_stops_at_the_edges() {
    let (mut gb, syms) = boot();
    let (cx, cy) = (sym(&syms, "wCurX"), sym(&syms, "wCurY"));

    assert_eq!((gb.peek(cx), gb.peek(cy)), (0, 0), "the cursor starts home");

    tap(&mut gb, Button::Right);
    tap(&mut gb, Button::Down);
    assert_eq!((gb.peek(cx), gb.peek(cy)), (1, 1), "one press is one cell");

    // Far more presses than the board is wide, to prove the clamp is a clamp
    // and not a wrap. A wrapped cursor would come back round to a low number.
    for _ in 0..20 {
        tap(&mut gb, Button::Right);
        tap(&mut gb, Button::Down);
    }
    assert_eq!(
        (gb.peek(cx), gb.peek(cy)),
        (GRID_W - 1, GRID_H - 1),
        "the cursor should stop on the last cell, not wrap or run off the board"
    );

    for _ in 0..20 {
        tap(&mut gb, Button::Left);
        tap(&mut gb, Button::Up);
    }
    assert_eq!(
        (gb.peek(cx), gb.peek(cy)),
        (0, 0),
        "and stop on the first one going the other way"
    );
}
