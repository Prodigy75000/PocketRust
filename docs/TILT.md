# MBC7: the tilt cartridges

Cart type `$22`. Two accessories in one mapper: a two-axis accelerometer and a
93LC56 serial EEPROM standing in for the cartridge RAM. Four commercial games
use it, and the one anybody wants is **Kirby Tilt 'n' Tumble**.

Nothing here is shared with the printer or the Camera. It is a mapper, not a
link accessory, and it needs no host support beyond an accelerometer.

## Where this stands

**Done.** The mapper, the accelerometer, and the EEPROM. Kirby boots, runs its
calibration screen, reads and writes save files, and rolls in both axes.

**Done.** The libretro side: the sensor interface, with the left analog stick as
a fallback so the cartridge is playable on a controller or a desktop frontend.

**Unverified.** The sign convention of the phone's accelerometer, which is
derived rather than measured. See "If it plays backwards" below. Everything
else in this document was measured against the cartridge.

## The mapper

```text
  $0000-$1FFF   write $0A: RAM enable 1
  $2000-$3FFF   ROM bank, $00-$7F
  $4000-$5FFF   write $40: RAM enable 2
  $A000-$BFFF   registers, mirrored every $10 bytes
```

Both enables are required together. That is unusual enough to be worth saying
twice: one alone leaves the whole `$A000` window reading `$FF`.

The register is chosen by the **second nibble of the address**, so `$A020`,
`$A02F` and `$B12F` are all the X low byte:

```text
  Ax0x   write $55   erase the latched reading, back to $8000
  Ax1x   write $AA   latch the current reading
  Ax2x   X low       Ax3x  X high
  Ax4x   Y low       Ax5x  Y high
  Ax6x   always $00  Ax7x  always $FF
  Ax8x   EEPROM      bit 7 CS, bit 6 CLK, bit 1 DI, bit 0 DO
```

The latch cannot be re-armed without erasing first, so `$55` then `$AA` is the
whole protocol and a stray `$AA` does nothing.

## Three numbers that each cost an afternoon

Every one of these leaves a mapper that traces perfectly and a game that does
not play.

**Resting is `$81D0`. `$8000` is only the erased value.** This is the expensive
one, because `$8000` is right there in the spec next to the latch command and
reads like the centre. The two differ by 464 counts and one g is about `$70`,
so using `$8000` for latched readings puts every sample more than four g from
where the game calibrated. Kirby then polls the sensor about once a frame, reads
back exactly the numbers it was handed, discards them all as impossible, and
sits still. There is nothing anomalous anywhere in the trace.

**The register's sign is the opposite of the screen's.** Driving X *below* rest
rolls the ball *right*. `set_tilt` takes screen coordinates instead, so `+x` is
right and `+y` is down, because a core that is provably working and plays
backwards is worse than one that is obviously broken.

**A fresh EEPROM is zeroed, not erased to ones.** An erased 93LC56 reads `$FF`
and that is what the part really does, but fill it with `$FF` and Kirby's file
select shows three fabricated saves reading `LEVEL 8-4 255%`, with no option to
format them. Fill it with `$00` and it shows `NO DATA`. So a real cartridge
cannot have shipped reading `$FF`, and beyond looking wrong, an invented
100%-complete file is exactly the kind of garbage state that false-unlocks
achievements.

## The save is 256 bytes

The header declares **no cartridge RAM at all**; the save *is* the EEPROM, 128
words of 16 bits, stored big-endian. The obvious fallback allocates one 8 KiB
bank, and that file will not load in any other emulator of this mapper. It is
sized exactly for that reason.

## The frontend contract

**Use the standard sensor interface.**
`RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE` is `25 | RETRO_ENVIRONMENT_EXPERIMENTAL`
= `$10019`, per current libretro.h.

This command has **two numberings in the wild**, and our own host's header
comments have them the opposite way round from upstream: it calls
`21 | EXPERIMENTAL` (`$10015`) the modern value. It does not bite, because the
host matches both, but do not resolve the disagreement by copying a neighbour.
Bare `21` and bare `25` are different commands whose payloads are structurally
unrelated; the host has already been bitten by aliasing bare 21 and landing on
`GET_INPUT_DEVICE_CAPABILITIES`, whose payload is a `uint64_t *`.

**It is a PULL, and that is the opposite of the camera.** The camera pushes
frames at the core. Here the frontend hands over one function and the core calls
it once a frame. Nothing to latch, no re-entrancy.

**The sensor is only started for a cartridge that has one.** Registering does
not turn anything on. Nobody should have their phone's sensors woken up because
they loaded Pokemon.

**Units are g, and the axes are the device's.** X rightward, Y forward, as
Android reports them. The core converts; the frontend should not.

**There is a fallback and it is automatic.** With no sensor, the core reads the
left analog stick at one g full deflection, so the cartridge is playable on a
controller, on a desktop frontend, or on a tablet in a stand. The core says
which one the player got, on load, via `SET_MESSAGE`.

## If it plays backwards

The one part of this that is derived rather than measured. An accelerometer
reports **specific force**, so at rest it reads the reaction to gravity pointing
*up*, not gravity pointing down. Writing `x_d` and `y_d` for the device's
rightward and up-the-screen axes:

```text
  sensor X = f . x_d        sensor Y = f . y_d
```

The ball rolls toward the lowered edge, along `-f` projected into the screen
plane, so it rolls along `(-X, -Y)` in device axes. The core's `+y` is **down**
and the device's `y_d` is **up**, and that flip cancels one of the two
negations:

```text
  core x = -sensor X        core y = +sensor Y
```

Which looks asymmetric and is not. Both signs live in `crates/gb-libretro/src/sensor.rs`
and nowhere else, so if Kirby rolls the wrong way on a real device, that is a
two-character fix in one file.

**The test, on hardware: lower the right-hand edge. Kirby must roll right.**
That needs no agreement about what "forward" means and no reasoning about
reaction forces.

## Testing

`crates/gb-core/tests/mbc7.rs`, skipped when the ROM is absent since a
commercial cartridge cannot be committed. Put it at
`dumps/roms/Kirby - Tilt 'n' Tumble (USA).gbc`.

The tests drive the game through its own calibration screen and into a level,
because that is the path that exercises the mapper: the EEPROM is read at the
file select and the accelerometer is latched about once a frame in play. A test
that only checked the cartridge boots would have passed throughout all three
bugs above.

Each was checked against a deliberately broken core before being trusted: the
direction test fails on a flipped sign, the latch test fails on `$8000` as the
centre, and the savestate tests fail if the accelerometer is left out of the
serializer. Resolving the ROM through `CARGO_MANIFEST_DIR` rather than the cwd
matters too, or every test skips while reporting pass.

`cargo run -p gb-runner --bin tiltprobe -- <rom> <script>` sweeps the axes and
prints where the game ends up. Measure the **scroll registers**, not a sprite:
Kirby stays nailed to the centre of the screen while the world moves under him,
so sprite positions look frozen at every tilt and the obvious measurement says
the sensor is dead.
