# PocketRust

A clean-room Game Boy and Game Boy Color emulator core written from scratch in Rust.
No C, no bindings, and no lifted code, just the hardware modeled from the docs.
The core passes major CPU timing and graphics compatibility tests and runs most commercial Game Boy and Game Boy Color titles.

## Status

| Component | State |
|-----------|-------|
| CPU (Sharp LR35902) | ✅ all 256 + 256 CB opcodes, interrupts, HALT/EI quirks, DAA |
| CPU accuracy | ✅ Blargg `cpu_instrs` 1-11 + `instr_timing` pass; timed at M-cycle granularity |
| PPU (DMG + CGB) | ✅ background, window, sprites (8x8 / 8x16), priority; `dmg-acid2` + `cgb-acid2` |
| Audio (APU) | ✅ all four channels (2 pulse, wave, noise), stereo; Blargg `dmg_sound` tests pass |
| Timer | ✅ shared 16-bit divider model (DIV / TIMA / TMA / TAC) |
| Interrupts | ✅ VBlank, STAT, Timer, Serial, Joypad |
| Cartridges | ✅ no-MBC, MBC1, MBC2, MBC3, MBC5; battery-backed save RAM |
| Colorization | ✅ automatic GBC-style palette for monochrome games |
| Save states | ✅ full machine state, bit-identical round-trip (video + audio) |
| Link cable | ✅ serial transfer, local and over TCP between two instances |

Compatibility: **4577 of 4794** GB / GBC ROMs (95.5%) boot and render in a
headless smoke test of a large No-Intro-style set. The remaining misses are a
handful of Hudson (HuC1 / HuC3) and other rare mappers, plus a few edge cases.

## How it is timed

The CPU advances the PPU, timer, and APU on every memory access (one M-cycle at
a time) rather than once per instruction, so tight polling loops observe the same
mid-instruction hardware state real hardware would. Instruction totals stay exact,
which is what keeps `instr_timing` green.

## Layout

```
crates/
  gb-core/      the emulator library (no I/O deps)
    src/cpu/    registers, decoder, execution
    src/{mmu, ppu, apu, timer, joypad, cartridge, serial}.rs
    tests/      Blargg + save-state + link-cable integration tests
  gb-runner/    minifb windowed frontend + headless compatibility smoke tester
  gb-libretro/  libretro core (builds the .so for RetroArch / libretro front-ends)
tests/roms/     Blargg + acid2 test ROMs
```

## Running

```sh
# Play a game in a window
#   arrows = D-pad, Z = A, X = B, Enter = Start, RShift = Select, R = reset, Esc = quit
cargo run --release -p gb-runner -- path/to/rom.gb

# Two-player link cable over the network
cargo run --release -p gb-runner -- game.gb --link-listen 5000        # host
cargo run --release -p gb-runner -- game.gb --link-connect host:5000  # peer

# Run the accuracy test suite
cargo test --release

# Batch-boot a folder of ROMs and report anything that fails to render
cargo run --release -p gb-runner --bin smoke -- path/to/roms/
```

## libretro core

```sh
# Native build -> target/release/libgbcore_libretro.{so,dll,dylib}
cargo build --release -p gb-libretro
```

To cross-compile the Android `.so`, copy `.cargo/config.toml.example` to
`.cargo/config.toml`, point the linker lines at your NDK install, and build with
`--target aarch64-linux-android`.

## License

To be decided.
