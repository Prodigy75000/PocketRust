#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Assemble the demo cartridge and refresh its published checksum.
#
# Run from the repository root. Nothing outside this repository is used.
set -e

ROM=roms/pocketrust-demo/pocketrust-demo.gbc

cargo run --quiet -p gb-asm -- roms/pocketrust-demo/src/main.s -o "$ROM" -s

cd roms/pocketrust-demo
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum pocketrust-demo.gbc > SHA256SUMS
else
    shasum -a 256 pocketrust-demo.gbc > SHA256SUMS
fi
cat SHA256SUMS

echo
echo "Now check it still matches the committed source:"
echo "  cargo test -p gb-asm --test demo_rom_reproduces"
