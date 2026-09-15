#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
# HOLD THE LINE. Dedicated to the public domain; see LICENSE.
"""Regenerate src/tiles.s from the glyph set and the graphics below.

This exists only so a glyph can be edited as a compact 5x7 block and still come
out with every row exactly eight pixels wide. Its output is checked in and is
perfectly readable on its own; you never need to run this to build the ROM.

    python3 tools/gentiles.py > src/tiles.s

The font is the same 5x7 face as the demo cartridge in this repository, which is
also CC0 and also ours. It is copied rather than shared because a cartridge that
needs a second directory to assemble is a cartridge nobody can hand to anybody.
"""

import sys

# The font. One entry per character from $20 (space) to $5F (underscore), in
# ASCII order, so a tile index is exactly its character code minus $20 and the
# assembly source can write screen text as text.
FONT = {
    " ": ["....."] * 7,
    "!": ["..#..", "..#..", "..#..", "..#..", ".....", "..#..", "....."],
    '"': [".#.#.", ".#.#.", ".....", ".....", ".....", ".....", "....."],
    "#": [".#.#.", "#####", ".#.#.", ".#.#.", "#####", ".#.#.", "....."],
    "$": ["..#..", ".####", "#.#..", ".###.", "..#.#", "####.", "..#.."],
    "%": ["##..#", "##..#", "...#.", "..#..", ".#...", "#..##", "#..##"],
    "&": [".##..", "#..#.", "#.#..", ".#...", "#.#.#", "#..#.", ".##.#"],
    "'": ["..#..", "..#..", ".....", ".....", ".....", ".....", "....."],
    "(": ["...#.", "..#..", "..#..", "..#..", "..#..", "..#..", "...#."],
    ")": [".#...", "..#..", "..#..", "..#..", "..#..", "..#..", ".#..."],
    "*": [".....", "#.#.#", ".###.", "#####", ".###.", "#.#.#", "....."],
    "+": [".....", "..#..", "..#..", "#####", "..#..", "..#..", "....."],
    ",": [".....", ".....", ".....", ".....", "..##.", "..#..", ".#..."],
    "-": [".....", ".....", ".....", "#####", ".....", ".....", "....."],
    ".": [".....", ".....", ".....", ".....", ".....", ".##..", ".##.."],
    "/": ["....#", "....#", "...#.", "..#..", ".#...", "#....", "#...."],
    "0": [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    "1": ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    "2": [".###.", "#...#", "....#", "..##.", ".#...", "#....", "#####"],
    "3": ["####.", "....#", "....#", ".###.", "....#", "....#", "####."],
    "4": ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    "5": ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    "6": ["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."],
    "7": ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    "8": [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    "9": [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
    ":": [".....", ".##..", ".##..", ".....", ".##..", ".##..", "....."],
    ";": [".....", ".##..", ".##..", ".....", ".##..", "..#..", ".#..."],
    "<": ["...#.", "..#..", ".#...", "#....", ".#...", "..#..", "...#."],
    "=": [".....", ".....", "#####", ".....", "#####", ".....", "....."],
    ">": [".#...", "..#..", "...#.", "....#", "...#.", "..#..", ".#..."],
    "?": [".###.", "#...#", "....#", "..##.", "..#..", ".....", "..#.."],
    "@": [".###.", "#...#", "#.###", "#.#.#", "#.###", "#....", ".###."],
    "A": [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "B": ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."],
    "C": [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
    "D": ["###..", "#..#.", "#...#", "#...#", "#...#", "#..#.", "###.."],
    "E": ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    "F": ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
    "G": [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###."],
    "H": ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "I": [".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
    "J": ["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."],
    "K": ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"],
    "L": ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
    "M": ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
    "N": ["#...#", "##..#", "##..#", "#.#.#", "#..##", "#..##", "#...#"],
    "O": [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "P": ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."],
    "Q": [".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"],
    "R": ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
    "S": [".###.", "#...#", "#....", ".###.", "....#", "#...#", ".###."],
    "T": ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
    "U": ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "V": ["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."],
    "W": ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#"],
    "X": ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
    "Y": ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
    "Z": ["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"],
    "[": ["..###", "..#..", "..#..", "..#..", "..#..", "..#..", "..###"],
    "\\": ["#....", "#....", ".#...", "..#..", "...#.", "....#", "....#"],
    "]": ["###..", "..#..", "..#..", "..#..", "..#..", "..#..", "###.."],
    "^": ["..#..", ".#.#.", "#...#", ".....", ".....", ".....", "....."],
    "_": [".....", ".....", ".....", ".....", ".....", ".....", "#####"],
}

# ---- graphics ---------------------------------------------------------------
# Everything that is not a letter, drawn as pixels. A digit is the colour number
# within the tile's palette; a dot is colour 0.

GRAPHICS = [
    # Ground: a cell you can build on. The field is colour 0 and each of the
    # four tiles carries a tick in its own OUTER corner, so a 2x2 cell is marked
    # at its four corners and the board reads as cells rather than as an
    # undifferentiated 8-pixel lattice. That is the whole reason this is four
    # tiles and not one.
    ("tile_ground_tl", [
        "11......", "1.......", "........", "........",
        "........", "........", "........", "........",
    ]),
    ("tile_ground_tr", [
        "......11", ".......1", "........", "........",
        "........", "........", "........", "........",
    ]),
    ("tile_ground_bl", [
        "........", "........", "........", "........",
        "........", "........", "1.......", "11......",
    ]),
    ("tile_ground_br", [
        "........", "........", "........", "........",
        "........", "........", ".......1", "......11",
    ]),
    # Path. Deliberately flat and untextured: creeps move along it every frame
    # and a busy floor under a moving object is the fastest way to make a Game
    # Boy screen unreadable.
    ("tile_path", [
        "22222222", "22222222", "22222222", "22222222",
        "22222222", "22222222", "22222222", "22222222",
    ]),
    # The build cursor, as ONE corner bracket. The other three corners are this
    # same tile with the object's X and Y flip bits set, so a 16x16 cursor costs
    # four objects and sixteen bytes of tile data.
    ("tile_cursor", [
        "33333...", "3.......", "3.......", "3.......",
        "........", "........", "........", "........",
    ]),
    # A creep.
    ("tile_creep", [
        "..3333..", ".322223.", "32222223", "32222223",
        "32222223", "32222223", ".322223.", "..3333..",
    ]),
]


def glyph_rows(cell):
    """Grow a 5x7 face into the 8x8 tile it is drawn in."""
    rows = [row + "..." for row in cell]
    rows.append("........")
    return [r.replace("#", "3") for r in rows]


def emit(name, rows, comment=None):
    print()
    if comment:
        print(f"; {comment}")
    print(f".tile {name}")
    for row in rows:
        assert len(row) == 8, f"{name}: row {row!r} is {len(row)} wide"
        print(row)
    print(".endtile")


def main():
    # Write LF regardless of platform: the output is checked in, and a build
    # that flips line endings would make the reproducibility test report a
    # source drift that is not one.
    sys.stdout.reconfigure(newline="\n")
    print("; SPDX-License-Identifier: CC0-1.0")
    print("; HOLD THE LINE. Dedicated to the public domain; see LICENSE.")
    print(";")
    print("; The character set and the board graphics, drawn as pixels rather than")
    print("; as hex, so that every graphic in this cartridge is visible in the")
    print("; source as the picture it is.")
    print("; Generated by tools/gentiles.py; edit the glyphs there and regenerate.")
    print(";")
    print("; A tile index is its character code minus $20, which is why the source")
    print("; can write screen text as text.")
    print()
    print("tiles_start:")

    order = [chr(c) for c in range(0x20, 0x60)]
    missing = [c for c in order if c not in FONT]
    assert not missing, f"font is missing {missing}"
    for i, ch in enumerate(order):
        label = "chr_%02x" % (0x20 + i)
        name = {" ": "space", '"': "quote", "\\": "backslash"}.get(ch, ch)
        emit(label, glyph_rows(FONT[ch]), f"${0x20 + i:02X}  {name}")

    print()
    print("; ---- graphics ----------------------------------------------------")
    for name, rows in GRAPHICS:
        emit(name, rows)

    print()
    print("tiles_end:")
    print()
    # 'A' is character $41, so it is tile $41-$20 and its bytes start there.
    print(".assert chr_41 - tiles_start == ($41 - $20) * 16, "
          '"a tile was inserted into the font and shifted every index after it"')
    print(".assert tile_ground_tl - tiles_start == $40 * 16, "
          '"the graphics no longer start where the font ends"')
    print(f".assert tiles_end - tiles_start == {(0x40 + len(GRAPHICS)) * 16}, "
          '"the tile set changed size"')


if __name__ == "__main__":
    sys.exit(main())
