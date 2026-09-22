#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check a built libretro core still exports every entry point libretro requires.

    python3 scripts/check-exports.py <core.so>

This is not paranoia, it is a bug that shipped. A patch inserted a helper
function between `#[no_mangle]` and `pub extern "C" fn retro_reset`, so the
attribute landed on the helper instead. `retro_reset` was name-mangled, the host
could not `dlsym` it, and every Game Boy game failed to load with
"NOT_INITIALIZED". The core compiled cleanly and every test passed.

It checks the symbol TABLE rather than the file, and that distinction is the
whole point: grepping the binary for the string would have passed happily,
because a mangled name like `_ZN..retro_reset17h..E` contains "retro_reset" as a
substring. Only an exact, NUL-delimited match tells an exported symbol from a
mangled one.
"""

import sys

REQUIRED = [
    "retro_set_environment",
    "retro_set_video_refresh",
    "retro_set_audio_sample",
    "retro_set_audio_sample_batch",
    "retro_set_input_poll",
    "retro_set_input_state",
    "retro_init",
    "retro_deinit",
    "retro_api_version",
    "retro_get_system_info",
    "retro_get_system_av_info",
    "retro_set_controller_port_device",
    "retro_reset",
    "retro_run",
    "retro_serialize_size",
    "retro_serialize",
    "retro_unserialize",
    "retro_cheat_reset",
    "retro_cheat_set",
    "retro_load_game",
    "retro_load_game_special",
    "retro_unload_game",
    "retro_get_region",
    "retro_get_memory_data",
    "retro_get_memory_size",
]


def main():
    if len(sys.argv) != 2:
        print("usage: check-exports.py <core.so>", file=sys.stderr)
        return 2
    try:
        blob = open(sys.argv[1], "rb").read()
    except OSError as e:
        print(f"  cannot read {sys.argv[1]}: {e}", file=sys.stderr)
        return 2

    tokens = set(blob.split(b"\0"))
    missing = [n for n in REQUIRED if n.encode() not in tokens]

    if missing:
        print("  MISSING exports: " + ", ".join(missing))
        print()
        print("  A core missing an entry point fails to load, and the host")
        print("  reports NOT_INITIALIZED rather than anything more specific.")
        print("  The usual cause is an attribute that drifted off its function:")
        print("  check that #[no_mangle] sits immediately above each one.")
        return 1

    print(f"  ok       all {len(REQUIRED)} libretro entry points exported")
    return 0


if __name__ == "__main__":
    sys.exit(main())
