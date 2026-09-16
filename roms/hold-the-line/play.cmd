@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem Assemble HOLD THE LINE and run it in a window. Double-click it, or run it
rem from anywhere; it finds the repository itself.
rem
rem   arrows   move the build cursor
rem   Z        A          X       B
rem   Enter    Start      RShift  Select, which steps the creep speed
rem
rem The first run builds the core in release and takes a minute. After that it
rem is a second or two, so this is the loop to iterate in rather than loading a
rem .gbc into a phone.

setlocal
cd /d "%~dp0..\.."

echo Checking the map...
python roms\hold-the-line\tools\checkmap.py
if errorlevel 1 (
  echo.
  echo The map would not play. Fix it in tools\mapdraw.html and try again.
  pause
  exit /b 1
)

echo.
echo Assembling...
cargo run --quiet -p gb-asm -- roms/hold-the-line/src/main.s -o roms/hold-the-line/hold-the-line.gbc -s
if errorlevel 1 (
  echo.
  echo The cartridge did not assemble.
  pause
  exit /b 1
)

echo.
cargo run --release --quiet -p gb-runner -- roms/hold-the-line/hold-the-line.gbc
