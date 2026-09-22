#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Build the app with this core in it, install it, and prove the device is
# running the binary you just built.
#
#   scripts/install-android-debug.sh [device-serial]
#
# With more than one device attached, adb refuses every command that has to
# pick one and the failure arrives several minutes in, after the app has
# already been built. So the serial is resolved and checked FIRST, and passed
# explicitly to every adb call below rather than left to adb's default.
#
# `deploy-android-debug.sh` stops when the .so is in jniLibs, deliberately:
# building the app is the Android agent's business. This script is the rest of
# MY loop, and it exists because running those steps by hand is how I got a
# wrong answer.
#
# ## The trap this closes
#
# `gradlew assembleDebug` printed BUILD FAILED, and the `adb install` run
# straight afterwards printed **Success**, because it installed the previous APK
# still sitting in app/build/outputs. Nothing in the install output hints at it.
# A failed build followed by a successful install is indistinguishable from a
# working deploy unless you read the gradle scrollback, and the device is then a
# build behind while looking current. Every observation taken on it is evidence
# about the wrong binary.
#
# So: the install is conditional on the build's exit code, and never sequential.
#
# ## Why it checks the INSTALLED apk and not the one it just built
#
# The core stamps the commit it came from into its feature string, which answers
# "what is this binary". It cannot answer "which binary is this device running",
# and three artifacts can disagree while each is internally consistent: that has
# happened here, with a tablet, a Drive copy and a tree all differing.
#
# The only way to settle it is to read the APK back off the device. So this
# pulls the installed base.apk and compares its core's stamp with the one in
# jniLibs. Anything else is inference.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$root/../TrophyHubAndroid"
JNI_SO="$APP_DIR/app/src/main/jniLibs/arm64-v8a/libgbcore_libretro.so"
PKG=com.trophyhub.android

# Which device. An explicit serial wins; otherwise there must be exactly one.
serial="${1:-}"
mapfile -t attached < <(adb devices | awk 'NR>1 && $2=="device"{print $1}')
if [ -n "$serial" ]; then
    found=0
    for d in "${attached[@]}"; do [ "$d" = "$serial" ] && found=1; done
    if [ "$found" -eq 0 ]; then
        echo "No attached device with serial '$serial'. Attached:"
        for d in "${attached[@]}"; do
            echo "  $d  $(adb -s "$d" shell getprop ro.product.model 2>/dev/null | tr -d '')"
        done
        exit 1
    fi
elif [ "${#attached[@]}" -eq 1 ]; then
    serial="${attached[0]}"
elif [ "${#attached[@]}" -eq 0 ]; then
    echo "No device attached."
    exit 1
else
    echo "More than one device attached. Name the one you want:"
    for d in "${attached[@]}"; do
        echo "  scripts/install-android-debug.sh $d   # $(adb -s "$d" shell getprop ro.product.model 2>/dev/null | tr -d '')"
    done
    exit 1
fi
adb="adb -s $serial"
echo "Target device: $serial ($($adb shell getprop ro.product.model 2>/dev/null | tr -d ''))"

stamp_of() {
    # The build= token out of POCKETRUST_FEATURES. Survives stripping, which is
    # why it is the thing compared: the APK's copy is stripped, so hashes
    # differ legitimately and would report a false mismatch.
    strings -a "$1" 2>/dev/null | grep -o 'build=[A-Za-z0-9.-]*' | head -1
}

if [ ! -f "$JNI_SO" ]; then
    echo "No core in jniLibs. Run scripts/deploy-android-debug.sh first."
    exit 1
fi
want="$(stamp_of "$JNI_SO")"
if [ -z "$want" ]; then
    echo "The core in jniLibs carries no build stamp."
    echo "It predates the stamp, so this script cannot tell you what is on the"
    echo "device. Re-run scripts/deploy-android-debug.sh."
    exit 1
fi
echo "Core in jniLibs: $want"

echo
echo "Building the app..."
# The whole point. A failed build must not reach the install below.
if ! (cd "$APP_DIR" && ./gradlew assembleDebug); then
    echo
    echo "  BUILD FAILED. Not installing."
    echo
    echo "  Nothing has been pushed to the device, so whatever is on it is"
    echo "  still the previous build. Do not read anything on it as evidence"
    echo "  about your change."
    exit 1
fi

APK="$APP_DIR/app/build/outputs/apk/debug/app-debug.apk"
echo
echo "Installing $APK"
$adb install -r "$APK"

echo
echo "Checking the DEVICE is running that build:"
dev_path="$($adb shell pm path "$PKG" | tr -d '\r' | sed -n 's/^package://p' | head -1)"
if [ -z "$dev_path" ]; then
    echo "  could not find $PKG on the device"
    exit 1
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
$adb pull -a "$dev_path" "$tmp/base.apk" >/dev/null 2>&1
unzip -o -q "$tmp/base.apk" "lib/arm64-v8a/libgbcore_libretro.so" -d "$tmp"
got="$(stamp_of "$tmp/lib/arm64-v8a/libgbcore_libretro.so")"

if [ "$got" = "$want" ]; then
    echo "  ok       device core is $got"
else
    echo "  MISMATCH device has '${got:-no stamp}', jniLibs has '$want'"
    echo
    echo "  The install reported success and the device is running something"
    echo "  else. Usually a stale APK in app/build/outputs, or a gradle task"
    echo "  that skipped the native merge."
    exit 1
fi
