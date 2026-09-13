# surf-oss

Standalone, single-player surf for Apple Silicon Macs: 16 community maps,
practice locations, timers, personal bests, a KSF leaderboard, ghosts and replays.

**Install Counter-Strike: Source through Steam, then open surf-oss. It finds your
CS:S content and downloads the maps, records and available replays. After setup,
you can play offline. Neither Steam nor CS:S needs to be running.**

## Build and play

The source build is the tested installation path. You need:

- An Apple Silicon Mac (tested on macOS Tahoe).
- Apple's Command Line Tools: `xcode-select --install`.
- [Rust and Cargo](https://rustup.rs/) (tested with Rust 1.97.1).
- Installed CS:S content: the game folder must contain `cstrike/` and `hl2/`.
  Ownership alone is not enough; download the files through Steam. The original
  game does not need to run on your Mac.

From this checkout:

```sh
cargo build --locked --release -p surf-app --bin surf-oss
./target/release/surf-oss
```

Choose **Set up / refresh content**, then **Play**. Setup downloads all 16 maps,
KSF CS:S 66-tick top-10 records, and every available replay in those records.
Installed maps are playable while the remaining downloads continue. Allow several
minutes and at least 3 GB of free space; slow connections take longer. No Python
is needed for this path.

Steam libraries are detected automatically, including external libraries. If
content is not found, setup opens a folder picker. Choose the installed
**Counter-Strike Source** folder. A valid selection is remembered.

Terminal equivalents and diagnostics:

```sh
./target/release/surf-oss --setup
./target/release/surf-oss --check
./target/release/surf-oss --setup --game-dir "/Volumes/Games/Steam/steamapps/common/Counter-Strike Source"
```

`--check` verifies CS:S archives, map hashes, zones and cached replays. Retry setup
after a network failure; verified maps and imported replays are reused.
Interrupted map downloads resume from the retained download cache. Existing conflicting maps
are preserved and reported rather than overwritten.

Use `--windowed`, `--size 1280x800`, or a short map name (`surf-oss summit`).
`--help` lists all launch options.

## Maps and records

**andromeda · aquaflow · boreas · botanica · cannonball · cement · cyberwave ·
demise · fornax · frost · lovetunnel · lux · overgrowth · summit · tendies · void**

Maps come from [fastdl.me](https://main.fastdl.me); their exact builds are pinned
in [the catalog](assets/maps/manifest.json) and verified with SHA-256. Zones ship
with the app. Stock assets are read from your own CS:S installation.

**Leaderboard** combines cached [KSF](https://ksf.surf) records with your local
history. Select a row marked **replay** to watch it. Records without a replay
still appear. **Set up / refresh content** updates the cache; an outage does not
prevent offline play. If live records are unavailable, a dated metadata snapshot
is used and labeled as such. Replay binaries still download directly from KSF.
surf-oss runs are not submitted to KSF, and its movement
defaults differ from KSF server rules.

Official finishes save your time and replay automatically. Practice runs do not
replace PBs. Choose a racing ghost in Settings → Ghost.

## Controls

- WASD: move; mouse: look; Space: autobhop; Ctrl: duck.
- R: restart run; T: restart stage; Esc: pause/menu.
- P: practice mode, off at each launch.
- Right mouse: save a location; left mouse: load it when practice is enabled.
- Settings → Keybinds: change movement and practice keys.

Click the game to capture the mouse. Disable pointer acceleration in macOS Mouse
settings for consistent feel. Procedural audio is adjustable in Settings.

## Your data

Settings, times, locations and personal replays live in
`~/Library/Application Support/surf-oss/`. Downloads normally live in its
`content/` folder; populated source checkouts keep their existing `assets/` tree.

For a fresh setup **without touching your existing profile or checkout assets**,
choose a new directory and use it for both setup and play:

```sh
./target/release/surf-oss --setup --data-dir /tmp/surf-oss-fresh-profile
./target/release/surf-oss --check --data-dir /tmp/surf-oss-fresh-profile
./target/release/surf-oss --data-dir /tmp/surf-oss-fresh-profile
```

## Sharing and development

[Development notes](DEVELOPMENT.md) cover tests, Mac app packaging and upgrades.
A packaged app needs no Rust or Python, but Apple signing/notarization remains a
release step. Installing CS:S through a **fresh Steam installation on current
macOS** has not yet been verified; setup has been tested against existing CS:S
content in a separate fresh profile.

Code: [MIT](LICENSE). Bundled fonts: [SIL Open Font License](assets/fonts/licenses/).
No Valve assets, downloaded maps, third-party replay files or personal data are
included in the app package.
