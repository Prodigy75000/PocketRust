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

const GRID_W: u8 = 14;
const GRID_H: u8 = 17;
const GRID_CELLS: usize = GRID_W as usize * GRID_H as usize;

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
    // Reset clears 8 KB of work RAM, copies the tile set and draws the board,
    // which takes longer than one frame. By 120 the route is derived.
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
///
/// The route comes back as (column, row) pairs, which is how the cartridge
/// stores it: a column and a row are what drawing a creep wants, and the board
/// has been resized twice already without that representation caring.
fn board_and_route(gb: &GameBoy, syms: &HashMap<String, u16>) -> (Vec<u8>, Vec<(u8, u8)>) {
    let cells = read(gb, sym(syms, "wCells"), GRID_CELLS);
    let len = gb.peek(sym(syms, "wPathLen")) as usize;
    let cols = read(gb, sym(syms, "wPathCol"), len);
    let rows = read(gb, sym(syms, "wPathRow"), len);
    (cells, cols.into_iter().zip(rows).collect())
}

fn at(cells: &[u8], (c, r): (u8, u8)) -> u8 {
    cells[r as usize * GRID_W as usize + c as usize]
}

fn walkable(k: u8) -> bool {
    matches!(k, CELL_PATH | CELL_SPAWN | CELL_EXIT)
}

#[test]
fn the_picture_decodes_into_the_board_it_draws() {
    let (gb, syms) = boot();
    let (cells, _) = board_and_route(&gb, &syms);

    assert_eq!(cells.len(), 238, "the board is fourteen cells by seventeen");
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
        cells.iter().filter(|&&k| k == CELL_GROUND).count() > 100,
        "a map with nowhere to build is not a tower defence"
    );
}

#[test]
fn the_cartridge_derives_the_route_from_the_picture() {
    let (gb, syms) = boot();
    let (cells, route) = board_and_route(&gb, &syms);

    assert!(
        !route.is_empty(),
        "the cartridge derived an empty route. It could not find a spawn, or the \
         walk gave up on the first step."
    );

    // The two ends are the two ends.
    assert_eq!(
        at(&cells, route[0]),
        CELL_SPAWN,
        "the route starts somewhere that is not the spawn"
    );
    assert_eq!(
        at(&cells, *route.last().unwrap()),
        CELL_EXIT,
        "the route stops somewhere that is not the exit. A walk that ran out of \
         room, or one that got stuck, ends exactly like this."
    );

    // Every waypoint is on the board at all. The walk works in columns and rows
    // now, and a row of 255 would index happily into work RAM and look plausible.
    for &(c, r) in &route {
        assert!(
            c < GRID_W && r < GRID_H,
            "the route visits ({c},{r}), which is off a {GRID_W} by {GRID_H} board"
        );
    }

    // Every step is exactly one cell, orthogonally.
    for pair in route.windows(2) {
        let ((c0, r0), (c1, r1)) = (pair[0], pair[1]);
        let d = c0.abs_diff(c1) + r0.abs_diff(r1);
        assert_eq!(
            d, 1,
            "the route steps from ({c0},{r0}) to ({c1},{r1}), which is diagonal, \
             stationary, or a jump"
        );
    }

    // It never doubles back, and it never visits anything twice.
    let mut seen = vec![false; GRID_CELLS];
    for &p in &route {
        assert!(
            walkable(at(&cells, p)),
            "the route walks over {p:?}, which is ground"
        );
        let i = p.1 as usize * GRID_W as usize + p.0 as usize;
        assert!(
            !seen[i],
            "the route visits {p:?} twice, so the walk turned round"
        );
        seen[i] = true;
    }

    // And it leaves nothing behind: a path cell that is not on the route is a
    // second loop somewhere on the board that no creep would ever reach. This is
    // the one that catches a switchback whose corridor and its connector are on
    // different columns and therefore never actually meet, which is exactly the
    // defect the first draft of this map had.
    let stranded: Vec<(usize, usize)> = (0..GRID_CELLS)
        .filter(|&i| walkable(cells[i]) && !seen[i])
        .map(|i| (i % GRID_W as usize, i / GRID_W as usize))
        .collect();
    assert!(
        stranded.is_empty(),
        "{} cells are path but are not on the route, starting at {:?}. The route \
         is broken in two somewhere.",
        stranded.len(),
        &stranded[..stranded.len().min(4)]
    );

    // Map 1's switchback, counted off the picture in data.s by tools/checkmap.py
    // independently. Stated as an absolute number rather than as a length
    // derived from the route, which would agree with anything.
    assert_eq!(route.len(), 106, "map 1 is a 106 cell route");

    // Creeps come in at the top and leave at the top, the way Element TD does.
    // A map that quietly grew its ends somewhere else would pass every check
    // above and simply be a different game.
    assert_eq!(route[0].1, 0, "the spawn is not on the top edge");
    assert_eq!(route.last().unwrap().1, 0, "the exit is not on the top edge");
    assert!(
        route[0].0.abs_diff(route.last().unwrap().0) > 4,
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
    for _ in 0..30 {
        tap(&mut gb, Button::Right);
        tap(&mut gb, Button::Down);
    }
    assert_eq!(
        (gb.peek(cx), gb.peek(cy)),
        (GRID_W - 1, GRID_H - 1),
        "the cursor should stop on the last cell, not wrap or run off the board"
    );

    for _ in 0..30 {
        tap(&mut gb, Button::Left);
        tap(&mut gb, Button::Up);
    }
    assert_eq!(
        (gb.peek(cx), gb.peek(cy)),
        (0, 0),
        "and stop on the first one going the other way"
    );
}

#[test]
fn creeps_walk_the_route_and_cost_a_life_when_they_get_out() {
    let (mut gb, syms) = boot();
    let lives = sym(&syms, "wLives");
    let alive = sym(&syms, "wCreepAlive");
    let idx = sym(&syms, "wCreepIdx");

    let start = gb.peek(lives);
    assert!(start > 0, "the game starts with lives");

    run(&mut gb, 400);
    let on_board = (0..16).filter(|i| gb.peek(alive + i) != 0).count();
    assert!(on_board > 0, "no creep reached the board at all");

    // They are spread along the route rather than piled on the spawn, which is
    // what a broken walk or a speed of zero would look like from here.
    let furthest = (0..16)
        .filter(|i| gb.peek(alive + i) != 0)
        .map(|i| gb.peek(idx + i))
        .max()
        .unwrap();
    assert!(
        furthest > 10,
        "the furthest creep has only reached waypoint {furthest} after 400 frames"
    );

    // And eventually one gets out, which has to cost something.
    run(&mut gb, 2600);
    assert!(
        gb.peek(lives) < start,
        "creeps have had 3000 frames to cross a 106 cell route and lives are \
         still {start}, so nothing is leaking at the exit"
    );
}
