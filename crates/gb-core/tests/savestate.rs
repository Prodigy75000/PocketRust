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

/// Build a synthetic MBC3+TIMER cartridge whose boot code programs the RTC to a
/// known, halted time (30 s, hour 7) and latches it, then spins. This lets us
/// drive the real CPU -> MMU -> cartridge RTC path without a commercial ROM.
fn rtc_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x10; // MBC3+TIMER+RAM+BATTERY
    rom[0x0148] = 0x00; // 32 KiB
    rom[0x0149] = 0x02; // 8 KiB RAM

    // Entry at 0x0100: jump over the header to the program at 0x0150.
    rom[0x0100] = 0x00; // NOP
    rom[0x0101] = 0xC3; // JP 0x0150
    rom[0x0102] = 0x50;
    rom[0x0103] = 0x01;

    // Hand-assembled program: A=v; LD (addr),A is `3E vv EA lo hi`.
    let mut p = 0x0150;
    let mut emit = |rom: &mut [u8], val: u8, addr: u16| {
        rom[p] = 0x3E;
        rom[p + 1] = val;
        rom[p + 2] = 0xEA;
        rom[p + 3] = (addr & 0xFF) as u8;
        rom[p + 4] = (addr >> 8) as u8;
        p += 5;
    };
    emit(&mut rom, 0x0A, 0x0000); // enable RAM/RTC
    emit(&mut rom, 0x08, 0x4000); // select seconds register
    emit(&mut rom, 30, 0xA000); // seconds = 30
    emit(&mut rom, 0x0C, 0x4000); // select DH
    emit(&mut rom, 0x40, 0xA000); // HALT the clock (freeze the time)
    emit(&mut rom, 0x0A, 0x4000); // select hours register
    emit(&mut rom, 7, 0xA000); // hours = 7
    emit(&mut rom, 0x00, 0x6000); // latch step 0
    emit(&mut rom, 0x01, 0x6000); // latch step 1 -> snapshot the time
    emit(&mut rom, 0x08, 0x4000); // re-select seconds for read-back
    rom[p] = 0x18; // JR -2 (spin)
    rom[p + 1] = 0xFE;
    rom
}

#[test]
fn rtc_survives_save_state() {
    let rom = rtc_rom();
    let mut gb = GameBoy::new(rom.clone());
    run(&mut gb, 3); // let the boot code program and latch the clock

    // Seconds register is selected and latched to 30.
    assert_eq!(gb.peek(0xA000), 30, "boot code should latch seconds = 30");

    let state = gb.save_state();

    // Restore into a fresh machine; the latched RTC time must come back.
    let mut gb2 = GameBoy::new(rom);
    assert!(gb2.load_state(&state));
    assert_eq!(gb2.peek(0xA000), 30, "RTC seconds lost across save state");
    assert_eq!(gb2.peek(0xA000), 30, "RTC read is side-effect free");
}
