#!/usr/bin/env python3
"""Throttled, resumable KSF CS:S 66t replay acquisition.

Caches binaries under assets/replays/external/ksf/<map>/raw/ and writes
manifest.json with provenance + SHA-256. See docs/KSF-REPLAY-HANDOFF.md.

Usage:
  python3 tools/ksf_fetch.py --map surf_summit --max-rank 10
  python3 tools/ksf_fetch.py --batch aesthetic-17 --max-rank 10
  python3 tools/ksf_fetch.py surf_nyx surf_boreas --max-rank 5

Polite defaults: one in-flight request, >=1s delay, Retry-After / backoff on
429/5xx. Does not mirror the whole service.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

try:
    import certifi

    SSL_CONTEXT = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    SSL_CONTEXT = ssl.create_default_context()

BASE = "https://ksf.surf"
UA = "osx-surf-research/1.0 (+local-dev; KSF replay acquisition; 1 req/s)"
MIN_DELAY_S = 1.0
MAX_RETRIES = 6

# Same aesthetic wave as tools/fetch_maps.py (exact KSF names, 2026-08-02 probe).
BATCHES = {
    "aesthetic-17": [
        "surf_nyx",
        "surf_boreas",
        "surf_tendies",
        "surf_lovetunnel",
        "surf_andromeda",
        "surf_cyberwave",
        "surf_overgrowth",
        "surf_cement",
        "surf_pantheon",
        "surf_lux",
        "surf_fornax",
        "surf_void",
        "surf_frost",
        "surf_aquaflow",
        "surf_hourglass",
        "surf_botanica",
        "surf_demise",
    ],
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
        "surf_cyberwave",
        "surf_aquaflow",
    ],
    "staged-later": [
        "surf_overgrowth",
        "surf_cement",
        "surf_botanica",
    ],
    "m1-corpus": [
        "surf_summit",
        "surf_beginner",
        "surf_utopia_njv",
        "surf_kitsune",
        "surf_mesa_fixed",
    ],
}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class ThrottledClient:
    def __init__(self, min_delay: float = MIN_DELAY_S) -> None:
        self.min_delay = min_delay
        self._next_ok = 0.0

    def _wait(self) -> None:
        now = time.monotonic()
        if now < self._next_ok:
            time.sleep(self._next_ok - now)

    def get(self, url: str) -> tuple[int, dict, bytes]:
        last_err: Exception | None = None
        for attempt in range(MAX_RETRIES):
            self._wait()
            req = urllib.request.Request(
                url,
                headers={"User-Agent": UA, "Accept": "*/*"},
            )
            try:
                with urllib.request.urlopen(req, timeout=60, context=SSL_CONTEXT) as resp:
                    body = resp.read()
                    headers = {k.lower(): v for k, v in resp.headers.items()}
                    status = resp.status
                    self._next_ok = time.monotonic() + self.min_delay
                    if status in (429, 500, 502, 503, 504):
                        retry_after = headers.get("retry-after")
                        delay = float(retry_after) if retry_after and retry_after.isdigit() else min(60.0, self.min_delay * (2**attempt))
                        print(f"  HTTP {status}; backing off {delay:.1f}s")
                        time.sleep(delay)
                        continue
                    return status, headers, body
            except urllib.error.HTTPError as e:
                last_err = e
                headers = {k.lower(): v for k, v in e.headers.items()} if e.headers else {}
                self._next_ok = time.monotonic() + self.min_delay
                if e.code in (429, 500, 502, 503, 504):
                    retry_after = headers.get("retry-after")
                    delay = float(retry_after) if retry_after and str(retry_after).isdigit() else min(60.0, self.min_delay * (2**attempt))
                    print(f"  HTTP {e.code}; backing off {delay:.1f}s")
                    time.sleep(delay)
                    continue
                raise
            except Exception as e:
                last_err = e
                self._next_ok = time.monotonic() + self.min_delay
                delay = min(60.0, self.min_delay * (2**attempt))
                print(f"  error {e!r}; backing off {delay:.1f}s")
                time.sleep(delay)
        raise RuntimeError(f"gave up after {MAX_RETRIES} tries: {url} ({last_err})")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_manifest(path: Path) -> dict:
    if path.exists():
        return json.loads(path.read_text())
    return {
        "source": "ksf.surf",
        "game": "66t",
        "mode": "fw",
        "zone": 0,
        "map": None,
        "fetched_at": None,
        "records": [],
    }


def save_manifest(path: Path, manifest: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(manifest, indent=2, sort_keys=False) + "\n")


def fetch_records(
    client: ThrottledClient,
    map_name: str,
    max_rank: int,
) -> list[dict]:
    """Walk leaderboard pages. offset=0 is empty; start at 1, step 10."""
    seen_files: set[str] = set()
    rows: list[dict] = []
    offset = 1

    while offset <= max_rank + 20:
        url = (
            f"{BASE}/api/maps/{urllib.parse.quote(map_name)}/records/zone/0/"
            f"{offset}?game=66t&mode=fw"
        )
        print(f"GET records offset={offset}")
        status, _, body = client.get(url)
        if status != 200:
            raise RuntimeError(f"records HTTP {status}: {url}")
        page = json.loads(body.decode("utf-8"))
        if not isinstance(page, list):
            raise RuntimeError(f"unexpected records payload: {type(page)}")
        if not page:
            break

        ranks = [int(r.get("rank") or 0) for r in page]
        min_rank = min(ranks) if ranks else 0
        if min_rank > max_rank:
            break

        for row in page:
            rank = int(row.get("rank") or 0)
            file_name = row.get("file")
            if rank < 1 or rank > max_rank or not file_name:
                continue
            if file_name in seen_files:
                continue
            seen_files.add(file_name)
            rows.append(row)

        if max(ranks) >= max_rank:
            break
        offset += 10

    rows.sort(key=lambda r: (int(r.get("rank") or 10**9), float(r.get("time") or 1e9)))
    return rows


def download_file(client: ThrottledClient, file_name: str, dest: Path) -> dict:
    url = f"{BASE}/api/replays/{urllib.parse.quote(file_name)}?game=66t"
    if dest.exists() and dest.stat().st_size > 0:
        data = dest.read_bytes()
        digest = sha256_bytes(data)
        print(f"  skip existing {dest.name} ({len(data)} bytes, sha256={digest[:12]}…)")
        return {
            "path": str(dest.relative_to(repo_root())),
            "bytes": len(data),
            "sha256": digest,
            "url": url,
            "downloaded": False,
        }

    print(f"GET {file_name}")
    status, headers, body = client.get(url)
    if status != 200:
        raise RuntimeError(f"replay HTTP {status}: {url}")
    ctype = headers.get("content-type", "")
    if "html" in ctype.lower():
        raise RuntimeError(f"got HTML instead of binary for {url}")
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(body)
    digest = sha256_bytes(body)
    print(f"  wrote {dest.name} ({len(body)} bytes, sha256={digest[:12]}…)")
    return {
        "path": str(dest.relative_to(repo_root())),
        "bytes": len(body),
        "sha256": digest,
        "url": url,
        "downloaded": True,
    }


def record_entry(row: dict, file_meta: dict) -> dict:
    return {
        "rank": row.get("rank"),
        "name": row.get("name"),
        "steamID": row.get("steamID"),
        "playerID": row.get("playerID"),
        "time": row.get("time"),
        "date": row.get("date"),
        "date_at": row.get("date_at"),
        "record_id": row.get("record_id"),
        "file": row.get("file"),
        "source_records_api": (
            f"{BASE}/api/maps/{{map}}/records/zone/0/1?game=66t&mode=fw"
        ),
        **file_meta,
    }


def fetch_map(
    client: ThrottledClient,
    map_name: str,
    max_rank: int,
    limit: int,
) -> int:
    root = repo_root()
    cache_dir = root / "assets" / "replays" / "external" / "ksf" / map_name
    raw_dir = cache_dir / "raw"
    manifest_path = cache_dir / "manifest.json"

    print(f"\n=== {map_name} (top {max_rank}) ===")
    rows = fetch_records(client, map_name, max_rank)
    print(f"{len(rows)} file-backed records within top {max_rank}")

    if limit > 0:
        rows = rows[:limit]

    manifest = load_manifest(manifest_path)
    by_file = {r.get("file"): r for r in manifest.get("records", []) if r.get("file")}

    manifest.update(
        {
            "source": "ksf.surf",
            "game": "66t",
            "mode": "fw",
            "zone": 0,
            "map": map_name,
            "max_rank": max_rank,
            "fetched_at": utc_now(),
            "note": (
                "Raw third-party files for local development only. "
                "Not redistributed. Contact business@ksf.surf before shipping."
            ),
        }
    )

    for row in rows:
        file_name = row["file"]
        dest = raw_dir / file_name
        existing = by_file.get(file_name)
        if (
            existing
            and dest.exists()
            and existing.get("sha256")
            and sha256_bytes(dest.read_bytes()) == existing["sha256"]
        ):
            print(f"  manifest-ok {file_name}")
            by_file[file_name] = record_entry(
                row,
                {
                    "path": existing.get("path") or str(dest.relative_to(root)),
                    "bytes": existing.get("bytes") or dest.stat().st_size,
                    "sha256": existing["sha256"],
                    "url": existing.get("url")
                    or f"{BASE}/api/replays/{urllib.parse.quote(file_name)}?game=66t",
                    "downloaded": False,
                },
            )
            continue
        meta = download_file(client, file_name, dest)
        by_file[file_name] = record_entry(row, meta)

    ordered = sorted(
        by_file.values(),
        key=lambda r: (int(r.get("rank") or 10**9), float(r.get("time") or 1e9)),
    )
    requested_files = {r["file"] for r in rows}
    kept = []
    for entry in ordered:
        path = root / entry["path"] if entry.get("path") else raw_dir / entry["file"]
        if not path.exists():
            continue
        if entry["file"] in requested_files or entry.get("sha256"):
            kept.append(entry)
    manifest["records"] = kept
    manifest["count"] = len(kept)
    save_manifest(manifest_path, manifest)
    print(f"manifest -> {manifest_path.relative_to(root)} ({len(kept)} records)")
    return len(kept)


def resolve_maps(args: argparse.Namespace) -> list[str]:
    maps: list[str] = []
    if args.batch:
        if args.batch not in BATCHES:
            names = ", ".join(sorted(BATCHES))
            raise SystemExit(f"unknown batch {args.batch!r}; choose from: {names}")
        maps.extend(BATCHES[args.batch])
    if args.map:
        maps.append(args.map)
    maps.extend(args.maps)
    # Preserve order, drop dupes.
    seen: set[str] = set()
    out: list[str] = []
    for m in maps:
        if m not in seen:
            seen.add(m)
            out.append(m)
    if not out:
        raise SystemExit("pass --map, --batch, or positional map names")
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "maps",
        nargs="*",
        help="KSF map names (optional; or use --map / --batch)",
    )
    ap.add_argument("--map", default=None, help="Single KSF map name")
    ap.add_argument(
        "--batch",
        choices=sorted(BATCHES),
        help="Named map batch (aesthetic-17, linear-wave1/2, staged-later, m1-corpus)",
    )
    ap.add_argument(
        "--max-rank",
        type=int,
        default=10,
        help="Walk ranks 1..N; download rows that have a replay file",
    )
    ap.add_argument(
        "--limit",
        type=int,
        default=0,
        help="Max files to download per map (0 = all file-backed rows within max-rank)",
    )
    ap.add_argument(
        "--delay",
        type=float,
        default=MIN_DELAY_S,
        help="Minimum seconds between requests",
    )
    args = ap.parse_args()

    maps = resolve_maps(args)
    client = ThrottledClient(min_delay=args.delay)
    totals = []
    for map_name in maps:
        n = fetch_map(client, map_name, args.max_rank, args.limit)
        totals.append((map_name, n))

    print("\n=== summary ===")
    for map_name, n in totals:
        print(f"  {map_name}: {n} cached")
    print(f"maps={len(totals)} files={sum(n for _, n in totals)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
