# MBC5 rumble

Cart types `$1C`, `$1D` and `$1E`: MBC5 with a motor. 43 cartridges in a
5341-cartridge library, Pokemon Pinball, Perfect Dark and Star Wars Episode I
Racer among them.

## Where this stands

**Done, and confirmed on hardware.** The mapper, the libretro rumble interface,
the host's actuator hook and Android's vibrator. The owner played Pokemon
Pinball on his phone and felt the bumpers.

## The bit that is not a bit

The motor is bit 3 of the RAM bank register at `$4000-$5FFF`, and on these
cartridges **it stops being an address line**. That is the whole feature and
also its one hazard: the same write means different things on two cartridges
with the same mapper.

```text
  rumble cart ($1C-$1E)   bit 3 = motor, bits 0-2 = RAM bank
  plain MBC5  ($19-$1B)   bits 0-3 = RAM bank
```

Getting it wrong is bad in both directions. Treat a plain MBC5 as a rumble cart
and you silently halve its RAM, Pokemon Yellow included. Treat a rumble cart as
plain and bank 0 aliases onto bank 8 every time the game buzzes, which on a
small cartridge is a save scribbling on itself whenever the ball hits a bumper.

## The motor is an output, not state

It is deliberately **absent from the save state**. Nothing in the emulator reads
it back, and serializing it would move a byte in every MBC5 save state that
already exists to carry a bit nobody needs. A state taken mid-buzz restores
quiet and the game sets it again on its next bank write.

## The frontend contract

`RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE` is **23**, no experimental bit, one
numbering with no history to it.

**Edges only, never per frame.** The core sends one call when the motor starts
and one when it stops, with an explicit zero on unload and on reset.

**Both `STRONG` and `WEAK`, same value.** libretro models a gamepad's two
motors; the Game Boy has one, so picking either alone would be a guess about
hardware the core cannot see. **Model it as an amplitude, not a pulse** —
the pair then arrives as a set and a no-op. Modelled as a pulse it fires twice
per bumper.

### The timing, which decides the whole design

Measured on Pokemon Pinball with the ball actually in play: **74 edges and 107
buzzing frames out of 3600**, so pulses average about 48 ms and the shortest are
one or two frames, 17 to 33 ms.

That is short enough to fall through a typical haptic API. **Nothing in the
chain debounces, smooths or imposes a minimum duration**, because any of those
swallows the short pulses and the result presents as "rumble feels mushy",
which is untraceable.

TH-Android's shape is the one that survives it: start a **long** vibration on
the ON edge and **cancel** it on the OFF edge, so the pulse length is decided by
when the cancel lands and the platform's minimum-duration behaviour is never
consulted. Reaching for a 20 ms one-shot is the obvious first instinct and is
exactly the case that renders badly.

Confirmed on hardware: bumper hits are felt as distinct hits rather than one
smeared buzz.

### Gate on the device, not the cartridge

This is the opposite of tilt, and the difference is worth stating because
applying the tilt advice here is a natural mistake (TH-Android made it and
caught it).

Tilt is a **push**: the frontend must decide when to wake a sensor, so it needs
to know which cartridges want one. Rumble is a **pull**: nothing moves until the
core calls `set_rumble_state`, so **the core already holds the only opinion
needed** about which cartridges rumble. A cartridge gate in the frontend is
redundant, and worse, a Game-Boy-cart-type gate silently withholds rumble from
every other core using the same hook, mupen's Rumble Pak included.

The gate that matters on the frontend is whether the **device** has an actuator.

### A refusal is the only feedback there is

`set_rumble_state` returning false means the frontend cannot do this effect, and
false is expected rather than an error to retry or give up over.

It is also the entire diagnostic channel, and that is an asymmetry worth
understanding. For the camera and the tilt sensor the core can test the **data**
and catch a frontend that advertised something it could not deliver; that is how
a dead accelerometer feed was caught. A motor returns a bool and nothing else,
so **a frontend that answers true and then does not move makes "my phone is not
buzzing" permanently undiagnosable.** Installing the actuator hook is therefore
the honest bool, with no separate "present" flag to drift out of step with it.

The core says so once per **device**, not once per game: the answer describes
the device and a device does not change when a cartridge does. Saying it per
load would nag forever on a tablet with no vibrator, which is a real
configuration and the owner's primary one.

The wording deliberately does not name a cause. A refusal cannot distinguish a
device with no vibrator from a frontend that never wired one up, and the second
is a bug somebody would want reported rather than explained away.

## Testing

`crates/gb-core/tests/rumble.rs`. The mapper tests build **synthetic
cartridges** rather than using real ones, and that is not a shortcut.

The first version of that file used Pokemon Pinball and Pokemon Yellow and was
worthless in two independent ways. The Yellow path was wrong by a few words of
filename so every test using it skipped while reporting pass. And Pokemon
Pinball declares **one** 8 KiB RAM bank, so `bank % banks` folds every bank onto
bank 0 and no aliasing test can observe anything on it. Deliberately breaking
the mapper to treat all MBC5 carts as rumble carts passed all five tests.

A synthetic cartridge always exists and can be given sixteen RAM banks, which
makes bank 0 and bank 8 distinct memory and the aliasing observable.

Two things still need the real ROM, and both are questions only a real cartridge
answers: that `$1E` is what the games people own actually use, and that a real
game drives the motor at all. The second nearly read as a broken implementation:
sixty seconds of Pokemon Pinball produced zero edges because the ball was
sitting in the plunger lane. A pinball table with no ball in play is not a test
of anything.
