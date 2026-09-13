#!/usr/bin/env python3
"""Convert Sayt123/SurfZones ck_zones SQL → assets/zones/<map>.json.

Usage:
  python3 tools/convert_surftimer_sql.py \
    --url 'https://raw.githubusercontent.com/Sayt123/SurfZones/main/Zones/Sorted%20-%20Alphabetically/zones/C/zones_surf_cyberwave_fix.sql' \
    --map surf_cyberwave --track-type staged --max-velocity 7500
"""

from __future__ import annotations

import argparse
import json
import re
import ssl
import urllib.request
from pathlib import Path

try:
    import certifi

    SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    SSL_CONTEXT = ssl.create_default_context()

# SurfTimer zonetype: 1=start, 2=end, 3=stage, 4=checkpoint (linear splits).
ZONE_START = 1
ZONE_END = 2
ZONE_STAGE = 3
ZONE_CHECKPOINT = 4


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "surf-oss-research/1.0"})
    with urllib.request.urlopen(req, timeout=60, context=SSL_CONTEXT) as resp:
        return resp.read().decode("utf-8", "replace")


def parse_rows(sql: str) -> list[dict]:
    # ('map','name',...,zonetype,zonetypeid,ax,ay,az,bx,by,bz,...,zonegroup,...)
    pat = re.compile(
        r"\('([^']+)'\s*,\s*'[^']*'\s*,\s*'[^']*'\s*,\s*'[^']*'\s*,\s*"
        r"(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*"
        r"(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*"
        r"(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*"
        r"(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)"
    )
    rows = []
    for m in pat.finditer(sql):
        rows.append(
            {
                "mapname": m.group(1),
                "zoneid": int(m.group(2)),
                "zonetype": int(m.group(3)),
                "zonetypeid": int(m.group(4)),
                "a": [float(m.group(5)), float(m.group(6)), float(m.group(7))],
                "b": [float(m.group(8)), float(m.group(9)), float(m.group(10))],
                "zonegroup": int(m.group(13)),
            }
        )
    return rows


def aabb(a: list[float], b: list[float]) -> dict:
    return {
        "mins": [min(a[i], b[i]) for i in range(3)],
        "maxs": [max(a[i], b[i]) for i in range(3)],
    }


# SurfTimer start AABBs are often flush with the floor and miss the teleport
# destination origin by a few units of Z (player spawns slightly above maxs).
START_Z_PAD = 128.0


def pad_start_zone(start: dict) -> dict:
    start = {
        "mins": list(start["mins"]),
        "maxs": list(start["maxs"]),
    }
    start["maxs"][2] = start["maxs"][2] + START_Z_PAD
    return start


def convert(map_name: str, rows: list[dict], track_type: str) -> dict:
    main = [r for r in rows if r["zonegroup"] == 0]
    start = next((aabb(r["a"], r["b"]) for r in main if r["zonetype"] == ZONE_START), None)
    end = next((aabb(r["a"], r["b"]) for r in main if r["zonetype"] == ZONE_END), None)
    if start is None or end is None:
        raise SystemExit(f"{map_name}: missing start/end in zonegroup 0")
    start = pad_start_zone(start)

    split_types = (ZONE_STAGE, ZONE_CHECKPOINT)
    cps = [
        (r["zonetypeid"], aabb(r["a"], r["b"]))
        for r in main
        if r["zonetype"] in split_types
    ]
    cps.sort(key=lambda x: x[0])
    # Stage 0 often duplicates the start box — drop exact matches.
    checkpoints = [b for _, b in cps if b != start]

    out: dict = {
        "formatVersion": 1,
        "mapName": map_name,
        "trackType": track_type,
        "source": "converted from Sayt123/SurfZones (ck_zones SQL)",
        "tracks": {
            "main": {
                "limitStartGroundSpeed": 350.0,
                "startOnJump": False,
                "start": start,
                "end": end,
                "checkpoints": checkpoints,
            }
        },
    }
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", required=True)
    ap.add_argument("--map", required=True, dest="map_name")
    ap.add_argument("--track-type", default="linear", choices=["linear", "staged"])
    ap.add_argument("--max-velocity", type=float, default=None)
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    map_name = args.map_name if args.map_name.startswith("surf_") else f"surf_{args.map_name}"
    dest = Path(__file__).resolve().parents[1] / "assets" / "zones" / f"{map_name}.json"
    if dest.exists() and not args.force:
        print(f"skip (exists): {dest}")
        return

    sql = fetch(args.url)
    rows = parse_rows(sql)
    if not rows:
        raise SystemExit("no rows parsed")
    out = convert(map_name, rows, args.track_type)
    if args.max_velocity is not None:
        out["maxVelocity"] = args.max_velocity
    dest.write_text(json.dumps(out, indent=2) + "\n")
    n = len(out["tracks"]["main"]["checkpoints"])
    print(f"wrote {dest} ({args.track_type}, {n} cps, rows={len(rows)})")


if __name__ == "__main__":
    main()
