#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Is the core in TrophyHubAndroid's jniLibs current with this repo's core code?
#
#   scripts/check-deployed.sh
#
# ## Why this is not already covered
#
# `deploy-android-debug.sh` checks features, exports, ABI and the commit stamp,
# and `install-android-debug.sh` proves the device runs what is in jniLibs.
# Every one of those describes the ARTIFACT. None of them asks whether the
# artifact is current, so all of them pass on a .so built before the fix you
# just committed.
#
# That is not hypothetical. 3eb80e7 stopped a malformed ROM panicking and
# taking the whole app down. It was committed, pushed, reported as fixed, and
# never deployed; the app shipped the crash for five days while every guard
# above said the binary was fine, because it was a fine binary of the wrong
# commit.
#
# ## Dating by HEAD is the trap
#
# HEAD is usually a docs commit, so "your core is behind HEAD" reads as
# ignorable and gets ignored. This dates by the last commit that touched code
# the core is built from, which is the only date that means anything.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Defaults to the deployed core; an explicit path is for testing this script
# against a known-stale artifact, which is the only way to watch it reject.
SO="${1:-$root/../TrophyHubAndroid/app/src/main/jniLibs/arm64-v8a/libgbcore_libretro.so}"

# Only the crates the .so is built from. A README commit must not fire this.
CORE_PATHS=(crates/gb-core crates/gb-libretro Cargo.toml Cargo.lock)

if [ ! -f "$SO" ]; then
    echo "No core in jniLibs at $SO"
    exit 1
fi

want="$(git -C "$root" log -1 --format=%h --abbrev=9 -- "${CORE_PATHS[@]}")"
want_subject="$(git -C "$root" log -1 --format=%s -- "${CORE_PATHS[@]}")"
got="$(strings -a "$SO" 2>/dev/null | grep -o 'build=[A-Za-z0-9.-]*' | head -1 | sed 's/^build=//; s/-local$//; s/-dirty$//')"

echo "last commit touching the core: $want  $want_subject"
echo "core in jniLibs:               ${got:-<no stamp>}"

if [ -z "$got" ]; then
    echo
    echo "  UNKNOWN. That .so predates the build stamp, so it cannot be dated."
    exit 1
fi
if [ "$got" = "$want" ]; then
    echo
    echo "  ok       jniLibs is current"
    exit 0
fi

# The stamp is whatever commit was checked out when the .so was built, which is
# often LATER than the last core change: build after a docs commit and the two
# differ while the binary is perfectly current. So the question is not whether
# the two match, it is whether the deployed build CONTAINS the last core
# change. That is `want` being an ancestor of `got`, and getting this backwards
# reported a current core as a mismatch on the first run.
if git -C "$root" merge-base --is-ancestor "$want" "$got" 2>/dev/null; then
    echo
    echo "  ok       jniLibs contains every core commit (built at $got)"
    exit 0
fi

if git -C "$root" merge-base --is-ancestor "$got" "$want" 2>/dev/null; then
    n="$(git -C "$root" rev-list --count "$got..$want" -- "${CORE_PATHS[@]}")"
    echo
    echo "  STALE    the deployed core is missing $n core commit(s):"
    git -C "$root" log --oneline "$got..$want" -- "${CORE_PATHS[@]}" | sed 's/^/             /'
    echo
    echo "  Run scripts/deploy-android-debug.sh, then install-android-debug.sh."
    exit 1
fi
echo
echo "  MISMATCH neither commit contains the other, so the .so was built from"
echo "  a history this tree does not have: a different branch, or a rebase."
exit 1
