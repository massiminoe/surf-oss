#!/usr/bin/env python3
"""Download CS:S surf BSPs from fastdl.me into assets/maps/.

Usage:
  python3 tools/fetch_maps.py surf_nyx surf_boreas
  python3 tools/fetch_maps.py --batch linear-wave1
"""

from __future__ import annotations

import argparse
import bz2
import hashlib
import ssl
import urllib.error
import urllib.request
from pathlib import Path

try:
    import certifi

    SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    SSL_CONTEXT = ssl.create_default_context()

BASE = "https://main.fastdl.me/maps"
UA = "osx-surf-research/1.0 (+local-dev; FastDL map acquisition)"

# Linear / aesthetic wave — user's list + matching recommendations with wrldspawn zones.
BATCHES = {
    "linear-wave1": [
        "surf_void",
        "surf_lux",
        "surf_hourglass",
        "surf_nyx",
        "surf_boreas",
        "surf_tendies",
        "surf_andromeda",
        "surf_pantheon",
    ],
    "linear-wave2": [
        "surf_lovetunnel",
        "surf_frost",
        "surf_fornax",
        "surf_demise",
        "surf_cyberwave",  # staged-ish in some lists; still acquire
        "surf_aquaflow",
    ],
    "staged-later": [
        "surf_overgrowth",
        "surf_cement",
        "surf_botanica",
    ],
}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def download_one(name: str, dest_dir: Path, force: bool = False) -> Path:
    stem = name[:-4] if name.endswith(".bsp") else name
    out = dest_dir / f"{stem}.bsp"
    if out.exists() and not force:
        print(f"skip (exists): {out} ({out.stat().st_size} bytes)")
        return out

    url = f"{BASE}/{stem}.bsp.bz2"
    print(f"GET {url}")
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=300, context=SSL_CONTEXT) as resp:
            compressed = resp.read()
    except urllib.error.HTTPError as e:
        raise SystemExit(f"failed {stem}: HTTP {e.code}") from e

    data = bz2.decompress(compressed)
    if data[:4] != b"VBSP":
        raise SystemExit(f"failed {stem}: decompressed data is not VBSP")

    tmp = out.with_suffix(".bsp.part")
    tmp.write_bytes(data)
    tmp.replace(out)
    sha1 = hashlib.sha1(data).hexdigest()
    print(f"wrote {out} ({len(data)} bytes, sha1={sha1})")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("maps", nargs="*", help="Map names (with or without surf_ / .bsp)")
    ap.add_argument("--batch", choices=sorted(BATCHES), help="Named batch")
    ap.add_argument("--force", action="store_true")
    ap.add_argument(
        "--dir",
        type=Path,
        default=None,
        help="Destination directory (default: assets/maps)",
    )
    args = ap.parse_args()

    names: list[str] = []
    if args.batch:
        names.extend(BATCHES[args.batch])
    names.extend(args.maps)
    if not names:
        ap.error("pass map names or --batch")

    dest = args.dir or (repo_root() / "assets" / "maps")
    dest.mkdir(parents=True, exist_ok=True)

    for name in names:
        n = name
        if not n.startswith("surf_") and not n.endswith(".bsp"):
            n = f"surf_{n}"
        download_one(n, dest, force=args.force)


if __name__ == "__main__":
    main()
