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

**A file appears when a printout is complete, and that is the whole contract.**
A frontend should announce one print per file and never try to work out which
files belong together.

### What happens if the continuation never comes

A page whose margins say "continuous" is **held**, and released when the printer
has been silent for five seconds (`IDLE_FLUSH_FRAMES`). So a print that the
player cancelled, or that a crashed cartridge abandoned, still lands: late, but
it lands, and it lands as its own file.

Five seconds of silence, rather than a fixed delay since the print, because the
two pages of a Pokedex entry are **750 frames apart**, twelve and a half
seconds, while the second page's eight data packets crawl over an 8192 Hz link.
Any fixed timeout long enough for that would make every abandoned print wait an
age. Within a job the printer is never quiet for more than about a second, so
silence separates "still working" from "gone" cleanly.

A game being unloaded also releases anything held.

### How this went wrong once

`stitch` takes every page and joins them, which is right when you have them all.
The libretro core called it **once a frame**, so it never held more than one page
and joined nothing; every Pokedex entry shipped as two files. The command-line
tool collected everything first and looked perfect. Same function, opposite
behaviour, and it surfaced on a phone rather than in a test.

`stitch` is now written in terms of `Spool`, the live assembler, so there is one
rule instead of two that agree until they do not. `gbprint` drains once a frame
exactly as the core does, because a tool that exercises a different path from the
thing it is testing is worse than no tool.

## Known bug: the Game Boy Camera retries forever

**Not fixed.** The Camera prints correctly, the PNG is written, and the game
then sits on "transferring..." with a full progress bar and re-sends the print
command about every 45 frames, forever.

Reproducible locally, which is the main thing:

```sh
cargo run --release -p gb-runner --bin gbprint --     "dumps/roms/Game Boy Camera (USA, Europe) (SGB Enhanced).gb"     --state dumps/states/camera-print.state --keys "w30,a,w60"     --frames 1500 --shot screen.png --out print
```

The save state sits on the print screen; A prints. Count `packet print` lines:
one is correct, seventeen is the bug.

### What is known, measured rather than assumed

The conversation is `init`, nine data packets of 640 bytes, an empty data
packet, then `print` repeating. Each print asks for the same thing: one sheet,
margins `$13`, palette `$E4`, exposure `$40`. The Camera polls status until the
busy bit clears, sees `$00`, and immediately re-sends.

### Hypotheses ruled out by measurement

- **Buffer-full being reported.** It WAS being reported wrongly and that is
  fixed (see below), but fixing it did not stop the retry.
- **"Unprocessed data" clearing too early.** Holding it set for the duration of
  the job changed the status bytes and nothing else.
- **The print finishing too fast.** Making the busy period 25 times longer
  changed the retry INTERVAL proportionally and nothing else. The Camera always
  retries once busy clears, so "busy cleared" is not what it is waiting for.
- **The open-bus idle byte.** Answering `$00` while idle, as the spec literally
  says, instead of `$FF`, made no difference. So that deviation is not implicated
  and can stay.

### The real bug found on the way, and fixed

The printer's buffer limit was **nine bands**, reasoned from "144 lines is one
Game Boy screen". The documented capacity is 8 KiB, "a maximum bitmap area of
160*200 pixels between prints". A full-screen print is exactly nine bands, 5760
bytes, so the Camera landed precisely on the invented limit and was told the
buffer was full when it was two thirds empty. Pokemon Yellow prints five and
seven bands and never came near it.

That is the lesson worth keeping: **a harness that only drives one game only
finds that game's bugs.** Yellow prints and walks away; the Camera waits for the
printer afterwards, so it exercises the tail of the protocol that Yellow never
touches.

### Also suspected, not yet acted on

The spec says command `$01` initialise "clears buffer RAM", which implies
**print does not**. This implementation clears the buffer on print. That is
probably wrong, but changing it while the retry bug is live would write one PNG
per retry, so it waits until the retry is understood.

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
