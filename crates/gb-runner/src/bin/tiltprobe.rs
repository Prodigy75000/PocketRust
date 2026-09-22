// SPDX-License-Identifier: GPL-3.0-or-later
//! Sweep the MBC7 accelerometer and report where the player sprite ends up.
//!
//! Eyeballing two screenshots cannot tell "the tilt did nothing" from "the
//! tilt worked and he was already against a wall". This reads sprite 0's X
//! out of OAM instead, so the answer is a number.

use gb_core::GameBoy;

fn main() {
    let mut args = std::env::args().skip(1);
    let rom = args.next().expect("usage: tiltprobe <rom> <script-to-gameplay>");
    let intro = args.next().expect("need a script that reaches gameplay");
    let rom = std::fs::read(&rom).expect("read rom");

    for (tx, ty) in [
        (0.0f32, 0.0f32),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (0.7, 0.7),
        (-0.7, -0.7),
    ] {
        let mut gb = GameBoy::new(rom.clone());
        assert!(gb.has_tilt(), "this cartridge has no tilt sensor");
        gb_runner::play(&mut gb, &intro);
        gb.set_tilt(tx, ty);
        for _ in 0..240 {
            gb.step_frame();
        }
        let oam = gb.oam();
        // The camera integrates movement, so it separates "he drifted a little"
        // from "he never moved" far better than one instantaneous sprite.
        println!(
            "tilt {tx:+.1}/{ty:+.1}g -> scroll x={:3} y={:3}  sprite0 {:3},{:3}",
            gb.peek(0xFF43),
            gb.peek(0xFF42),
            oam[1],
            oam[0]
        );
    }
}
