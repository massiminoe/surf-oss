# Development and packaging

## Tests

```sh
python3 tools/fetch_maps.py --all  # Python 3.10+; fixtures in the checkout
cargo test --locked --workspace --release
```

Use release mode: the audio performance checks are invalid in unoptimized builds.
Renderer tests require GPU access. Some historical external-replay tests skip
when the developer KSF cache is absent. In-app setup downloads current KSF
records; it does not reconstruct historical research fixtures.

`assets/maps/manifest.json` defines the 16 supported maps and pins exact builds.
Changing a map hash requires revalidating its zones and physics fixtures.
`tools/ksf_fetch.py --batch all` uses the same catalog; the game provides the
normal download-and-import workflow without Python.

Crates: `surf-core` (physics), `surf-map` (BSP/assets), `surf-render` (wgpu/HUD),
`surf-audio` (procedural audio), `surf-app` (game, content setup, timer/replays).

## Package a Mac app

```sh
sh tools/package_macos.sh
```

Outputs `target/dist/surf-oss.app` and `target/dist/surf-oss-macos.zip`. The package
contains the executable (including zones/fonts), README and license notices.
It does not contain downloaded maps, KSF replay files, Valve assets or profiles.
Recipients need CS:S content, but do not need Rust or Python.

The script refuses to overwrite an existing package. Pass a new output directory
for another build, for example `sh tools/package_macos.sh /tmp/surf-build-2`.

The app is ad-hoc signed, **not Apple-notarized**. Developer ID signing,
notarization and the Gatekeeper experience for a downloaded package still need
a release pass before public binary distribution. No signing credentials are
used by this script. Other operating systems are not release-tested.

## Content and isolation

The app downloads directly from fastdl.me and the public KSF endpoints. KSF's API
is undocumented and may change; requests are serial, throttled and retried.
Use `surf-oss --refresh-records` to update only leaderboard data and replays.
Dated public metadata in `assets/replays/ksf-records.json` provides a fallback
when an API result is empty or unavailable. Existing cached metadata takes
precedence over that bundled snapshot; the original date is retained and the
records page labels it as a snapshot. No replay binaries are bundled. Hashes in
historical metadata verify downloaded replay files before import.

No account credentials are collected. Rankings do not depend on replay
availability, and local times are not submitted to KSF.

Setup verifies map hashes before publishing them, writes metadata atomically,
and preserves existing conflicting maps. Failed scratch transfers remain on
disk for inspection. Map downloads use the host's documented
[`/mapsredir/` route](https://fastdl.me/), which redirects to storage with verified
byte-range support. Partial maps resume from the download cache; completed maps
and imported replays are reused. Quitting the app cancels its download worker.

`--data-dir PATH` (or `SURF_OSS_DATA_DIR`) isolates both profile and content,
bypassing legacy migration and development assets. `SURF_OSS_ASSETS` otherwise
selects an asset directory, including a new directory setup can populate.
Mac app bundles use installed content rather than their build checkout.

## Upgrading from mx-surf / osx-surf

Quit the old app first. Normal launch moves the legacy profile only if a
`surf-oss` profile does not already exist. `mx-surf` takes precedence over
`osx-surf`; an existing `surf-oss` profile always wins. Migration errors stop
access rather than silently starting an empty profile.

`--data-dir` never migrates the normal profile. Stored legacy replay paths are
resolved under the migrated profile. The `.osxr` extension and format are
unchanged. Rename old environment variables to `SURF_OSS_*`; the Cargo package
remains `surf-app` and the executable is `surf-oss`.

See [setup validation](SETUP-VALIDATION.md) for the tested environment, results,
retained artifacts and remaining release checks.
