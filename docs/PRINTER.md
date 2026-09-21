# Game Boy Printer

The core emulates a Game Boy Printer on the link port and writes what it prints
as PNG files. This is the contract between the core and whatever surfaces those
files, written down so it does not drift.

![A Pokedex entry printed by the core](printer-pikachu.png)

That image is a real printout, produced by Pokemon Yellow through this core, and
it is committed here as a sample: it is byte for byte what the core writes at
runtime, so it can be used to test a decode, a gallery save or a share sheet
without needing the ROM or a save state.

## Turning it on

A libretro core option, **on by default**:

```
key     pocketrust_printer
values  on | off        (lower case, exactly these strings)
```

There is deliberately no user-facing toggle. The accessory is a pure slave and,
because it idles as open bus (below), a game that never prints cannot tell it is
attached. There is nothing for a player to decide, so a frontend that has not
read our options at all still gets a working printer.

### It idles as an empty port

While no packet is in progress the printer answers **`$FF`**, not `$00`. This
matters because the printer can be left plugged in permanently, so every game
that pokes the serial port meets it, not only the ones that print. An unplugged
Game Boy reads `$FF` back, and a game hunting for a link partner uses exactly
that to decide nobody is there. Answering `$00` while idle would tell Pokemon's
Cable Club that *something* is on the wire, and the failure would land on
trading rather than on printing.

A real printer does answer `$00` there. The deviation is one byte, only ever the
first of a packet, and only when the printer was not already mid-packet; games
read the reply at the trailer, not at the magic. Verified by printing a Pokedex
entry with it in place.

**A live GameLink session outranks it.** If one is up, the cable is a person and
the printer setting is ignored until that session ends. A front end that offers
both as toggles should disable or explain the printer one while a session is
live, rather than leaving a control that silently does nothing.

GameLink and netplay are **not** synonyms here, and the difference decides
whether that paragraph applies to you. Netplay means deterministic input
lockstep, and Game Boy does not have it: Trophy Hub's `netplayReady` list is
NES, SNES, Mega Drive, PS1, N64 and friends, with no Game Boy slot in it.
GameLink is the serial tunnel, carried over the libretro netpacket interface
this core implements, and Game Boy very much does have it: it has been the
default GB/GBC link path since PocketRust graduated on 2026-08-04, and 0.10.28
shipped cross-device trading on it.

So this is a real conflict a user can reach, not a defensive branch. Two people
mid-trade is exactly when a silently dead printer toggle would be noticed.

## Where the files go

```
<save directory>/printer/<CARTRIDGE TITLE> print NNN.png
```

**The subdirectory is load bearing.** On Android the save directory and the
system directory are the same path, and that path is the shared support tree for
every core in the app: BIOS images, Dolphin's `Sys`, PCSX2 resources. User
pictures must not be mixed into that, or a future "clear my prints" becomes a
dangerous thing to write.

`NNN` rises and never overwrites. The cartridge title is sanitised to
alphanumerics, spaces and dashes, so a corrupt or hostile header cannot write
outside the directory.

## The file

A PNG, 160 pixels wide, **2-bit indexed** with a four-entry palette running
white to black. That is exactly what the printer produces: four greys at the
resolution the hardware actually had. A full Pokedex printout is about 8 KB.

It is deliberately *not* converted to RGB or upscaled. This is the authentic
artifact and it is lossless; anything a destination needs beyond it, a larger
image for a social app or RGB for a chat client, is a rendering decision at the
point of sharing rather than a property of the file. Converting in the encoder
would throw information away for every consumer to suit one of them.

The encoder is hand written in `gb-core` rather than pulled in as a dependency,
because the libretro core has none and a printed page is not a good enough
reason to give it one. It uses stored deflate blocks, which PNG permits and
every decoder accepts.

## One printout, not one print command

**A Pokedex entry is two print commands.** The first ends with a paper feed of
zero and the second begins with one, and on real paper that means a single
continuous strip. The core joins them, in `gb_core::stitch`, and hands over one
image per *physical* printout.

This lives in the core on purpose. The margin rule is protocol knowledge, and
four clients each reimplementing it is three of them getting it subtly wrong.
Saving each print command separately turns one picture into fragments.

## Faults

Checksum errors and packet errors are reported, because a game can provoke them.
Paper jam, low battery and the generic error are never reported, because there is
no paper, no battery and no heat to be honest about.

A print reports itself busy for a handful of status polls rather than finishing
instantly, so a game's progress bar has something to show. It is a countdown, so
it always terminates.

## Trying it

```sh
cargo run --release -p gb-runner --bin gbprint -- <rom.gb> \
    --state <save.state> --keys "w20,a,w40,down,down,down,w20,a,w60" \
    --frames 3000 --out print
```

It prints the packet log as it goes, which is the first thing worth looking at
when a game says it cannot print, and writes the joined printouts.

## Tested

- `crates/gb-core/src/printer.rs`, unit tests: the device id, a bad checksum
  being reported and ignored, a band becoming a page, the palette being applied,
  busy terminating, resynchronising on the magic after junk, compressed and plain
  data producing identical pictures, a truncated run stopping rather than reading
  off the end, zero copies being a paper feed, the margin joining, and the PNG
  structure with its checksums against known answers.
- `crates/gb-core/tests/printer.rs`, against the real game: the exact command
  sequence Pokemon Yellow sends, the band sizes, both page dimensions, the
  margins, and that all four shades appear. That last one is the check a unit
  test cannot make, because the unit tests use a band of `$FF` where every pixel
  is colour 3, and a renderer that dropped the high bit plane would pass them.
  It skips when the ROM and save state are absent; see that file's comment.
