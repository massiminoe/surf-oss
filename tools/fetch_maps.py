#!/usr/bin/env python3
"""Download the CS:S surf BSP corpus from fastdl.me into assets/maps/.

BSPs are not committed to the repo (they run ~17 MB–175 MB each). This script
is the reproducible way to get them, and it verifies every file against the
sha256 pinned in assets/maps/manifest.json — the zone AABBs, resim baselines
and surf-map tests are all keyed to those exact recompiles.

Usage:
  python3 tools/fetch_maps.py --all              # whole corpus (~1.4 GB)
  python3 tools/fetch_maps.py --batch core       # maps the test suite needs
  python3 tools/fetch_maps.py surf_boreas surf_frost
  python3 tools/fetch_maps.py --verify           # check what's on disk, no network
"""

from __future__ import annotations

import argparse
import bz2
import hashlib
import json
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
UA = "surf-oss-research/1.0 (+local-dev; FastDL map acquisition)"


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def load_manifest() -> dict:
    path = repo_root() / "assets" / "maps" / "manifest.json"
    with path.open() as f:
        return json.load(f)


def batches(doc: dict) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for name, entry in doc["maps"].items():
        out.setdefault(entry["batch"], []).append(name)
    out["tests"] = list(doc["required_by_tests"])
    return {k: sorted(v) for k, v in out.items()}


def normalize(name: str) -> str:
    stem = name[:-4] if name.endswith(".bsp") else name
    return stem if stem.startswith("surf_") else f"surf_{stem}"


def check(path: Path, entry: dict) -> str | None:
    """Return None if the file matches the manifest, else a reason string."""
    if not path.is_file():
        return "missing"
    size = path.stat().st_size
    if size != entry["bytes"]:
        return f"size {size} != manifest {entry['bytes']}"
    got = hashlib.sha256(path.read_bytes()).hexdigest()
    if got != entry["sha256"]:
        return f"sha256 {got[:16]}… != manifest {entry['sha256'][:16]}…"
    return None


def download_one(stem: str, entry: dict, dest_dir: Path, force: bool = False) -> None:
    out = dest_dir / f"{stem}.bsp"
    if out.exists() and not force:
        why = check(out, entry)
        if why is None:
            print(f"ok (cached): {stem}")
            return
        print(f"re-fetching {stem}: {why}")

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

    got = hashlib.sha256(data).hexdigest()
    if got != entry["sha256"]:
        raise SystemExit(
            f"failed {stem}: sha256 mismatch\n"
            f"  manifest {entry['sha256']}\n"
            f"  fetched  {got}\n"
            f"The mirror is serving a different build of this map. Zones and "
            f"baselines are pinned to the manifest hash — do not overwrite it "
            f"without re-deriving assets/zones/{stem}.json."
        )

    tmp = out.with_suffix(".bsp.part")
    tmp.write_bytes(data)
    tmp.replace(out)
    print(f"wrote {out} ({len(data)} bytes)")


def verify_all(manifest: dict[str, dict], names: list[str], dest: Path) -> None:
    bad = 0
    for stem in names:
        why = check(dest / f"{stem}.bsp", manifest[stem])
        if why is None:
            print(f"ok       {stem}")
        else:
            print(f"BAD      {stem}: {why}")
            bad += 1
    print(f"\n{len(names) - bad}/{len(names)} ok")
    if bad:
        raise SystemExit(1)


def main() -> None:
    doc = load_manifest()
    manifest = doc["maps"]
    known = batches(doc)

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("maps", nargs="*", help="Map names (with or without surf_ / .bsp)")
    ap.add_argument("--all", action="store_true", help="Every map in the manifest")
    ap.add_argument("--batch", choices=sorted(known), help="Named batch")
    ap.add_argument("--force", action="store_true", help="Re-download even if cached")
    ap.add_argument(
        "--verify",
        action="store_true",
        help="Check on-disk files against the manifest; no network",
    )
    ap.add_argument(
        "--dir",
        type=Path,
        default=None,
        help="Destination directory (default: assets/maps)",
    )
    args = ap.parse_args()

    names: list[str] = []
    if args.all:
        names.extend(sorted(manifest))
    if args.batch:
        names.extend(known[args.batch])
    names.extend(normalize(n) for n in args.maps)
    if not names:
        if args.verify:
            names = sorted(manifest)
        else:
            ap.error("pass map names, --batch, or --all")

    unknown = [n for n in names if n not in manifest]
    if unknown:
        ap.error(f"not in manifest: {', '.join(unknown)}")

    # De-dupe, preserving order.
    names = list(dict.fromkeys(names))
    dest = args.dir or (repo_root() / "assets" / "maps")

    if args.verify:
        verify_all(manifest, names, dest)
        return

    dest.mkdir(parents=True, exist_ok=True)
    for stem in names:
        download_one(stem, manifest[stem], dest, force=args.force)


if __name__ == "__main__":
    main()
