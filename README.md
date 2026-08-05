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
| Cartridges | ✅ no-MBC, MBC1, MBC2, MBC3 (+ RTC), MBC5, HuC1, HuC3 (+ RTC); battery-backed save RAM |
| MBC3 RTC | ✅ real-time clock (latch, halt, day carry); deterministic, cycle-driven; persists in save state and `.srm` |
| Colorization | ✅ automatic GBC-style palette for monochrome games |
| Save states | ✅ full machine state, bit-identical round-trip (video + audio) |
| Link cable | ✅ serial transfer: local, TCP between two instances, and networked play over the libretro netpacket interface (`pocketrust-link-4`) |
| Networked link | ✅ sequenced paired exchange, sub-frame polling, retransmit — byte-perfect through 1-in-3 packet loss |

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

`gb-libretro` is a standard libretro core (the full `retro_*` C ABI). It builds a
single `cdylib` — one crate, one source of truth — that every Trophy Hub client
loads by `dlopen` + `dlsym`: Android, desktop (Windows/macOS/Linux) and iOS. The
output is named for the libretro convention (`gbcore_libretro`), so the file is:

| Platform | Target triple | Output file |
|----------|---------------|-------------|
| Linux    | host          | `libgbcore_libretro.so` |
| Windows  | host          | `gbcore_libretro.dll` |
| macOS    | host          | `libgbcore_libretro.dylib` |
| Android arm64 | `aarch64-linux-android` | `libgbcore_libretro.so` |
| Android arm32 | `armv7-linux-androideabi` | `libgbcore_libretro.so` |
| iOS device    | `aarch64-apple-ios` | `libgbcore_libretro.dylib` |
| iOS simulator (Apple Silicon) | `aarch64-apple-ios-sim` | `libgbcore_libretro.dylib` |
| iOS simulator (Intel)         | `x86_64-apple-ios`      | `libgbcore_libretro.dylib` |

All builds are release + LTO (`[profile.release]` in the workspace `Cargo.toml`).
The library carries no non-Rust dependencies, so cross-compiling only needs a
linker for the target.

### Desktop (Windows `.dll`, macOS `.dylib`, Linux `.so`)

```sh
cargo build --release -p gb-libretro
# -> target/release/{libgbcore_libretro.so | gbcore_libretro.dll | libgbcore_libretro.dylib}
```

Build on the OS you are targeting (or with the matching `--target`). No config
file is needed for host builds.

### Android (`.so`)

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cp .cargo/config.toml.example .cargo/config.toml
# edit .cargo/config.toml: point the two `linker =` lines at your NDK's clang wrappers

cargo build --release -p gb-libretro --target aarch64-linux-android
cargo build --release -p gb-libretro --target armv7-linux-androideabi
# -> target/<triple>/release/libgbcore_libretro.so
```

The config pins `-Wl,-z,max-page-size=16384` so the `.so` is 16 KB-aligned —
required or the Play Store blocks uploads targeting API 35+. Drop the resulting
`.so` into the app's `jniLibs/<abi>/`.

### iOS (`.dylib`, embedded in a co-signed `.framework`)

The iOS host `dlopen`s each core from an embedded, co-signed framework (dlsym
loader — same path Gambatte takes), so the core is a plain `cdylib`; no
`staticlib` and no code change are needed.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

# Device:
cargo build --release -p gb-libretro --target aarch64-apple-ios
# Simulator (Apple Silicon host):
cargo build --release -p gb-libretro --target aarch64-apple-ios-sim
# -> target/<triple>/release/libgbcore_libretro.dylib
```

Then wrap the `.dylib` in a `.framework`, set its install name, and co-sign it
before embedding — exactly as the existing Gambatte core is packaged. Build on
macOS with the Xcode command-line tools installed (provides the iOS linker).

## GameLink (link cable over the network)

GameLink is wired and requires **no core configuration**. The core implements
`RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE` (env 78) and hands the frontend a
callback struct on `retro_load_game`. When the host starts a netplay session it
gives the core a `send`/`receive` pair, which the core bridges straight to the
Game Boy serial engine using the same 2-byte protocol as the local/TCP
transports. A host that ignores env 78 simply gets a normal single-player core.
This replaces the old gambatte link path; the netpacket transport is the one
Trophy Hub drives on every platform.

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE).
