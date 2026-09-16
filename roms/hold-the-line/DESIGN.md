<!--
SPDX-License-Identifier: CC0-1.0
HOLD THE LINE. Dedicated to the public domain; see LICENSE.
-->

# HOLD THE LINE, design

An element-drafting tower defence for the Game Boy Color, for one player or for
two over a link cable. Original work, CC0, built by the assembler in this
repository.

The title is the tower defence phrase and the cable at the same time.

## The one sentence

You do not buy towers. You earn one element per wave, and the elements you have
decide what you are allowed to build.

That is the whole identity of the game and everything below is in service of it.
Placement matters, but the interesting decision is always the draft: taking a
second Fire now to unlock the tower you want, or taking a Water you do not need
yet because the wave after next is going to hurt.

## Four elements

Fire, Water, Earth, Nature. Four, not six, for two reasons that both matter.
Four maps exactly onto the d-pad, so the draft is one unambiguous press with no
cursor. And four elements give six dual combinations rather than fifteen, which
is the difference between a ten-row tower table and a nineteen-row one on a
32 KB cartridge.

| Element | Colour | Character |
|---|---|---|
| Fire | red | high damage, splash, slow rate of fire |
| Water | blue | low damage, slows what it hits |
| Earth | brown | very high damage, very slow, ignores armour |
| Nature | green | low damage, very fast, cheap |

Six duals, each needing at least one of both elements:

| Dual | Character |
|---|---|
| Fire + Water = Steam | splash that also slows |
| Fire + Earth = Magma | the heavy splash tower |
| Fire + Nature = Wildfire | fast, and burns what it hits |
| Water + Earth = Mud | the strongest slow in the game |
| Water + Nature = Vine | hits several creeps at once |
| Earth + Nature = Thorn | pierces everything in a line |

Ten towers, which is a ten-row stats table of (cost, damage, rate, range,
effect).

**Colour is never the only signal.** Each element has its own tower silhouette
as well as its own palette. This is what lets the cartridge be `cgb=on` rather
than `cgb=only`: it stays playable on a monochrome Game Boy, and it stays
playable for a player who cannot tell red from green.

## The board

160 by 144 is 20 tiles by 18. The interface is a **panel down the right**, six
columns wide, and the board takes the rest:

```
columns 0-13   the board, 14 x 17 cells of 8x8
columns 14-19  the panel: lives, wave, speed, gold
```

A cell `(cx, cy)` is the single tile at map row `cy`, column `cx`. Towers are
background tiles, so a full board of towers costs no objects at all. Creeps and
the build cursor are the only objects on screen.

**The panel is down the side rather than across the top for two reasons.** A
status strip costs two of the eighteen rows and caps the board at sixteen, which
turned out to be one row short of the map that was wanted. And the element draft
needs somewhere to show four running totals and whatever is buildable from them,
which a two-row strip was never going to hold.

The cost is width: six columns for the panel means the board can be at most
fourteen across. That is a real trade and it is the reason the route is 106 cells
rather than the 134 a full-width board held.

## A map is a picture

Same principle as MONKEY FARCE in the sibling repository: the level format is
the picture of the level, because authoring has to be cheap enough that you can
afford to throw a map away.

```
map_1:
  .str "S.........+++E"
  .str "+.+++.+++.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+++.+++.+++..."
```

`S` is where creeps enter, `E` is where they leave and cost you a life, `+` is
path, `.` is ground you can build on. Fourteen characters by seventeen rows, and
`.str` already emits ASCII minus $20, so the map is an index into a lookup table
without anything being packed by hand.

The shape is a switchback: no branches, one long winding corridor, in at the
left edge and out at the right. That is the trick Element TD's map is really
doing, and it is what maximises how much of the route sits inside a tower's
range on a board this small. 134 cells of route, 186 to build on.

**The order of the path is derived, not written.** At load the cartridge walks
from `S`, following path cells, and writes an ordered waypoint list. Nobody
hand-maintains a list of coordinates that has to agree with a picture.

`tools/checkmap.py` proves each map is well formed before it ships: exactly one
`S` and one `E`, every path cell with exactly two path neighbours except the
ends, the walk from `S` reaching `E`, and no path cell left unvisited. This is
the `reach.py` of this cartridge. A tower defence whose path has a fork in it is
not a hard bug to write and it is an impossible one to see by looking at a map.

## Creeps

A creep is a position on the path and nothing else, so it is cheap:

```
path_index  which waypoint it has reached      1 byte
sub         0-255 across the 8 pixels to the next one
hp          2 bytes
kind        1 byte
slow        frames of slow remaining           1 byte
```

`sub += speed` each frame; the carry out of that addition is exactly
`path_index += 1`, so movement is eight-bit arithmetic with no special case at a
corner. Pixel position is the waypoint's cell plus `sub >> 5` along the
direction to the next waypoint, which the walk recorded when it had the
columns and rows in hand.

Sixteen creeps maximum. The hardware draws ten objects per scanline and drops
the rest, so the object buffer is rotated every frame the way the demo cart's
sprite screen demonstrates: a crowded row flickers rather than permanently
hiding the same creep, which is the difference between a rendering artefact and
a lie about where the creeps are.

## Towers

Up to twenty-four, each four bytes: cell, kind, level, cooldown. A parallel
320-byte cell map answers "what is on this cell" for the cursor in one index.

Targeting is "the creep furthest along the path that is in range", which is the
tower defence convention and the one that makes the player's placement read
correctly. Range is checked on squared distance against a small table.

**No projectile objects in the first version.** A shot is instant, with the
tower flashing for two frames and the creep flashing when hit. This is a
deliberate trade of prettiness for the object budget, and it is reversible: if
there is room later, eight projectile objects go in without touching the
simulation.

## The wave, and the timer

Between waves a timer runs. When it reaches zero the next wave spawns. Pressing
Start spawns it immediately.

In one player that is a convenience. In two player it is the whole game.

## Two players

The rule for link games is: never send state, send seeds and intent.

At connect, the host rolls a sixteen-bit seed and sends it. Both cartridges seed
the same generator, so **both players face an identical wave schedule for the
rest of the match without another byte crossing the wire.** Each player
simulates only their own board. Your towers, your creeps, your gold and your
draft are never transmitted, because the opponent's cartridge does not need any
of them to stay correct.

The wire carries two things:

- **status**, a small packet cycling one byte per frame: wave, lives, and the
  four element counts packed into two bytes. This drives the opponent readout in
  the status bar. Losing a byte makes the readout stale for a tenth of a second
  and nothing else.
- **attack**, sent the moment it happens: calling a wave early sends the
  opponent extra creeps, in proportion to the time left on the timer that you
  skipped. Take the risk of two overlapping waves, make them eat it.

Two consequences are worth stating plainly, because they are the reason to build
it this way:

**There is no lockstep, so there is no desync to get wrong.** An attack that
arrives three frames late changes nothing. This is what makes the design survive
a networked cable, where a lockstep design would not.

**One player and two player are the same code.** Versus is a status bar and an
incoming-creep queue on top of the identical simulation. There is no second code
path to keep in agreement with the first.

You lose when your lives reach zero. The cartridge that is still standing wins.

### Bandwidth

At the normal 8192 Hz shift clock one byte costs 4096 T-cycles, so a frame holds
about seventeen. Fast mode holds about five hundred. This design needs a handful
a second. There is no bandwidth question here, which is exactly why the design
can afford to be this conservative about what it sends.

### Who clocks

The host picks the internal clock and is the master; the join side runs off the
external clock. A Game Boy slave cannot start a transfer, so the host sending
one byte a frame is what gives *both* directions a byte a frame, since the two
SB registers swap in the same exchange. If the host sees no sane reply for two
seconds it says so rather than pretending.

## Testing

`local_pair()` in `gb-core` wires two cores together in one process. That means
the two player mode is a `cargo test`, not a thing that needs two consoles and a
cable, which is not true of any link game we know of. The tests to write, in the
order they are worth writing:

1. the committed ROM still assembles byte for byte from the committed source;
2. every map passes `checkmap.py`;
3. one player: a recorded input solution clears wave five, driven through the
   real core rather than through a model of it;
4. two player: two cores, one link, both reach the same wave five composition
   from the same seed;
5. two player: calling a wave early on core A puts creeps on core B's board.

Number four is the one that matters. It is the claim the whole architecture
rests on, and it is checkable.

## Scope, honestly

32 KB, no mapper, which is the widest-compatibility Game Boy cartridge there is.
The demo cart in this repository uses 5.6 KB of its 32, so there is room, but a
tower defence is a much larger program than five test screens and this is not a
guarantee. `gb-asm` has no bank support at all, so if this outgrows 32 KB the
answer is to teach the assembler about banks, not to squeeze. That is a real
piece of work and it is better to know early, which is why the layout assertion
at the end of `main.s` is load-bearing rather than decorative.
