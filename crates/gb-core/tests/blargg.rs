//! Integration tests driven by Blargg's test ROMs.
//!
//! Each ROM runs entirely headlessly: it reports its result by printing ASCII
//! to the serial port (which our MMU captures) and finishing with either
//! "Passed" or "Failed". We run the machine until that text appears.

use gb_core::GameBoy;
use std::path::PathBuf;

fn rom_path(rel: &str) -> PathBuf {
    // Tests run with the crate dir as CWD; ROMs live at the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/roms")
        .join(rel)
}

/// Run a serial-reporting test ROM until it prints "Passed" or "Failed",
/// or we hit the frame budget. Returns the accumulated serial text.
fn run_serial_rom(rel: &str, max_frames: u32) -> String {
    let rom = std::fs::read(rom_path(rel))
        .unwrap_or_else(|e| panic!("failed to read {rel}: {e}"));
    let mut gb = GameBoy::new(rom);
    let mut out = String::new();
    for _ in 0..max_frames {
        gb.step_frame();
        let bytes = gb.take_serial();
        if !bytes.is_empty() {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
        if out.contains("Passed") || out.contains("Failed") {
            break;
        }
    }
    out
}

fn check(rel: &str) {
    let out = run_serial_rom(rel, 4000);
    assert!(
        out.contains("Passed"),
        "ROM {rel} did not pass.\nSerial output:\n{out}"
    );
}

/// Some Blargg ROMs (dmg_sound) report only through the in-memory protocol:
/// a `DE B0 61` signature at 0xA001, a result byte at 0xA000 (0x80 = running,
/// 0 = pass), and the message text from 0xA004.
fn check_mem(rel: &str) {
    let rom = std::fs::read(rom_path(rel))
        .unwrap_or_else(|e| panic!("failed to read {rel}: {e}"));
    let mut gb = GameBoy::new(rom);
    for _ in 0..4000 {
        gb.step_frame();
        let has_sig =
            gb.peek(0xA001) == 0xDE && gb.peek(0xA002) == 0xB0 && gb.peek(0xA003) == 0x61;
        if has_sig && gb.peek(0xA000) != 0x80 {
            let result = gb.peek(0xA000);
            let mut text = String::new();
            for a in 0xA004u16..0xA100 {
                let b = gb.peek(a);
                if b == 0 {
                    break;
                }
                text.push(b as char);
            }
            assert_eq!(result, 0, "ROM {rel} failed (code {result}):\n{text}");
            return;
        }
    }
    panic!("ROM {rel} did not finish within the frame budget");
}

#[test]
fn cpu_01_special() {
    check("cpu_instrs/01.gb");
}
#[test]
fn cpu_02_interrupts() {
    check("cpu_instrs/02.gb");
}
#[test]
fn cpu_03_op_sp_hl() {
    check("cpu_instrs/03.gb");
}
#[test]
fn cpu_04_op_r_imm() {
    check("cpu_instrs/04.gb");
}
#[test]
fn cpu_05_op_rp() {
    check("cpu_instrs/05.gb");
}
#[test]
fn cpu_06_ld_r_r() {
    check("cpu_instrs/06.gb");
}
#[test]
fn cpu_07_jr_jp_call_ret_rst() {
    check("cpu_instrs/07.gb");
}
#[test]
fn cpu_08_misc() {
    check("cpu_instrs/08.gb");
}
#[test]
fn cpu_09_op_r_r() {
    check("cpu_instrs/09.gb");
}
#[test]
fn cpu_10_bit_ops() {
    check("cpu_instrs/10.gb");
}
#[test]
fn cpu_11_op_a_hl() {
    check("cpu_instrs/11.gb");
}

#[test]
fn instr_timing() {
    check("instr_timing.gb");
}

// --- APU behaviour (Blargg dmg_sound) ---------------------------------------
// These validate register masks, length counters, triggers, sweep and the
// power-off behaviour of the sound unit.

#[test]
fn sound_01_registers() {
    check_mem("dmg_sound/01.gb");
}
#[test]
fn sound_02_len_ctr() {
    check_mem("dmg_sound/02.gb");
}
#[test]
fn sound_03_trigger() {
    check_mem("dmg_sound/03.gb");
}
#[test]
fn sound_04_sweep() {
    check_mem("dmg_sound/04.gb");
}
#[test]
fn sound_08_len_ctr_during_power() {
    check_mem("dmg_sound/08.gb");
}
#[test]
fn sound_11_regs_after_power() {
    check_mem("dmg_sound/11.gb");
}
