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
| Cartridges | ✅ no-MBC, MBC1, MBC2, MBC3 (+ RTC), MBC5, MBC7, HuC1, HuC3 (+ RTC); battery-backed save RAM |
| Game Boy Camera | ✅ mapper, M64282FP sensor model and the libretro camera interface (`docs/CAMERA.md`). Point it at a lens and the cartridge develops a real photograph, with the in-game brightness and contrast sliders doing real work. Edge enhancement and analogue gain are not modelled, so photos are soft but correct |
| Rumble (MBC5) | ✅ the motor bit, and the RAM bank bit it steals on cart types `$1C-$1E` (`docs/RUMBLE.md`). Driven through the libretro rumble interface on the edge, never per frame. Measured on Pokemon Pinball at 74 motor edges a minute with pulses averaging 48 ms, and confirmed on two independent actuators, an Android vibrator and an iPhone Taptic Engine: bumper hits are felt as distinct hits rather than one smeared buzz |
| Tilt (MBC7) | ✅ two-axis accelerometer and 93LC56 EEPROM save (`docs/TILT.md`). Kirby Tilt 'n' Tumble calibrates, saves and rolls, steered by tilting the device through the libretro sensor interface; confirmed on hardware |
| MBC3 RTC | ✅ real-time clock (latch, halt, day carry); deterministic, cycle-driven; persists in save state and `.srm`. Pokemon Gold / Silver / Crystal and Harvest Moon |
| HuC3 clock + IR | ✅ a separate clock in HuC3's own format (minutes since midnight, days) behind its command mailbox, persisted in the `.srm` under its own magic. The IR port answers "no signal", so software polling it gets a quiet line rather than a reply that never comes. Robopon and Pocket Family boot |
| Colorization | ✅ the GBC boot ROM's own per-cartridge palette for monochrome games, byte-exact |
| Super Game Boy | ✅ command decoding over the joypad lines, VRAM transfers read off the rendered frame, palettes, per-tile attributes, and the **decorative border** composed into a 256x224 frame (`docs/SGB.md`). Opt-in via `pocketrust_sgb`, off by default and read at load only |
| Save states | ✅ full machine state, including video and audio; a state reloads exactly as it was saved |
| Link cable | ✅ serial transfer: local, TCP between two instances, and GameLink sessions over the libretro netpacket interface (`pocketrust-link-4`) |
| Game Boy Printer | ✅ full packet protocol on the link port, including RLE; pages joined on the printer's own margins and written as PNG (`docs/PRINTER.md`). Verified against Pokemon Yellow printing a Pokedex entry |
| GameLink transport | ✅ sequenced paired exchange, sub-frame polling, retransmit; byte-perfect through 1-in-3 packet loss |
| Demo cartridge | ✅ an original, CC0 Game Boy Color cartridge in `roms/pocketrust-demo/`, assembled by the SM83 assembler in this repo |
| Memory map | ✅ full descriptor table (work RAM, high RAM, VRAM, OAM, ROM bank 0, cart RAM, CGB banks 2-7) plus the legacy SYSTEM_RAM / SAVE_RAM ids, so achievements, cheats and RAM watch all address the core |

Compatibility: **5190 of 5344** GB / GBC ROMs (97.1%) boot and render, in a
headless smoke test over the full Game Boy and Game Boy Color SMDB sets. No
cartridge crashes the loader.

The 154 that do not render are almost entirely unlicensed: 118 unlicensed carts,
11 prototypes, 8 betas, 4 demos, a test cartridge, and two Pokemon bootlegs.
**Two licensed retail titles fail**, The Smurfs and Jim Henson's Bear in the Big
Blue House, both on mappers the core fully supports, so both are emulation bugs
rather than missing hardware.

A further 32 cartridges use a mapper the core does not implement. Thirty are
Sachen multicarts, unlicensed carts and copier tools; the two licensed ones are
Tamagotchi (TAMA5) and Net de Get (MBC6).

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
    tests/      Blargg + save-state + link-cable + demo-cart integration tests
  gb-runner/    minifb windowed frontend + headless compatibility smoke tester
  gb-libretro/  libretro core (builds the .so for RetroArch / libretro front-ends)
  gb-asm/       a dependency-free SM83 assembler, used to build the demo cart
roms/           the demo cartridge: source, ROM, licence, screenshots
tests/roms/     Blargg + acid2 test ROMs
```

## The demo cartridge

`roms/pocketrust-demo/` is an original Game Boy Color cartridge written for this
project and dedicated to the public domain under CC0 1.0, so it can be handed to
anyone, bundled with anything, and used to demonstrate the core without a
licensing question attached. It is five screens, each loading one part of the
machine hard enough to see whether it is right: the ten-objects-per-scanline
limit, a raster split, the 15-bit palette, all four sound channels, and the
joypad matrix.

```sh
scripts/build-demo-rom.sh                        # assemble it from source
cargo test -p gb-asm --test demo_rom_reproduces  # prove the ROM is that source
cargo test -p gb-core --test demo_cart           # drive the core with it
cargo run --release -p gb-runner -- roms/pocketrust-demo/pocketrust-demo.gbc
```

The chain from source to `.gbc` is entirely inside this repository: the
assembler is `crates/gb-asm/` and has no dependencies at all. See
[`roms/pocketrust-demo/README.md`](roms/pocketrust-demo/README.md).

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
single `cdylib` (one crate, one source of truth), loaded by `dlopen` + `dlsym`
on Android, desktop (Windows/macOS/Linux) and iOS. The output is named for the
libretro convention (`gbcore_libretro`), so the file is:

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

The core stamps the commit it was built from into its feature string:

```sh
strings -a gbcore_libretro.dll | grep -o 'build=[A-Za-z0-9.-]*'
# build=85f6a4ed3-local
```

### Android (`.so`)

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cp .cargo/config.toml.example .cargo/config.toml
# edit .cargo/config.toml: point the two `linker =` lines at your NDK's clang wrappers

cargo build --release -p gb-libretro --target aarch64-linux-android
cargo build --release -p gb-libretro --target armv7-linux-androideabi
# -> target/<triple>/release/libgbcore_libretro.so
```

The config pins `-Wl,-z,max-page-size=16384` so the `.so` is 16 KB-aligned, which
the Play Store requires for uploads targeting API 35+. Drop the resulting
`.so` into the app's `jniLibs/<abi>/`.

### iOS (`.dylib`, embedded in a co-signed `.framework`)

An iOS host that `dlopen`s cores from an embedded, co-signed framework needs a
plain `cdylib`; no `staticlib` and no code change are required.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

# Device:
cargo build --release -p gb-libretro --target aarch64-apple-ios
# Simulator (Apple Silicon host):
cargo build --release -p gb-libretro --target aarch64-apple-ios-sim
# -> target/<triple>/release/libgbcore_libretro.dylib
```

Then wrap the `.dylib` in a `.framework`, set its install name, and co-sign it
before embedding. Build on macOS with the Xcode command-line tools installed,
which provide the iOS linker.

## GameLink (link cable over the network)

GameLink is wired and requires **no core configuration**. The core implements
`RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE` (env 78) and hands the frontend a
callback struct on `retro_load_game`. When the host starts a **GameLink
session** it gives the core a `send`/`receive` pair, which the core bridges
straight to the Game Boy serial engine using the same 2-byte protocol as the
local/TCP transports.

A host that ignores env 78 simply gets a normal single-player core.

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE).
