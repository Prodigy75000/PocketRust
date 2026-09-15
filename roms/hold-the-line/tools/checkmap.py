#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
# HOLD THE LINE. Dedicated to the public domain; see LICENSE.
"""Prove every map in src/data.s is a map a creep can actually walk.

    python3 tools/checkmap.py

The cartridge derives the route by walking from S and following whatever path
cell it has not just come from. That is the right way round, because it means
the picture is the only place the route is written down. It also means the walk
believes the picture, so a picture with a fork in it, a gap in it, or a stray
path cell in a corner produces a route that is quietly wrong rather than a
cartridge that fails to build.

A fork is not hard to draw by accident and it is close to impossible to see by
looking at a map. So this checks, before anything ships:

  * exactly one S and exactly one E;
  * S and E each touch exactly one path cell, so the route has two ends;
  * every other path cell touches exactly two, so there is no fork and no dead
    end;
  * the walk from S reaches E;
  * and it visits every path cell on the way, so there is no second loop
    floating somewhere else on the board.

Exits non-zero and says which map and which cell, so this can sit in a build.
"""

import re
import sys
from pathlib import Path

GRID_W = 10
GRID_H = 8

PATH = "+"
GROUND = "."
SPAWN = "S"
EXIT = "E"
WALKABLE = PATH + SPAWN + EXIT

HERE = Path(__file__).resolve().parent.parent
DATA = HERE / "src" / "data.s"


def load_maps(text):
    """Every `name:` followed by .str rows, up to its `name_end:` label."""
    maps = {}
    name = None
    rows = []
    for line in text.splitlines():
        label = re.match(r"^(\w+):\s*$", line)
        if label:
            if name and rows:
                maps[name] = rows
            tag = label.group(1)
            if tag.endswith("_end"):
                name, rows = None, []
            else:
                name, rows = tag, []
            continue
        row = re.match(r'^\s*\.str\s+"([^"]*)"', line)
        if row and name:
            rows.append(row.group(1))
    if name and rows:
        maps[name] = rows
    return {n: r for n, r in maps.items() if n.startswith("map_")}


def neighbours(cx, cy):
    for dx, dy in ((1, 0), (0, 1), (-1, 0), (0, -1)):
        nx, ny = cx + dx, cy + dy
        if 0 <= nx < GRID_W and 0 <= ny < GRID_H:
            yield nx, ny


def check(name, rows):
    """Return a list of complaints, empty if the map is sound."""
    bad = []

    if len(rows) != GRID_H:
        bad.append(f"has {len(rows)} rows, wanted {GRID_H}")
        return bad
    for y, row in enumerate(rows):
        if len(row) != GRID_W:
            bad.append(f"row {y} is {len(row)} wide, wanted {GRID_W}")
    if bad:
        return bad

    at = {}
    for y, row in enumerate(rows):
        for x, ch in enumerate(row):
            if ch not in WALKABLE + GROUND:
                bad.append(f"({x},{y}) is {ch!r}, which is not a cell kind")
            at[(x, y)] = ch
    if bad:
        return bad

    spawns = [p for p, c in at.items() if c == SPAWN]
    exits = [p for p, c in at.items() if c == EXIT]
    if len(spawns) != 1:
        bad.append(f"has {len(spawns)} spawns, wanted exactly one")
    if len(exits) != 1:
        bad.append(f"has {len(exits)} exits, wanted exactly one")
    if bad:
        return bad

    # Degree: the two ends have one walkable neighbour, everything else two.
    walk_cells = [p for p, c in at.items() if c in WALKABLE]
    for p in walk_cells:
        deg = sum(1 for n in neighbours(*p) if at[n] in WALKABLE)
        want = 1 if at[p] in (SPAWN, EXIT) else 2
        if deg != want:
            what = {1: "an end", 2: "a link in the chain"}[want]
            bad.append(
                f"{p} is {at[p]!r} with {deg} walkable neighbours; as {what} it "
                f"should have {want}. A fork here makes the route ambiguous and "
                f"the cartridge would pick one arm of it silently."
            )
    if bad:
        return bad

    # The walk the cartridge itself does.
    route = []
    cur, prev = spawns[0], None
    while True:
        route.append(cur)
        if at[cur] == EXIT:
            break
        nxt = [n for n in neighbours(*cur) if at[n] in WALKABLE and n != prev]
        if not nxt:
            bad.append(f"the walk from S got stuck at {cur}")
            return bad
        prev, cur = cur, nxt[0]
        if len(route) > GRID_W * GRID_H:
            bad.append("the walk from S never reached E")
            return bad

    missed = sorted(set(walk_cells) - set(route))
    if missed:
        bad.append(
            f"the route from S to E is {len(route)} cells but the map has "
            f"{len(walk_cells)} walkable ones; {missed} are on a separate loop "
            f"and no creep would ever reach them"
        )

    if not bad:
        print(f"  {name}: {len(route)} waypoints, S{route[0]} to E{route[-1]}")
    return bad


def main():
    if not DATA.exists():
        print(f"checkmap: cannot find {DATA}", file=sys.stderr)
        return 2
    maps = load_maps(DATA.read_text(encoding="utf-8"))
    if not maps:
        print("checkmap: found no maps in src/data.s", file=sys.stderr)
        return 2

    print(f"checkmap: {len(maps)} map(s) in {DATA.name}")
    failed = 0
    for name, rows in sorted(maps.items()):
        bad = check(name, rows)
        for complaint in bad:
            print(f"  {name}: {complaint}", file=sys.stderr)
        failed += bool(bad)

    if failed:
        print(f"checkmap: {failed} map(s) would not play", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
