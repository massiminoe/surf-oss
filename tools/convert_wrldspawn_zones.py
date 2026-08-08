#!/usr/bin/env python3
"""Convert wrldspawn/surf-zones bhoptimer JSON → assets/zones/<map>.json.

Usage:
  python3 tools/convert_wrldspawn_zones.py surf_nyx surf_boreas
  python3 tools/convert_wrldspawn_zones.py --batch linear-wave1
"""

from __future__ import annotations

import argparse
import json
import ssl
import sys
import urllib.request
from pathlib import Path

# Allow `python3 tools/convert_wrldspawn_zones.py` without installing a package.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from fetch_maps import BATCHES  # noqa: E402

try:
    import certifi

    SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    SSL_CONTEXT = ssl.create_default_context()

RAW_URL = (
    "https://raw.githubusercontent.com/wrldspawn/surf-zones/main/z/{map}.json"
)

# From wrldspawn README (+ GameBanana notes). Missing → omit (engine default 3500).
# 0 = uncapped in our format.
MAX_VELOCITY = {
    "surf_nyx": 4000.0,
    "surf_boreas": 5000.0,
    "surf_tendies": 5000.0,
    "surf_andromeda": 5000.0,
    "surf_pantheon": 10000.0,
    "surf_fornax": 5000.0,
    "surf_hourglass": 5000.0,
    "surf_void": 5000.0,
    "surf_cyberwave": 7500.0,
    "surf_summit": 0.0,
}

# Maps that are staged on KSF / wrldspawn notes.
STAGED = {
    "surf_overgrowth",
    "surf_overgrowth2",
    "surf_cement",
    "surf_botanica",
    "surf_cyberwave",  # wrldspawn: 2 stages
}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def aabb(a: list[float], b: list[float]) -> dict:
    mins = [min(a[i], b[i]) for i in range(3)]
    maxs = [max(a[i], b[i]) for i in range(3)]
    return {"mins": mins, "maxs": maxs}


def convert(map_name: str, raw: list[dict]) -> dict:
    main_start = None
    main_end = None
    checkpoints: list[dict] = []
    # Collect checkpoint/stage boxes on track 0, ordered by data index when present.
    cps: list[tuple[int, dict]] = []

    for z in raw:
        if z.get("track", 0) != 0:
            continue
        t = z.get("type")
        box = aabb(z["point_a"], z["point_b"])
        if t == "start":
            main_start = box
        elif t == "end":
            main_end = box
        elif t in ("checkpoint", "stage"):
            # Linear maps often store CP as type=checkpoint with data=N,
            # or stage boxes that act as timing splits.
            idx = int(z.get("data") or 0)
            cps.append((idx, box))

    if main_start is None or main_end is None:
        raise SystemExit(f"{map_name}: missing start/end on track 0")

    cps.sort(key=lambda x: x[0])
    checkpoints = [b for _, b in cps]

    track_type = "staged" if map_name in STAGED else "linear"
    out: dict = {
        "formatVersion": 1,
        "mapName": map_name,
        "trackType": track_type,
        "source": "converted from wrldspawn/surf-zones (bhoptimer AABB)",
        "tracks": {
            "main": {
                "limitStartGroundSpeed": 350.0,
                "startOnJump": False,
                "start": main_start,
                "end": main_end,
                "checkpoints": checkpoints,
            }
        },
    }
    if map_name in MAX_VELOCITY:
        out["maxVelocity"] = MAX_VELOCITY[map_name]
    return out


def fetch_raw(map_name: str) -> list[dict]:
    url = RAW_URL.format(map=map_name)
    print(f"GET {url}")
    req = urllib.request.Request(url, headers={"User-Agent": "osx-surf-research/1.0"})
    with urllib.request.urlopen(req, timeout=60, context=SSL_CONTEXT) as resp:
        return json.load(resp)


def convert_one(map_name: str, dest_dir: Path, force: bool = False) -> Path:
    if not map_name.startswith("surf_"):
        map_name = f"surf_{map_name}"
    out = dest_dir / f"{map_name}.json"
    if out.exists() and not force:
        print(f"skip (exists): {out}")
        return out

    raw = fetch_raw(map_name)
    converted = convert(map_name, raw)
    out.write_text(json.dumps(converted, indent=2) + "\n")
    n_cp = len(converted["tracks"]["main"]["checkpoints"])
    mv = converted.get("maxVelocity", "default")
    print(
        f"wrote {out} ({converted['trackType']}, {n_cp} cps, maxVelocity={mv})"
    )
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("maps", nargs="*")
    ap.add_argument("--batch", choices=sorted(BATCHES))
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    names: list[str] = []
    if args.batch:
        names.extend(BATCHES[args.batch])
    names.extend(args.maps)
    if not names:
        ap.error("pass map names or --batch")

    dest = repo_root() / "assets" / "zones"
    dest.mkdir(parents=True, exist_ok=True)

    # Wave batches include maps without wrldspawn files — skip those gently.
    for name in names:
        n = name if name.startswith("surf_") else f"surf_{name}"
        try:
            convert_one(n, dest, force=args.force)
        except Exception as e:
            print(f"WARN {n}: {e}", file=sys.stderr)


if __name__ == "__main__":
    main()
