# osx-surf

A macOS-native, single-player recreation of Counter-Strike's **surf** gamemode:
Source-engine-faithful movement physics (surf + bunnyhop), real community surf map
loading (CS:S/CS:GO BSP), and speedrun timer features — built for Apple Silicon,
with no Source engine and no networking.

## Status

**M0 complete** — physics + graybox feel-check. **Next: M1** (real BSP maps).

→ New implementers for map loading: **[`docs/M1-HANDOFF.md`](docs/M1-HANDOFF.md)**  
→ First map preference: **`surf_summit` (CS:S)** (not yet in `assets/maps/`; see handoff)

```bash
cargo test --workspace
cargo run -p surf-app --release   # graybox M0
```

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
crates/surf-map     (M1 — not created yet)
assets/maps/        BSP test corpus (+ surf_summit when acquired)
docs/research/      research reports
docs/reference/     read-only upstream (do not copy into game code)
docs/M1-HANDOFF.md  next-milestone brief
```
