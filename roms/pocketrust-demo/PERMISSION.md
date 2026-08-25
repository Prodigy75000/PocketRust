# Permission to distribute the PocketRust Demo Cart

**Work:** PocketRust Demo Cart (`pocketrust-demo.gbc`), a Game Boy Color
cartridge image, together with its complete source code.

**Author and sole rights holder:** Armand Bireaud, publishing as Prodigy75000
(`https://github.com/Prodigy75000`).

**Source of record:** `https://github.com/Prodigy75000/PocketRust`, directory
`roms/pocketrust-demo/`.

---

## Statement

I am the sole author of the PocketRust Demo Cart. I wrote every line of its
SM83 assembly source, drew every graphic in it, and composed the music in it. It
was assembled by `gb-asm`, a Game Boy assembler that I also wrote and that lives
in the same repository, so no third-party build tool contributed any content to
the resulting file.

With the single exception described in the next section, the cartridge contains
no code, artwork, audio, text, font, logo, trademark or data taken from any
commercial video game, from any other emulator project, or from any other third
party. It is not derived from, traced from, or measured against any existing
cartridge. Its character set, its graphics and its music were made for this
cartridge and exist nowhere else.

## The one exception: the 48-byte header logo

Bytes `$0104` through `$0133` of every Game Boy cartridge ever made hold a fixed
48-byte bitmap of Nintendo's logo. The console's boot ROM compares those bytes
against its own copy and refuses to start the cartridge if they differ. They are
not there because a cartridge author chose them; they are the lock the hardware
opens with, and no cartridge that runs on a Game Boy can omit them.

Those 48 bytes are in this file, they are the only bytes in it that are not
mine, and they are visible as a labelled block in `src/main.s`. I claim no
rights in them and none are granted here. They are reproduced solely so the
cartridge starts on the hardware it was written for, which is the same reason
every other Game Boy cartridge contains them.

If you would rather distribute a build that carries nothing of anyone else's,
zero that block in `src/main.s` and reassemble. The result runs in PocketRust
and in any front-end that starts a cartridge directly, and stops at the logo
screen on hardware or on an emulator running a real boot ROM.

## Grant

I dedicate the PocketRust Demo Cart, both the assembled `.gbc` image and its
complete source, to the public domain under **CC0 1.0 Universal**. The full text
of that dedication is in the `LICENSE` file beside this one.

To the extent that anything further is useful, I additionally and expressly
permit any person or organisation, without charge, condition, notice or further
permission, to:

- download, copy, host, redistribute and mirror the cartridge image;
- include the cartridge image inside another product, including a commercial
  product and including an application submitted to or distributed through the
  Apple App Store, the Google Play Store, or any other channel;
- use the cartridge image to test, demonstrate, review, benchmark or certify
  emulator software;
- modify the cartridge or its source and distribute the result.

This permission is irrevocable and is not limited by territory, medium, format
or duration.

## Relationship to the surrounding repository

The PocketRust emulator core that this cartridge lives beside is licensed
GPL-3.0-or-later. The cartridge is not. A repository is not a single work, and
these are two separate works distributed together: the core is a program, and
the cartridge is a data file with its own source, built by a separate tool and
executed by the emulated machine rather than linked into anything.

The assembler that produces the cartridge, `gb-asm`, is itself GPL-3.0, and that
has no effect on the cartridge. A compiler's licence does not attach to what it
compiles, and this assembler in particular injects nothing of its own into its
output: every byte of `pocketrust-demo.gbc` other than the header fields listed
below and the unused padding comes from the cartridge source in `src/`.

The fields `gb-asm` fills in are `$0134` to `$014F`: the title, the hardware
flags declared on the `.gb` line of `src/main.s`, and the two checksums, both of
which are computed from the cartridge's own bytes. They are the Game Boy's
equivalent of a container header. Nothing in them is creative content and
nothing in them came from anywhere else.

CC0 imposes no conditions on anyone, so nothing about it can conflict with the
GPL in either direction. In any case I hold the copyright in both works and am
free to license each as I choose.

## Verification

Anyone can confirm that the distributed binary is exactly what the published
source produces. From a clone of the repository:

```sh
cargo test -p gb-asm --test demo_rom_reproduces
```

That test reassembles `roms/pocketrust-demo/src/main.s` and compares the result
byte for byte with the committed `pocketrust-demo.gbc`. The published SHA-256 of
the current build is recorded in `SHA256SUMS` in this directory, and the same
test checks that file against the image too.

## Disclaimer of warranty

The work is provided as-is, without warranty of any kind, as set out in section
4 of the CC0 text in `LICENSE`.

## Note on trademarks

"Game Boy", "Game Boy Color" and "Nintendo" are trademarks of Nintendo. This
cartridge and the PocketRust project are independent works and are not
affiliated with, endorsed by, or sponsored by Nintendo. Nothing in this document
claims any right in anyone else's trademark, and CC0 does not purport to license
trademark rights (see section 4(a) of `LICENSE`).

---

Signed,

**Armand Bireaud**

Date: 25 August 2026

GitHub: https://github.com/Prodigy75000

If a handwritten signature is required, print this page and sign below.

Signature: __________________________________
