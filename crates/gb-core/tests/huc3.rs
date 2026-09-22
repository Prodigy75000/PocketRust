// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! HuC3: Hudson's mapper, with its own real-time clock and an IR port.
//!
//! Not MBC3. The two are easy to conflate because both are "the Game Boy
//! mapper with a clock", but the clocks share no format: MBC3 counts
//! seconds/minutes/hours/days and latches a snapshot, HuC3 counts minutes
//! since midnight and days, read through a command mailbox. Pokemon Crystal is
//! MBC3 (`$10`); HuC3 (`$FE`) is Robopon and Pocket Family.

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

/// A minimal HuC3 cartridge. Synthetic so the test always runs: the real ones
/// are commercial and cannot be committed.
fn huc3_cart() -> GameBoy {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0134..0x0140].copy_from_slice(b"HUC3 FIXTURE");
    rom[0x0147] = 0xFE; // HuC3
    rom[0x0148] = 0x00; // 32 KiB ROM
    rom[0x0149] = 0x03; // 32 KiB RAM, 4 banks
    GameBoy::new(rom)
}

/// Long enough for the minute counter to move, since the footer is only
/// refreshed when the displayed time changes.
fn run_a_few_minutes(gb: &mut GameBoy) {
    for _ in 0..(60 * 60 * 4) {
        gb.step_frame();
    }
}

#[test]
fn the_clock_survives_a_battery_save() {
    // The bug this is here for: HuC3's clock ticked correctly and survived a
    // save state, but had no battery footer at all, so quitting and coming
    // back reset it to zero. On hardware that clock is battery-backed like
    // MBC3's. Robopon runs real-time events off it.
    let mut gb = huc3_cart();
    run_a_few_minutes(&mut gb);
    let saved = gb.sram().to_vec();

    // A footer is appended past the game-visible RAM, so the save is longer
    // than the declared 32 KiB.
    assert!(
        saved.len() > 4 * 0x2000,
        "no clock footer in the battery save: got {} bytes",
        saved.len()
    );
    assert_eq!(&saved[4 * 0x2000..4 * 0x2000 + 4], b"PHU3");

    // Power-cycle: a fresh machine, the old battery file, and the clock must
    // come back where it was rather than at zero.
    let mut fresh = huc3_cart();
    fresh.load_sram(&saved);
    fresh.restore_rtc();
    assert_eq!(
        fresh.sram()[4 * 0x2000..],
        saved[4 * 0x2000..],
        "the restored clock does not match the saved one"
    );
}

#[test]
fn an_mbc3_footer_is_never_read_as_a_huc3_one() {
    // Both footers are the same length and sit at the same offset, so the
    // magic is the only thing keeping them apart. Read the wrong way round,
    // MBC3's day counter would land in HuC3's, and a cartridge would wake up
    // hundreds of days in the future.
    const OFF: usize = 4 * 0x2000;

    // A well-formed MBC3 footer claiming day 500.
    let mut save = vec![0u8; OFF + 15];
    save[OFF..OFF + 4].copy_from_slice(b"PRTC");
    save[OFF + 4] = 1;
    save[OFF + 8..OFF + 10].copy_from_slice(&500u16.to_le_bytes());

    let mut gb = huc3_cart();
    gb.load_sram(&save);
    gb.restore_rtc();

    // Run until HuC3 writes its own footer over the foreign one, then read the
    // day count back out of it. Decoding the MBC3 footer would have given 500.
    run_a_few_minutes(&mut gb);
    let now = gb.sram().to_vec();
    assert_eq!(&now[OFF..OFF + 4], b"PHU3", "HuC3 must write its own magic");
    let days = u16::from_le_bytes([now[OFF + 7], now[OFF + 8]]);
    assert_eq!(days, 0, "HuC3 adopted a day count from an MBC3 footer");
}

#[test]
fn a_save_with_no_footer_still_loads() {
    // Every HuC3 .srm written before this existed is exactly 32 KiB. Those
    // must keep working, with the clock simply starting at power-on, which is
    // the behaviour this replaced rather than a regression.
    let mut gb = huc3_cart();
    let legacy = vec![0xABu8; 4 * 0x2000];
    gb.load_sram(&legacy);
    gb.restore_rtc();
    assert_eq!(&gb.sram()[..legacy.len()], &legacy[..]);
}

#[test]
fn the_ir_port_reports_no_signal_rather_than_nothing() {
    // HuC3 window mode $0E is the infrared port. We have no IR partner and are
    // not going to, so it answers "no signal" forever. That is deliberate: a
    // register that reads back whatever was written, or floating $FF, is how
    // software ends up polling for a reply that never comes.
    let mut gb = huc3_cart();
    gb.poke(0x0000, 0x0E); // select the IR window
    assert_eq!(gb.peek(0xA000), 0x00, "IR must read as a quiet line");
}

#[test]
fn the_robopon_cartridges_boot() {
    // The one thing only real cartridges answer: that $FE is what they use and
    // that the mailbox is enough to get them past their own init.
    let names = [
        "Robopon - Sun Version (USA) (SGB Enhanced) (GB Compatible).gbc",
        "Pocket Family GB 2 (Japan).gbc",
    ];
    for name in names {
        let Some(path) = find(name) else { continue };
        let rom = std::fs::read(&path).unwrap();
        assert_eq!(rom[0x0147], 0xFE, "{name} should be HuC3");
        let mut gb = GameBoy::new(rom);
        for _ in 0..900 {
            gb.step_frame();
        }
        let shades = gb
            .framebuffer()
            .iter()
            .map(|p| *p & 0x00FF_FFFF)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            shades.len() > 3,
            "{name} drew {} distinct colours, so it did not reach a title screen",
            shades.len()
        );
    }
}

fn find(name: &str) -> Option<PathBuf> {
    fn walk(dir: &PathBuf, name: &str, out: &mut Option<PathBuf>) {
        if out.is_some() {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, name, out);
            } else if p.file_name().is_some_and(|f| f == name) {
                *out = Some(p);
                return;
            }
        }
    }
    let mut out = None;
    walk(&repo("dumps"), name, &mut out);
    if out.is_none() {
        eprintln!("skipping: needs {name}");
    }
    out
}
