# surf-oss

A macOS-native, single-player recreation of Counter-Strike's **surf** gamemode:
Source-engine-faithful movement physics (surf + bunnyhop), real community surf map
loading (CS:S/CS:GO BSP), and speedrun timer features — built for Apple Silicon,
with no Source engine and no networking.

## Status

**M0–M3 complete** — physics, real BSP map loading, timer/zones/replays, textures
and lightmaps, procedural audio.

## Setup

Map BSPs are **not** committed (17 MB–175 MB each, ~1.4 GB for the full corpus).
Fetch them first — they come from the public [fastdl.me](https://main.fastdl.me)
mirror and are community-made maps, *not* Valve content, so no game install is
needed to obtain them:

```bash
python3 tools/fetch_maps.py --batch tests   # what `cargo test` needs (~700 MB)
python3 tools/fetch_maps.py --all           # full corpus (~1.4 GB)
python3 tools/fetch_maps.py --verify        # re-check on-disk files, no network
```

Every download is verified against the sha256 in
[`assets/maps/manifest.json`](assets/maps/manifest.json). Those hashes pin the
exact recompiles that `assets/zones/*.json` and the resim baselines were derived
from — a mirror serving a different build of a map would silently move spawns and
zone volumes.

```bash
cargo test --workspace --release              # requires --batch tests maps
cargo run -p surf-app --release               # launch surf-oss
cargo build -p surf-app --release --bin surf-oss
./target/release/surf-oss
```

The map-loading tests deliberately **fail** rather than skip when a BSP is
missing, so absent assets can't quietly hide a regression behind a green run.

**Optional — stock CS:S/HL2 textures.** Most corpus maps embed their custom
assets in the BSP pakfile and render standalone. Faces using Valve *stock*
materials need a real game install: set `SURF_OSS_GAME_DIR` to it. This is the
only part that requires owning Counter-Strike: Source.

### Upgrading from mx-surf

The game launches fullscreen by default. Use `surf-oss --windowed` for a window,
or `surf-oss --size 1280x800` for a specific window size.

The executable is now `surf-oss`. Rename any `MX_SURF_*` environment variables
in your shell or launch scripts to `SURF_OSS_*` (for example,
`SURF_OSS_GAME_DIR` and `SURF_OSS_ASSETS`). Rebuild any locally installed binary.
The Cargo package remains `surf-app`.

Quit the old app before running the new version. On first data access, it moves
`~/Library/Application Support/mx-surf/` to
`~/Library/Application Support/surf-oss/`, preserving settings, PBs, run history,
savelocs, and replays. An older `osx-surf/` directory is used only if `mx-surf/`
is absent. An existing `surf-oss/` directory is never merged or overwritten;
migration errors stop access and report the affected paths. Replay files keep
their `.osxr` extension and format. Stored replay paths and custom ghost selections
using the old data directory resolve to the new location automatically. Your
checkout folder can keep its old name.

### Feel-check controls

| Input | Action |
|---|---|
| Click | Capture mouse |
| WASD | Move / strafe |
| Mouse | Look (default sens 5.0 × `m_yaw`/`m_pitch` 0.022; axes equal) |
| `[` / `]` | Lower / raise sens |
| `-` / `=` | Lower / raise airaccelerate (A/B vs stock aa 10) |
| Space | Jump (autobhop on) |
| R | Reset to spawn |
| Esc | Release mouse / quit |

On-screen: **blue/green speed bar**, **gold sync bar**, corner pip (green=air, red=ground). Numbers also in the window title.

Graybox course: high drop-in → long striped V → mid pad → one-sided ramp → short V → end pad.

**macOS mouse:** turn off pointer acceleration (System Settings → Mouse), or
`defaults write -g com.apple.mouse.scaling -integer -1`.

## Docs

- `docs/M1-HANDOFF.md` — M1 onboarding (summit, checklist, order)
- `docs/research/00-summary.md` — synthesis, stack, roadmap
- `docs/research/01-movement-physics.md` — physics bible
- `docs/research/02-map-format.md` — BSP import (§8.2 MVP, §8.4 gotchas)
- `CLAUDE.md` / `AGENTS.md` — locked decisions + conventions

## Layout

```
crates/surf-core    pure f32 physics (zero I/O)
crates/surf-render  wgpu flat-shaded mesh + HUD bars
crates/surf-app     winit loop, 66.67 Hz tick + interpolation
crates/surf-map     BSP v19/20/21 → collision brushes, mesh, materials
crates/surf-audio   procedural f32 DSP (zero audio assets)
assets/maps/        BSP corpus — gitignored; manifest.json + fetch_maps.py
assets/zones/       timer zone volumes (tracked)
docs/research/      research reports
docs/reference/     read-only upstream (do not copy into game code)
docs/M1-HANDOFF.md  next-milestone brief
```
