# RustGameBoy

A clean-room Game Boy (DMG) emulator core, written from scratch in Rust. No C,
no bindings, no lifted code — just the hardware, modelled from the docs.

Built for fun. The CPU passes Blargg's full `cpu_instrs` suite *and* `instr_timing`,
and the PPU passes `dmg-acid2`.

## Status

| Component | State |
|-----------|-------|
| CPU (Sharp LR35902) | ✅ all 256 + 256 CB opcodes, interrupts, HALT/EI quirks, DAA |
| CPU accuracy | ✅ Blargg `cpu_instrs` 1–11 pass, `instr_timing` passes |
| PPU | ✅ background, window, sprites (8×8 / 8×16), priority, `dmg-acid2` passes |
| Timer (DIV/TIMA) | ✅ shared 16-bit divider model |
| Interrupts | ✅ VBlank, STAT, Timer, Serial, Joypad |
| Cartridges | ✅ no-MBC, MBC1, MBC3, MBC5 |
| Input | ✅ full joypad matrix |
| Audio (APU) | ⬜ not yet |
| Save RAM persistence | ⬜ not yet (battery RAM is in memory only) |

Boots to the title screen: **Pokémon Red**, **dmg-acid2**. (Tobu Tobu Girl DX
runs but currently self-blanks the BG — a known follow-up.)

## Layout

```
crates/
  gb-core/     the emulator library (no I/O deps)
    src/cpu/   registers, decoder, execution
    src/{mmu, ppu, timer, joypad, cartridge}.rs
    tests/     Blargg test-ROM integration tests
  gb-runner/   minifb window frontend + headless PNG screenshotter
tests/roms/    Blargg + acid2 test ROMs
```

## Running

```sh
# Play a game in a window (Z=A, X=B, Enter=Start, RShift=Select, arrows=D-pad)
cargo run --release -p gb-runner -- path/to/rom.gb

# Headless screenshot after N frames
cargo run --release -p gb-runner --bin shot -- path/to/rom.gb 600 out.png

# Run the accuracy test suite
cargo test --release
```
