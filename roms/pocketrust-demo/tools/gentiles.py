#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
# PocketRust Demo Cart. Dedicated to the public domain; see LICENSE.
"""Regenerate src/tiles.s from the glyph set below.

This exists only so a glyph can be edited as a compact 5x7 block and still come
out with every row exactly eight pixels wide. Its output is checked in and is
perfectly readable on its own; you never need to run this to build the ROM.

    python3 tools/gentiles.py > src/tiles.s
"""

import sys

# The font. One entry per character from $20 (space) to $5F (underscore), in
# ASCII order, so a tile index is exactly its character code minus $20 and the
# assembly source can write screen text as text.
#
# Each glyph is 5 wide and 7 tall inside an 8x8 tile, which leaves three columns
# and one row of gap. Three columns because the Game Boy's screen is 20 tiles
# wide and a 5x7 face with a 3-pixel gutter fits a whole line of readable text
# across it without any kerning work.
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

# Everything above the font: full 8x8 blocks, written straight out in the three
# drawable shades rather than in the font's one.
GRAPHICS = [
    ("tile_solid1", [
        "11111111", "11111111", "11111111", "11111111",
        "11111111", "11111111", "11111111", "11111111",
    ]),
    ("tile_solid2", [
        "22222222", "22222222", "22222222", "22222222",
        "22222222", "22222222", "22222222", "22222222",
    ]),
    ("tile_solid3", [
        "33333333", "33333333", "33333333", "33333333",
        "33333333", "33333333", "33333333", "33333333",
    ]),
    # The menu cursor: a filled arrowhead, shaded on its lower edge so it reads
    # as a solid object next to flat text.
    ("tile_cursor", [
        "........", ".33.....", ".333....", ".33333..",
        ".33333..", ".333....", ".33.....", "........",
    ]),
    # The sprite the object screen throws around. Round, with a highlight top
    # left and a shadow bottom right, so a wrong palette or a flipped attribute
    # is obvious at a glance rather than merely suspicious.
    ("tile_orb", [
        "..3333..", ".311113.", "31111223", "31112223",
        "31122223", "31222223", ".322223.", "..3333..",
    ]),
    # Its counterpart, a diamond, so the two rings of objects are told apart.
    ("tile_gem", [
        "...33...", "..3113..", ".311223.", "31122223",
        "31222223", ".322223.", "..3223..", "...33...",
    ]),
    # The scrolling playfield: brick courses that alternate every row, so a
    # scroll that tears or wraps early shows up as a broken course.
    ("tile_brick_a", [
        "33333333", "31111113", "31111113", "33333333",
        "11133111", "11133111", "11133111", "33333333",
    ]),
    ("tile_brick_b", [
        "33333333", "11133111", "11133111", "33333333",
        "31111113", "31111113", "31111113", "33333333",
    ]),
    # Sky furniture above the bricks.
    ("tile_star", [
        "........", "...2....", "..222...", ".22222..",
        "..222...", "...2....", "........", "........",
    ]),
    ("tile_cloud", [
        "........", "..2222..", ".222222.", "22222222",
        "22222222", ".222222.", "........", "........",
    ]),
    # A swatch frame: the colour screen fills the middle with a palette and the
    # border keeps two neighbouring swatches from bleeding into each other.
    ("tile_swatch", [
        "33333333", "31111113", "31111113", "31111113",
        "31111113", "31111113", "31111113", "33333333",
    ]),
    # The audio screen's meter, drawn at four heights.
    ("tile_bar0", [
        "........", "........", "........", "........",
        "........", "........", "........", "33333333",
    ]),
    ("tile_bar1", [
        "........", "........", "........", "........",
        "..2222..", "..2222..", "..2222..", "33333333",
    ]),
    ("tile_bar2", [
        "........", "........", "..2222..", "..2222..",
        "..2222..", "..2222..", "..2222..", "33333333",
    ]),
    ("tile_bar3", [
        "..2222..", "..2222..", "..2222..", "..2222..",
        "..2222..", "..2222..", "..2222..", "33333333",
    ]),
    # A button, lit and unlit, for the input screen.
    ("tile_key_off", [
        "..3333..", ".3....3.", "3......3", "3......3",
        "3......3", "3......3", ".3....3.", "..3333..",
    ]),
    ("tile_key_on", [
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
    print("; PocketRust Demo Cart. Dedicated to the public domain; see LICENSE.")
    print(";")
    print("; The character set, drawn as pixels rather than as hex, so that every")
    print("; graphic in this cartridge is visible in the source as the picture it is.")
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
    print(".assert tile_solid1 - tiles_start == $40 * 16, "
          '"the graphics no longer start where the font ends"')
    print(f".assert tiles_end - tiles_start == {(0x40 + len(GRAPHICS)) * 16}, "
          '"the tile set changed size"')


if __name__ == "__main__":
    sys.exit(main())
