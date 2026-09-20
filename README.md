# surf-oss

This is a standalone, single-player implementation I've been using to surf on my Mac.
I've spent a lot of time in-game getting the surf to feel like CS: Source.
It supports a selection of real surf maps, with practice tools, timers, PBs,
replays and ghosts.

## [Gameplay Video (YouTube)](https://www.youtube.com/watch?v=ypqafhi-wXk)

<a href="https://www.youtube.com/watch?v=ypqafhi-wXk">
  <img src="https://img.youtube.com/vi/ypqafhi-wXk/maxresdefault.jpg" alt="Watch surf-oss gameplay on YouTube — Cyberwave's neon skyline and surf ramps" width="960">
</a>

---

## Setup

**You need to own Counter-Strike: Source and download its files through Steam on
your Mac.** surf-oss uses textures, models and other stock assets from that
installation to load the maps. You don't need to launch CS:S; once setup is done,
neither CS:S nor Steam needs to be running, and you can play offline.

The steps below build surf-oss from source on an **Apple Silicon Mac**. It has
been tested on macOS Tahoe.

1. **Install Counter-Strike: Source.**

   Download [Counter-Strike: Source](https://store.steampowered.com/app/240/CounterStrike_Source/)
   from your Steam library. The warning that the game cannot run on current
   macOS concerns the original game; surf-oss only needs its downloaded content.
   Let the download finish before continuing. The installed game folder should
   contain both `cstrike/` and `hl2/`.

2. **Build surf-oss.**

   Install Apple's Command Line Tools with `xcode-select --install`, and
   [Rust and Cargo](https://rustup.rs/). Then, from a terminal:

   ```sh
   git clone https://github.com/massiminoe/surf-oss.git
   cd surf-oss
   ./tools/package_macos.sh
   ```

   If you already have this checkout, run just the packaging command from its root.
   For subsequent builds, pass a new output directory, such as
   `./tools/package_macos.sh target/dist-next`.

3. **Launch the app.**

   ```sh
   open target/dist/surf-oss.app
   ```

   You can also move `surf-oss.app` to Applications and launch it from Finder.
   The app bundle includes the icon used by the Dock and app switcher. Launching
   the bare executable directly is still supported for terminal diagnostics.

4. **Complete the setup window, then choose “Play”.**

   A separate setup window opens automatically when CS:S isn't detected or no
   maps are installed. You can also open it from **Set up / refresh content**
   on the main menu. Choose **Set up / refresh** in that window to begin.

   The window tracks three numbered steps, keeping completed checks visible:

   - **Counter-Strike: Source** — shows a checkmark and the detected folder.
     External Steam libraries are detected too. If CS:S can't be found, starting
     setup opens a folder picker; choose your installed **Counter-Strike Source**
     folder. The app remembers your selection.
   - **Maps and timing zones** — installs the 16 community maps and their zones,
     showing which map is being downloaded, unpacked or verified.
   - **Leaderboards and replays** — fetches KSF rankings and available top-10
     ghost replays.

   Scroll the activity history to review progress and errors. **Hide window**,
   Esc, or closing the setup window leaves downloads running while the app stays
   open; reopen it from the main menu to check progress. When setup reports
   **Ready to surf**, choose **Done**, then **Play** on the main menu.

   Allow several minutes and at least **3 GB of free space** for surf-oss content,
   in addition to the CS:S installation. You can play installed maps while the
   remaining downloads continue. No Python is needed.

### Before you surf: turn off pointer acceleration

> [!IMPORTANT]
> **Turn off pointer acceleration for consistent mouse feel.** In macOS, open
> **System Settings → Mouse → Advanced**, then turn **Pointer acceleration** off.
> With it enabled, the same mouse distance can turn you by different amounts
> depending on how quickly you move it, making strafing harder to control.
> This setting affects your mouse across macOS. [Apple's instructions](https://support.apple.com/guide/mac-help/mchlp1138/mac).

### If setup is interrupted

Use **Cancel setup** to stop downloads. After cancellation or a failure, choose
**Retry setup** in the setup window. If you quit the app, reopen it and choose
**Set up / refresh content**, then **Set up / refresh**. Partial map downloads
resume, and verified maps and imported replays are reused. Existing conflicting
maps are preserved and reported rather than overwritten.

<details>
<summary>Terminal setup and diagnostics</summary>

```sh
./target/release/surf-oss --setup
./target/release/surf-oss --check
./target/release/surf-oss --setup --game-dir "/Volumes/Games/Steam/steamapps/common/Counter-Strike Source"
```

`--check` verifies CS:S archives, map hashes, zones and cached replays.
Use `--windowed`, `--size 1280x800`, or a short map name (`surf-oss summit`).
`--help` lists all launch options.

</details>

## Maps, leaderboards and replays

### Maps

Setup downloads these **16 community maps**:

- andromeda · aquaflow · boreas · botanica
- cannonball · cement · cyberwave · demise
- fornax · frost · lovetunnel · lux
- overgrowth · summit · tendies · void

Maps come from [fastdl.me](https://main.fastdl.me). Exact builds are pinned in
[the map catalog](assets/maps/manifest.json) and verified with SHA-256. Timer zones
ship with surf-oss; stock assets come from your own CS:S installation.

### Leaderboards

Setup downloads **KSF CS:S 66-tick top-10 records** for each supported map.
The in-app **Leaderboard** shows these alongside your local run history.
Choose **Set up / refresh content** whenever you want to update them.

Records are cached for offline use. If live records aren't available, setup can
use a dated snapshot, labeled in the leaderboard. surf-oss runs aren't submitted
to [KSF](https://ksf.surf), and its movement defaults differ from KSF server rules.

### Replays and ghosts

Setup also downloads **available replays from those KSF records**. Not every
record has one; select a leaderboard row marked **replay** to watch it.
Replay files download directly from KSF and remain available offline.

Your own official finishes save a time and replay automatically. Practice runs
don't replace your PBs. To race against a ghost, choose one in **Settings → Ghost**.

## Your data

Settings, times, practice locations and personal replays live in
`~/Library/Application Support/surf-oss/`. Downloads normally live in its
`content/` folder; populated source checkouts keep their existing `assets/` tree.

<details>
<summary>Use a separate profile</summary>

To try a fresh setup without touching your existing profile or checkout assets,
launch with a new profile directory. This opens the in-app setup window:

```sh
./target/release/surf-oss --windowed --data-dir /tmp/surf-oss-fresh-profile
```

To set up and verify that profile from the terminal instead:

```sh
./target/release/surf-oss --setup --data-dir /tmp/surf-oss-fresh-profile
./target/release/surf-oss --check --data-dir /tmp/surf-oss-fresh-profile
./target/release/surf-oss --data-dir /tmp/surf-oss-fresh-profile
```

This example uses a temporary directory; choose a permanent location for a
profile you want to keep.

</details>

## License

Code: [MIT](LICENSE). Bundled fonts: [SIL Open Font License](assets/fonts/licenses/).
No Valve assets, downloaded maps, third-party replay files or personal data are
included in the app package.
