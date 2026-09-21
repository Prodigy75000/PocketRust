#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Build the libretro core for Android arm64 and drop it where the app loads it.
#
# The umbrella CLAUDE.md says every core repo has this script. This one did not,
# which is how a six-week-old .so ended up shipping: the build steps were in the
# README, so rebuilding was a thing you had to remember rather than a thing you
# ran. Now it is one command.
#
#   scripts/deploy-android-debug.sh
#
# It does NOT run gradle. Building the app is the Android agent's business and
# their build is the one that should fail if something is wrong with it; this
# script's job ends when the binary is in place, and it says so.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

APP=../TrophyHubAndroid/app/src/main/jniLibs/arm64-v8a
SO=target/aarch64-linux-android/release/libgbcore_libretro.so

if [ ! -f .cargo/config.toml ]; then
    echo "No .cargo/config.toml. Copy .cargo/config.toml.example and point the"
    echo "linker lines at your NDK, then run this again. See the README."
    exit 1
fi

echo "Building for aarch64-linux-android..."
cargo build --release -p gb-libretro --target aarch64-linux-android

# The shipped .so is a binary with no version string in it, so the only way to
# know a feature is really in the build is to look for something it added. A
# core that silently predates the feature under test is indistinguishable from a
# broken feature, which is exactly the trap this script exists to close.
echo
echo "Checking the build actually contains what it should:"
# The feature list the core advertises about itself, plus the strings that only
# exist if particular features were compiled in. The list is the reliable one:
# the Game Boy Camera's mapper has no string literal of its own, so before
# BUILD_FEATURES existed there was no way to prove it was in a binary at all.
for marker in POCKETRUST_FEATURES: printer camera gamelink pocketrust_printer /printer; do
    if grep -qa -- "$marker" "$SO"; then
        echo "  ok       $marker"
    else
        echo "  MISSING  $marker"
        echo
        echo "The build is missing something it should have. Not deploying it."
        exit 1
    fi
done

# e_machine at offset 18 is 0xB7 little-endian for AArch64. Dropping a host
# build into jniLibs produces a dlopen failure at runtime and nothing sooner.
abi=$(od -An -tx1 -j18 -N2 "$SO" | tr -d ' ')
if [ "$abi" != "b700" ]; then
    echo "  WRONG ABI: e_machine is $abi, wanted b700 (AArch64). Not deploying."
    exit 1
fi
echo "  ok       AArch64"

mkdir -p deploy
cp "$SO" deploy/gbcore_libretro_android.so

if [ -d "$APP" ]; then
    cp "$SO" "$APP/libgbcore_libretro.so"
    echo
    echo "Deployed to $APP/libgbcore_libretro.so"
    ls -l "$APP/libgbcore_libretro.so"
else
    echo
    echo "TrophyHubAndroid is not beside this repo, so only deploy/ was written."
fi

echo
echo "Now build the app. From TrophyHubAndroid:"
echo "  ./gradlew assembleDebug"
echo
echo "TrophyHubAndroid's own check should agree the core is current:"
echo "  node scripts/cores.mjs verify"
