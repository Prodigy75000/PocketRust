#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Assemble HOLD THE LINE.
#
# The ROM is committed, so this only needs running when the cartridge source
# changes. `cargo test -p gb-asm --test hold_the_line_reproduces` is what
# notices if someone forgets.
#
# The map check runs first and on purpose. The cartridge derives the creeps'
# route by walking the picture in data.s, which means it believes the picture: a
# fork or a gap there produces a route that is quietly wrong rather than a build
# that fails. That is not something you find by looking at a map.
#
# Run from the repository root. Nothing outside this repository is used.
set -e

ROM=roms/hold-the-line/hold-the-line.gbc

python roms/hold-the-line/tools/checkmap.py
echo

cargo run --quiet -p gb-asm -- roms/hold-the-line/src/main.s -o "$ROM" -s

echo
echo "Now check the image and the source still agree, and that the cartridge"
echo "still derives the route the map says it should:"
echo "  cargo test -p gb-asm  --test hold_the_line_reproduces"
echo "  cargo test -p gb-core --test hold_the_line"

# There is deliberately no SHA256SUMS here yet. It is the file PERMISSION.md
# points a reader at, and publishing a checksum for a cartridge that changes
# every afternoon teaches everyone to ignore it. It gets written, and gets a
# test, when this ships.
