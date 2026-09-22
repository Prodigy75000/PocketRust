// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Shared pieces of the headless tooling.
//!
//! The input-script player lives here rather than in `shot` because more than
//! one tool needs to drive a ROM to a particular screen, and two copies of a
//! script parser is two dialects waiting to disagree about what a recipe means.

use gb_core::{Button, GameBoy};

/// Play an input script. Anything it does not recognise is a hard error rather
/// than a silent skip, because a typo in a screenshot recipe would otherwise
/// produce a plausible picture of the wrong screen.
pub fn play(gb: &mut GameBoy, script: &str) {
    for token in script.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(n) = token.strip_prefix('w') {
            let n: u32 = n.parse().unwrap_or_else(|_| panic!("bad wait {token:?}"));
            for _ in 0..n {
                gb.step_frame();
            }
            continue;
        }
        // tilt:<x>/<y> in g, for MBC7. Slash, not comma: the script itself is
        // comma-separated, so a comma here splits the token in half.
        //
        // Sticky: it stays until the next tilt token, because the game latches
        // when IT chooses and a one-frame pulse would almost always be missed.
        if let Some(rest) = token.strip_prefix("tilt:") {
            let (x, y) = rest
                .split_once('/')
                .unwrap_or_else(|| panic!("bad tilt {token:?}, want tilt:<x>/<y>"));
            let parse = |v: &str| v.parse::<f32>().unwrap_or_else(|_| panic!("bad tilt {token:?}"));
            if !gb.set_tilt(parse(x), parse(y)) {
                panic!("tilt: this cartridge has no tilt sensor");
            }
            continue;
        }
        let (name, action) = match token.as_bytes()[0] {
            b'+' => (&token[1..], 1),
            b'-' => (&token[1..], 2),
            _ => (token, 0),
        };
        let button = match name {
            "a" => Button::A,
            "b" => Button::B,
            "up" => Button::Up,
            "down" => Button::Down,
            "left" => Button::Left,
            "right" => Button::Right,
            "start" => Button::Start,
            "select" => Button::Select,
            other => panic!("no such button {other:?}"),
        };
        match action {
            1 => gb.set_button(button, true),
            2 => gb.set_button(button, false),
            _ => {
                gb.set_button(button, true);
                for _ in 0..6 {
                    gb.step_frame();
                }
                gb.set_button(button, false);
                for _ in 0..6 {
                    gb.step_frame();
                }
            }
        }
    }
}

