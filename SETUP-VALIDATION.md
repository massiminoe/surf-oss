# Setup validation — 2026-09-13

Tested on an Apple Silicon M4 Pro Mac running macOS Tahoe, with Rust 1.97.1.
The existing CS:S installation was read only. No existing player data was deleted.

## Results

- **242 tests passed** with `cargo test --locked --workspace --release` and GPU access.
- A separate source snapshot built successfully with its own target directory,
  `--locked --offline`, and the existing Cargo dependency cache. No checkout map
  files or developer replay cache were copied into that source snapshot.
- **16/16 maps downloaded and SHA-256 verified** in a separate profile. The fresh
  source build's map loader loaded all 16 successfully.
- **16 leaderboards, 155 records and 74 imported replay files** were cached.
  These counts describe this test, not a fixed promise about future KSF data.
- `surf-oss --check --data-dir ...` exited successfully from both the independent
  source build and the packaged app, checking CS:S archives, maps, zones and
  referenced replay files.
- The independent game launched the freshly downloaded Summit map and rendered
  it for 45 seconds. It also played a newly downloaded/imported Andromeda KSF
  replay for an 18-second smoke test.
- The new menu row was rendered with the real renderer and visually inspected at
  1280×800. Existing rendering tests passed. Native UI inspection was unreliable
  in this session, so this is not a claim of exhaustive interactive menu testing.
- Packaging produced an app and ZIP. `codesign --verify --deep --strict` passed.
  Packaging into the same output directory again was refused without overwriting.

## Problems the fresh test found and fixed

The map host's `/maps/` proxy route was slow and sometimes reset connections.
The host [documents a `/mapsredir/` route](https://fastdl.me/) that leads to its
stored files. A bounded request there returned HTTP 206 with the requested 32
bytes. Setup now uses that route and keeps a resumable compressed download cache.
Tendies resumed from a retained 19,005,440-byte partial download and then passed
its pinned map hash. Every catalog redirect was checked against the object name
for the corresponding downloaded BSP; all 16 matched. Previous partial files
were preserved.

KSF returned an empty leaderboard for Cyberwave despite its replay URLs still
working. Its historical WR replay matched the previously recorded SHA-256.
Setup now falls back to dated metadata, preferring an existing cache over the
bundled public snapshot, and labels snapshots in the records page. The final
cache used snapshots for Cyberwave and a transiently empty Cannonball response.
Replay binaries are still downloaded directly; none are bundled.

## Retained test locations

- Source snapshot: `/tmp/surf-oss-share-source-20260913`
- Isolated profile: `/tmp/surf-oss-share-live-20260913/profile`
- Setup logs: `/tmp/surf-oss-share-live-20260913/`
- Full suite log: `/tmp/surf-share-final-verification.log`
- Map load log: `/tmp/surf-fresh-all-maps.log`
- Content check: `/tmp/surf-complete-check.log`
- Direct-route check: `/tmp/surf-direct-catalog.log`
- Menu captures: `/tmp/surf-setup-menu-shots/`
- Package: `/tmp/surf-oss-share-ready-20260913/surf-oss-macos.zip`

These are local validation artifacts, not repository requirements.

## Remaining release checks

A fresh Steam installation and acquisition of CS:S content on current macOS
were **not** tested. Validation used already-installed CS:S files, rather than
modifying or removing that installation. The supported setup needs those files;
it does not need the original game executable to run.

The app is ad-hoc signed, not Developer-ID signed or notarized. A clean-machine
Gatekeeper test remains before public binary distribution. Other operating
systems and architectures are not release-tested. These checks do not establish
that every map can be completed without a gameplay bug; movement physics was not
changed in this work.
