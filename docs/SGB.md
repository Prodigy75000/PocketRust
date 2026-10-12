# Super Game Boy

The SGB is a Super Nintendo cartridge with a Game Boy inside it. An enhanced
cartridge detects that it is running in one and sends commands to the SNES side,
which paints a decorative border around the picture and colours the screen.

## Where this stands

**Done for borders and palettes, and off by default.** Verified against the
owner's reference capture on Pokemon Blue, Kirby's Dream Land 2 and Donkey Kong
Land. Sound commands (`SOU_TRN`), multiplayer (`MLT_REQ` beyond answering it)
and the SNES-side custom programs some cartridges upload are not implemented.

Turn it on with the core option `pocketrust_sgb`. It is read **at load only**:
a cartridge probes for an SGB in its first frames and never asks again, so a
live toggle could not make it re-detect and would look like a broken control.
The frontend's label says "restart" for that reason, and a test pins both the
word and the fact that `off` is the first of the two choices, because libretro
treats the first as the default.

## How a command arrives

There is no bus. The cartridge pulses P14 and P15 of the joypad register at
`$FF00`: `$00` resets the bit stream, then each bit is clocked by pulling
exactly one line low (`$20` = 0, `$10` = 1) with `$30` between bits. Bits are
LSB first and 128 of them make one 16-byte packet. The first packet's header
byte is `command << 3 | length`, where length counts packets in the whole
command.

## The transfer is READ OFF THE SCREEN

This is the part that matters most, and everything painful about SGB support
comes back to it.

A `_TRN` command does not hand over memory. The cartridge **draws** 4 KiB onto
the display as ordinary tiles and the SNES reads it back off the scanlines. Pan
Docs describes the data as being "as originally stored at 8000-8FFF", which
reads like a guarantee about VRAM and is actually a description of the usual
setup. Reading `$8000` directly is the tempting shortcut and it is wrong the
moment LCDC's tile-data select points elsewhere: Game & Watch Gallery 2 runs
with `LCDC=$87`, so that shortcut transferred 4096 zeroes and turned its palette
black. The core rebuilds the payload from the rendered frame instead, keeping
every pixel's background colour index for the purpose.

The consequence is worth stating plainly: **any PPU rendering bug becomes data
corruption here.** One of them did. Turning the LCD off parked the PPU in
HBlank and nothing reset the mode when it came back on, so the PPU waited out
the rest of a line it had never drawn and advanced to line 1. Scanline 0 kept
the previous frame's pixels. That is close to invisible in a game and survived
for months, but a cartridge blanks the LCD to set up its transfer picture and
turns it back on, so row 0 of every captured tile carried the wrong bytes. In
Pokemon Blue it punched 20 black rectangles through the border and corrupted
`PAL_TRN` badly enough to turn the whole title screen red. Both symptoms, one
cause, and neither of them in the SGB code.

## Drawing the border

`CHR_TRN` carries the tile art, 256 tiles of 8x8 at 4 bits per pixel, in two
halves selected by bit 0 of its packet. `PCT_TRN` carries the 32x28 tilemap at
`$000-$6FF` and then **four** palettes at `$800-$87F`, numbered 4 to 7. Four,
not three: reading three and dropping every tile that named the fourth left
holes in Pokemon Blue's border.

A composed frame is 256x224 and each pixel comes from one of three places:

```text
  border colour index != 0   the border's own artwork
  index 0, inside 160x144    the Game Boy screen, at (48, 40)
  index 0, outside it        the backdrop
```

Colour 0 is transparent, as on any SNES 4bpp layer, and it has to stay that way:
Kirby's Dream Land 2 opens its screen window with transparent tiles rather than
by naming a non-border palette. The screen goes **underneath** rather than
having a hole punched for it, because a cartridge is free to draw border art
over the centre and several do on a title screen.

The backdrop is **not black**. The SGB drives the SNES backdrop from the Game
Boy palette so a border with transparent gaps blends into the picture. Pokemon
Blue depends on it: the white in its Pokeballs and its corner medallions is not
in the artwork at all.

## Geometry

The core sends real per-frame geometry, 160x144 without a border and 256x224
with one, and renegotiates with `RETRO_ENVIRONMENT_SET_GEOMETRY` (37) when a
border arrives partway into a boot. `retro_get_system_av_info` reports a maximum
of 256x224 always, so a frontend sizes its texture once and the later change
cannot be refused.

It never letterboxes. Bars written into the framebuffer become content: the
printer and camera paths read that buffer, so a screenshot would carry them, and
"native" aspect would be constrained to the wrong ratio. Bars the frontend draws
are layout and are the frontend's business.

## A Super Game Boy is a DMG, and that decides the machine

There is no console that is both a Super Game Boy and a Color Game Boy. So when
the option is on, a Color-capable cartridge is run as a **monochrome** Game Boy.

This is not a detail. A cartridge supporting both asks what it is running on,
takes its Color path when it finds a CGB, and only reaches its Super Game Boy
path otherwise. Report both and the game takes the Color path and never sends a
border. Pokemon Yellow did exactly that: SGB enabled, border enabled, four empty
palettes over an all-zero tilemap, because nothing had ever asked it for one.

Measured over the full 5344-cartridge set, deciding the machine correctly turns
**70 blank cartridges into rendering ones** with the option on.

The flag therefore has to be passed to `GameBoy::with_sgb` at construction: the
mode is threaded into the CPU and the MMU when they are built, and `set_sgb`
alone cannot undo it. `new()` plus `set_sgb(true)` is the shape of the bug.

Color-ONLY cartridges (`$C0` at `$0143`) are left as Color. They refuse to boot
on monochrome hardware, so a real SGB could not play them, and forcing DMG would
model a console that cannot run the cartridge rather than one that can.

**The trade is real and worth stating.** With the option on, 751 of the
cartridges in that set lose their Color output and gain a border plus SGB
palettes. That is what the hardware did, and it is why the option is off by
default.

## The mask, and how it is really cancelled

`MASK_EN` ($17) freezes, blackens or blanks the screen until cancelled, and it
exists because a transfer draws garbage on the display. A cartridge raises it,
transfers, and then lifts it.

The lift almost never comes as `MASK_EN 0`. It arrives as **bit 6 of a flags
byte in another command**: byte 9 of `PAL_SET`, or byte 1 of `ATTR_SET`. Pan
Docs, on `PAL_SET`: "If this bit is set, then any current MASK_EN screen freeze
is cancelled." Decoding only `MASK_EN` means waiting for a cancel that never
arrives, and the screen stays masked forever. That is exactly what happened:
applying the mask used to cost 99 cartridges, and Wario Land II, Tetris Blast,
Tetris Plus, Bomberman GB and Madden 96 were all games that raise a mask once
and never send `MASK_EN 0`.

With the cancel decoded the mask is applied unconditionally and costs nothing.
Measured over `dumps/roms`, 549 cartridges, 7 blank either way and the same
seven titles.

**A frame that ends without reaching line 144 still has to be masked.** The PPU
stops dead while the LCD is off, `step_frame` gives up on its cycle budget, and
the partly drawn frame is presented anyway. Applying the mask from the line-144
path alone left those frames unmasked, which is two frames of visible garbage in
Pokemon Blue's boot. It is applied at the end of `step_frame` instead.

Scored by counting horizontal colour changes per frame, which is a good proxy
for a transfer picture, Pokemon Blue's first 300 frames went from **14 frames
over 3000 edges to none**.

## Tools

```bash
# A composed frame as a PNG. 2600 frames gets most cartridges past their intro.
cargo run -p gb-runner --bin sgbborder -- "<rom>" out.png 2600

# Border palettes, tilemap palette histogram, and any holes in the artwork.
cargo run -p gb-runner --bin palprobe -- "<rom>"

# The cartridge's whole SGB command sequence, in order.
CMDLOG=1 cargo run -p gb-runner --bin palprobe -- "<rom>"

# The whole library, with and without SGB, to price the option.
SGB=1 cargo run -p gb-runner --release --bin smoke -- dumps/roms
```

## What it costs

Measured over the full 5344-cartridge set:

```
                       clean   blank   silent
SGB off (the default)   4818     154      470
SGB on                  4696     142      597
```

**It costs no blank screens at all; it removes twelve.** The option renders
slightly more of the library than the default does, which was not true before
the DMG rule above. What it costs is 127 cartridges moving into the smoke test's
"silent" bucket, which is a soft signal: a game that spends longer in its SGB
handshake and transfers may simply not reach its music inside the 900-frame
sample.

For the record, since the figure was quoted for a long time: applying MASK_EN
used to cost 99 cartridges and running SGB at all used to cost 52. Both numbers
were symptoms of bugs since fixed, not properties of the feature.
