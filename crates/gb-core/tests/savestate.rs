//! Save-state round-trip tests.
//!
//! The strong guarantee we want: after restoring a state and running forward,
//! the machine produces *bit-identical* output to running forward from the
//! original point. If any persistent field is missing from the snapshot, the
//! two runs diverge and these tests fail.

use gb_core::GameBoy;
use std::path::PathBuf;

fn load(rel: &str) -> GameBoy {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/roms")
        .join(rel);
    GameBoy::new(std::fs::read(path).unwrap())
}

/// Run `frames` frames, returning the final framebuffer and all audio produced.
fn run(gb: &mut GameBoy, frames: u32) -> (Vec<u32>, Vec<i16>) {
    let mut audio = Vec::new();
    let mut fb = Vec::new();
    for _ in 0..frames {
        fb = gb.step_frame().to_vec();
        audio.extend(gb.take_audio());
    }
    (fb, audio)
}

fn roundtrip(rel: &str) {
    let mut gb = load(rel);
    run(&mut gb, 400); // warm up into meaningful state

    // Snapshot, then run a reference span forward.
    let state = gb.save_state();
    let (fb_ref, audio_ref) = run(&mut gb, 200);

    // A save taken at the same instant must be byte-identical (determinism).
    let mut gb2 = load(rel);
    run(&mut gb2, 400);
    assert_eq!(state, gb2.save_state(), "{rel}: save state is not deterministic");

    // Restore and run the same span; output must match bit-for-bit.
    assert!(gb2.load_state(&state), "{rel}: load_state rejected a valid state");
    let (fb_after, audio_after) = run(&mut gb2, 200);

    assert!(fb_ref == fb_after, "{rel}: video diverged after load_state");
    assert!(
        audio_ref == audio_after,
        "{rel}: audio diverged after load_state ({} vs {} samples)",
        audio_ref.len(),
        audio_after.len()
    );
}

#[test]
fn roundtrip_dmg_sound() {
    // Exercises APU channels, envelopes, length counters, timers.
    roundtrip("dmg_sound/02.gb");
}

#[test]
fn roundtrip_cgb() {
    // Exercises VRAM banking, CGB palettes, and CGB-mode CPU state.
    roundtrip("cgb-acid2.gbc");
}

#[test]
fn roundtrip_cpu() {
    roundtrip("cpu_instrs/06.gb");
}

#[test]
fn rejects_garbage() {
    let mut gb = load("cpu_instrs/01.gb");
    assert!(!gb.load_state(b"not a real state"));
    assert!(!gb.load_state(&[]));
}
