<!--
SPDX-License-Identifier: CC0-1.0
HOLD THE LINE. Dedicated to the public domain; see LICENSE.
-->

# HOLD THE LINE

An element-drafting tower defence for the Game Boy Color, for one player or for
two over a link cable.

**This is in progress and is not a game yet.** What is here is the board, creeps
that walk it, and the checks. There are no towers. `DESIGN.md` beside this file is the spec it
is being built against, and is worth reading first.

![The board](screenshots/1-board.png)

## The one sentence

You do not buy towers. You earn one element per wave, and the elements you have
decide what you are allowed to build.

Four elements, Fire, Water, Earth and Nature, so the draft is one press of the
d-pad. Four pure towers and the six pairs between them.

## What works today

- The board: ten cells by eight, each 16 by 16, under a two-tile status bar.
  160 by 16 plus 160 by 128 is 160 by 144 exactly, so there is no margin
  anywhere to get wrong.
- A map is a picture in `src/data.s`, ten characters by eight rows, and the
  route creeps walk is **derived from it** rather than written beside it.
- Creeps that walk that route, leak at the exit, and cost you a life when they
  do. Waves arrive on a loop and the counter moves.
- A build cursor that moves and stops at the edges.

A creep is a waypoint index and a fraction of the way to the next one, so the
carry out of a single eight-bit addition is exactly "it reached the next
waypoint" and a corner needs no special case at all. That is the reason the
route is stored as waypoints rather than as pixel coordinates.

## What is not built yet

Towers, the element draft, the economy, a real wave table, sound, and the link
cable. In roughly that order.

## The route is derived, and that is checked three ways

The picture is the only place the route is written down. Nobody maintains a list
of coordinates that has to keep agreeing with a map, because that is a thing
that stops being true quietly.

The cost of deriving it is that the cartridge believes the picture: a fork or a
gap in a map produces a route that is wrong rather than a build that fails, and
neither is visible by looking at the map. So:

1. **`tools/checkmap.py`** proves the picture is sound before it ships. Exactly
   one `S` and one `E`; the two ends touching one path cell each and every other
   path cell touching exactly two, which is what rules out a fork and a dead
   end; the walk from `S` reaching `E`; and that walk visiting every path cell,
   which is what rules out a second loop stranded elsewhere on the board.
2. **`crates/gb-core/tests/hold_the_line.rs`** runs the actual cartridge in the
   actual core and reads the derived route back out of work RAM. `checkmap.py`
   is a second implementation in another language, and a second implementation
   agreeing with itself is not the claim; the claim is that the code on the
   cartridge gets the same answer.
3. Counting it by hand off the picture, which is where the number 36 came from.

All three say map 1 is a 36 cell route, so the number in the test is an absolute
one rather than something derived from the route it is checking. The test also
asserts that both ends are on the top edge, because a map that quietly grew an
exit on another edge would pass every other check and simply be a different
game.

The addresses those tests read come from the committed `.sym` file, not from
constants, because work RAM moves every time the game grows a variable and a
test that silently reads the wrong byte is worse than no test at all.

## Building it

Nothing outside this repository is needed. No RGBDS, no assembler to install.
Python is only for the map check and the tile generator, neither of which the
build itself requires.

```sh
scripts/build-hold-the-line.sh          # checks the maps, then assembles
```

or by hand, from the repository root:

```sh
cargo run -p gb-asm -- roms/hold-the-line/src/main.s \
    -o roms/hold-the-line/hold-the-line.gbc -s
```

`-s` writes the `.sym` listing beside the ROM, which the tests read.

```sh
cargo test -p gb-asm  --test hold_the_line_reproduces
cargo test -p gb-core --test hold_the_line
```

## A map is a picture

```
map_1:
  .str "S........E"
  .str "+..++++..+"
  .str "+..+..+..+"
  .str "+..+..+..+"
  .str "+..+..+..+"
  .str "+..+..+..+"
  .str "+..+..+..+"
  .str "++++..++++"
```

`S` is where creeps enter, `E` is where they leave and it costs you a life, `+`
is path, `.` is ground you can build on.

Creeps go in at the top and come out at the top, the way Element TD does, and
that one constraint decides the whole shape. It rules out a spiral, because a
spiral has to finish somewhere in the middle and there is no way back out to the
rim from there without crossing an arm it already drew. What it leaves is a
comb: four corridors joined alternately at the bottom and the top.

The corridors are three columns apart rather than two, and that is the part
worth knowing. Two apart also works and is a cell shorter, but it leaves the
right third of the board too far from anything to build on, and a fifth corridor
cannot be added to use it, because the route would then finish at the bottom of
the board. Three apart uses the full width and wastes nothing, so **every dark
cell on the board has a corridor on either side of it** and a tower placed
anywhere covers two passes of the route.

Nothing is packed, indexed or compiled by hand. The assembler's `.str` directive
already emits ASCII minus $20, which is exactly an index into a 64-byte lookup
table, so a map is edited by editing the picture of it.

## The two-player plan

The short version, with the reasoning in `DESIGN.md`: at connect the host rolls
a seed and sends it, both cartridges seed the same generator, and from then on
**both players face an identical wave schedule without another byte crossing the
wire.** Each side simulates only its own board. The cable carries a status
heartbeat and one event, which is that calling a wave early sends the opponent
creeps.

There is no lockstep, so there is no desync to get wrong, and single player is
the same code with the cable unplugged.

`local_pair()` in `gb-core` wires two cores together in one process, so the two
player mode will be a `cargo test` rather than something that needs two consoles
and a cable.

## Layout

```
src/main.s      code
src/data.s      maps, palettes, and what a map character means
src/tiles.s     the character set and the board graphics, as text art
tools/gentiles.py    regenerates src/tiles.s
tools/checkmap.py    proves every map is one a creep can walk
screenshots/    rendered by the headless harness
DESIGN.md       the spec
```

## Licence

Dedicated to the public domain under [CC0 1.0](LICENSE). Use it for anything, no
attribution required, no permission to ask for.

The only bytes here that are not original are the 48 at `$0104`, which are the
Game Boy's boot logo. Every cartridge that runs on the hardware carries them.
