//! Downloaded content is separate from the player's settings and run history.
//! macOS supplies curl, shasum, and bzip2; setup needs neither Python nor Steam login.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn cancel() {
    CANCELLED.store(true, std::sync::atomic::Ordering::Relaxed);
}
fn check_cancelled() -> Result<(), String> {
    if CANCELLED.load(std::sync::atomic::Ordering::Relaxed) {
        Err("Setup cancelled; downloaded content preserved.".into())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct MapAsset {
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Deserialize)]
pub struct Catalog {
    pub maps: BTreeMap<String, MapAsset>,
}
pub fn catalog() -> Catalog {
    serde_json::from_str(include_str!("../../../assets/maps/manifest.json"))
        .expect("embedded map catalog")
}
pub fn zones(map: &str) -> Option<&'static str> {
    macro_rules! zone { ($($name:literal),*) => { match map { $(concat!("surf_", $name) => Some(include_str!(concat!("../../../assets/zones/surf_", $name, ".json"))),)* _ => None } }; }
    zone!(
        "andromeda",
        "aquaflow",
        "boreas",
        "botanica",
        "cannonball",
        "cement",
        "cyberwave",
        "demise",
        "fornax",
        "frost",
        "lovetunnel",
        "lux",
        "overgrowth",
        "summit",
        "tendies",
        "void"
    )
}
fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
fn temporary(path: &Path) -> PathBuf {
    path.with_extension(format!("{}.{}.part", std::process::id(), stamp()))
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("missing parent")?).map_err(|e| e.to_string())?;
    let tmp = temporary(path);
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}
fn run(command: &mut Command) -> Result<std::process::Output, String> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        if let Err(error) = check_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out)
}
fn download(
    url: &str,
    dest: &Path,
    label: &str,
    progress: &mut impl FnMut(String),
    resume: bool,
) -> Result<(), String> {
    fs::create_dir_all(dest.parent().ok_or("missing parent")?).map_err(|e| e.to_string())?;
    let errors = temporary(dest);
    let error_file = fs::File::create(&errors).map_err(|e| e.to_string())?;
    let mut command = Command::new("/usr/bin/curl");
    command
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "20",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--retry-max-time",
            "240",
            "--retry",
            "3",
            "--retry-all-errors",
            "--retry-delay",
            "2",
            "--user-agent",
            "surf-oss/0.1 (content setup)",
            "--output",
        ])
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(error_file);
    if resume && dest.metadata().is_ok_and(|m| m.len() > 0) {
        command.args(["--continue-at", "-"]);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut last_update = std::time::Instant::now();
    loop {
        if let Err(error) = check_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(fs::read_to_string(&errors)
                    .unwrap_or_else(|_| format!("Download failed: {status}")))
            };
        }
        if last_update.elapsed() >= Duration::from_secs(2) {
            let bytes = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            progress(format!(
                "Downloading {label} · {:.1} MB",
                bytes as f64 / 1_000_000.0
            ));
            last_update = std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn verify(path: &Path, asset: &MapAsset) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != asset.bytes {
        return Err("size differs from supported map build".into());
    }
    let out = run(Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path))?;
    if String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        != Some(&asset.sha256)
    {
        return Err("SHA-256 differs from supported map build".into());
    }
    Ok(())
}
/// Called before starting threads. A profile override never migrates the normal profile.
pub fn configure_game_dir() {
    if std::env::var_os("SURF_OSS_GAME_DIR").is_none() {
        if let Ok(path) = fs::read_to_string(crate::assets::root().join("game-dir.txt")) {
            surf_map::stock::select_game_dir(PathBuf::from(path.trim()));
        }
    }
}
/// The GUI can recover from a missing/nonstandard Steam install without a shell.
pub fn setup_interactive(progress: &mut impl FnMut(String)) -> Result<(), String> {
    if surf_map::stock::discover_game_dir()
        .is_none_or(|p| surf_map::stock::validate_game_dir(&p).is_err())
    {
        progress(
            "Locate the installed Counter-Strike Source folder (contains cstrike and hl2).".into(),
        );
        let output = run(Command::new("/usr/bin/osascript").args(["-e", "POSIX path of (choose folder with prompt \"Choose your installed Counter-Strike Source folder (contains cstrike and hl2)\")"]))
            .map_err(|_| "Setup cancelled. Install CS:S through Steam, then choose Set up / refresh content again.".to_string())?;
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        surf_map::stock::validate_game_dir(&path)?;
        surf_map::stock::select_game_dir(path);
    }
    setup(progress)
}
pub fn check(progress: &mut impl FnMut(String)) -> Result<(), String> {
    let game = surf_map::stock::discover_game_dir()
        .ok_or("CS:S not found. Install it through Steam, or use --game-dir PATH.")?;
    surf_map::stock::validate_game_dir(&game)?;
    progress(format!("CS:S: {}", game.display()));
    let mut missing = Vec::new();
    for (map, asset) in catalog().maps {
        let status = verify(
            &crate::assets::maps_dir().join(format!("{map}.bsp")),
            &asset,
        );
        if let Err(e) = status {
            missing.push(format!("{map}: {e}"));
        } else {
            progress(format!("Verified {map}"));
        }
        match crate::zones::load_zones_file(&crate::assets::zones_dir().join(format!("{map}.json")))
        {
            Ok(zones) if zones.map_name == map => {}
            _ => missing.push(format!("{map}: missing or invalid zones")),
        }
        let records = crate::leaderboard::ksf_records(&map);
        if records.is_empty() {
            missing.push(format!("{map}: leaderboard not cached"));
        }
        for record in records.iter().filter(|r| !r.file_stem.is_empty()) {
            let path =
                crate::replay::ksf_imported_dir(&map).join(format!("{}.osxr", record.file_stem));
            if crate::replay::Replay::load(&path).is_err() {
                missing.push(format!(
                    "{map}: replay #{} not available locally",
                    record.rank
                ));
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}\nRun --setup to download missing content.",
            missing.join("\n")
        ))
    }
}
/// Idempotent setup: valid maps are reused; conflicting existing BSPs are never replaced.
pub fn setup(progress: &mut impl FnMut(String)) -> Result<(), String> {
    let game = surf_map::stock::discover_game_dir().ok_or("CS:S not found. Install through Steam, or launch with --game-dir PATH (folder containing cstrike and hl2).")?;
    surf_map::stock::validate_game_dir(&game)?;
    let root = crate::assets::root();
    write_atomic(
        &root.join("game-dir.txt"),
        game.to_string_lossy().as_bytes(),
    )?;
    let mut failures = Vec::new();
    let maps = catalog().maps;
    // Summit first; every map remains in the catalog and is installed in this run.
    let mut names: Vec<_> = maps.keys().collect();
    names.sort_by_key(|m| (m.as_str() != "surf_summit", m.as_str()));
    for (i, map) in names.iter().enumerate() {
        check_cancelled()?;
        progress(format!("Map {}/{}: {map}", i + 1, names.len()));
        if let Err(e) = install_map(root, map, &maps[*map], progress) {
            progress(format!("Could not install {map}: {e}"));
            failures.push(format!("{map}: {e}"));
        }
    }
    if let Err(error) = refresh_records(progress) {
        failures.push(error);
    }
    if failures.is_empty() {
        progress("Ready: all 16 maps, leaderboards and available top-10 replays.".into());
        Ok(())
    } else {
        Err(format!(
            "Setup incomplete; installed content is usable. Retry Set up / refresh content.\n{}",
            failures.join("\n")
        ))
    }
}
/// Refresh cached rankings and available replays without downloading maps again.
pub fn refresh_records(progress: &mut impl FnMut(String)) -> Result<(), String> {
    let root = crate::assets::root();
    let maps = catalog().maps;
    let names: Vec<_> = maps.keys().collect();
    let mut failures = Vec::new();
    for (i, map) in names.iter().enumerate() {
        check_cancelled()?;
        progress(format!(
            "Leaderboard and replays {}/{}: {map}",
            i + 1,
            names.len()
        ));
        if let Err(e) = sync_records(root, map, progress) {
            progress(format!("Could not update KSF {map}: {e}"));
            failures.push(format!("KSF {map}: {e}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn install_map(
    root: &Path,
    map: &str,
    asset: &MapAsset,
    progress: &mut impl FnMut(String),
) -> Result<(), String> {
    let path = root.join("maps").join(format!("{map}.bsp"));
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if path.exists() {
        verify(&path, asset).map_err(|e| {
            format!(
                "{}: {e}. Existing file preserved; use a separate --data-dir for a fresh install.",
                path.display()
            )
        })?;
    } else {
        let compressed = root.join("downloads").join(format!("{map}.bsp.bz2"));
        let transfer = download(
            &format!("https://main.fastdl.me/mapsredir/{map}.bsp.bz2"),
            &compressed,
            map,
            progress,
            true,
        );
        check_cancelled()?;
        let unpacked = temporary(&path);
        let output = fs::File::create(&unpacked).map_err(|e| e.to_string())?;
        let result = Command::new("/usr/bin/bzip2")
            .arg("-dc")
            .arg(&compressed)
            .stdout(output)
            .output()
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(transfer
                .err()
                .unwrap_or_else(|| "map decompression failed; cached download preserved".into()));
        }
        verify(&unpacked, asset)?;
        fs::rename(&unpacked, &path).map_err(|e| e.to_string())?;
        // Download scratch files are left intact; setup never deletes local files.
    }
    let zone = root.join("zones").join(format!("{map}.json"));
    if !zone.exists() {
        write_atomic(&zone, zones(map).ok_or("missing bundled zones")?.as_bytes())?;
    }
    Ok(())
}
fn safe_file(file: &str) -> bool {
    !file.is_empty()
        && file.ends_with(".rec")
        && file
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        && !file.contains("..")
}
fn normalize_records(rows: Value) -> Result<Vec<Value>, String> {
    let rows = rows
        .as_array()
        .ok_or("KSF returned an unexpected response")?;
    let mut result: Vec<_> = rows
        .iter()
        .filter(|r| {
            r["rank"]
                .as_u64()
                .is_some_and(|rank| (1..=10).contains(&rank))
                && r["time"].as_f64().is_some_and(|t| t.is_finite() && t > 0.0)
        })
        .cloned()
        .collect();
    for row in &mut result {
        if !row["file"].as_str().is_some_and(safe_file) {
            row["file"] = Value::String(String::new());
        }
    }
    result.sort_by_key(|r| r["rank"].as_u64());
    Ok(result)
}
fn record_manifest(
    map: &str,
    live: Result<Vec<Value>, String>,
    cached: Option<Value>,
) -> Result<Value, String> {
    if let Ok(rows) = live {
        if !rows.is_empty() {
            return Ok(
                json!({"source":"ksf.surf", "game":"66t", "mode":"fw", "map":map, "fetched_at_unix":stamp()/1_000_000_000, "records":rows}),
            );
        }
    }
    let seed: Value =
        serde_json::from_str(include_str!("../../../assets/replays/ksf-records.json"))
            .expect("bundled public record metadata");
    let mut fallback = cached
        .filter(|v| normalize_records(v["records"].clone()).is_ok_and(|r| !r.is_empty()))
        .unwrap_or_else(|| seed["maps"][map].clone());
    if normalize_records(fallback["records"].clone())?.is_empty() {
        return Err("No live or cached records available".into());
    }
    fallback["snapshot"] = Value::Bool(true);
    Ok(fallback)
}

fn sync_records(root: &Path, map: &str, progress: &mut impl FnMut(String)) -> Result<(), String> {
    let dir = root.join("replays/external/ksf").join(map);
    let manifest = dir.join("manifest.json");
    let response = temporary(&manifest);
    std::thread::sleep(Duration::from_secs(1));
    let live = download(
        &format!("https://ksf.surf/api/maps/{map}/records/zone/0/1?game=66t&mode=fw"),
        &response,
        &format!("{map} leaderboard"),
        progress,
        false,
    )
    .and_then(|()| {
        normalize_records(
            serde_json::from_slice(&fs::read(&response).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
        )
    });
    check_cancelled()?;
    let cached = fs::read(&manifest)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let document = record_manifest(map, live, cached)?;
    if document["snapshot"] == true {
        progress(format!(
            "{map}: live records unavailable; using a dated snapshot"
        ));
    }
    let rows = normalize_records(document["records"].clone())?;
    write_atomic(&manifest, &serde_json::to_vec_pretty(&document).unwrap())?;
    let mut errors = Vec::new();
    for row in &rows {
        check_cancelled()?;
        let file = row["file"].as_str().unwrap_or_default();
        if file.is_empty() {
            continue;
        }
        progress(format!("Replay {map} #{}", row["rank"]));
        if let Err(e) = install_replay(&dir, map, row, progress) {
            errors.push(format!("{file}: {e}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
fn install_replay(
    dir: &Path,
    map: &str,
    row: &Value,
    progress: &mut impl FnMut(String),
) -> Result<(), String> {
    let file = row["file"].as_str().ok_or("missing replay name")?;
    if !safe_file(file) {
        return Err("invalid replay name".into());
    }
    let dest = dir
        .join("imported")
        .join(format!("{}.osxr", file.trim_end_matches(".rec")));
    if dest.exists() {
        crate::replay::Replay::load(&dest)?;
        return Ok(());
    }
    let raw = dir.join("raw").join(file);
    let source = if raw.exists() {
        raw.clone()
    } else {
        std::thread::sleep(Duration::from_secs(1));
        let temp = temporary(&raw);
        download(
            &format!("https://ksf.surf/api/replays/{file}?game=66t"),
            &temp,
            &format!("{map} replay #{}", row["rank"]),
            progress,
            false,
        )?;
        temp
    };
    if let Some(hash) = row["sha256"].as_str() {
        verify(
            &source,
            &MapAsset {
                bytes: fs::metadata(&source).map_err(|e| e.to_string())?.len(),
                sha256: hash.to_string(),
            },
        )?;
    }
    let zones = crate::zones::parse_zones_json(zones(map).ok_or("missing zones")?)
        .map_err(|e| format!("zones: {e}"))?;
    let (replay, _, _) = crate::ksf_replay::import_ksf_to_replay(
        &source,
        map,
        &zones,
        row["time"].as_f64().map(|t| t as f32),
    )
    .map_err(|e| e.to_string())?;
    if source != raw {
        fs::rename(&source, &raw).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = temporary(&dest);
    replay.save(&temp)?;
    fs::rename(temp, dest).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_catalog_has_matching_zones() {
        assert_eq!(catalog().maps.len(), 16);
        for map in catalog().maps.keys() {
            let zone: Value = serde_json::from_str(zones(map).unwrap()).unwrap();
            assert_eq!(zone["mapName"].as_str(), Some(map.as_str()));
        }
    }
    #[test]
    fn conflicting_local_map_is_preserved() {
        let root = std::env::temp_dir().join(format!("surf-content-preserve-{}", stamp()));
        fs::create_dir_all(root.join("maps")).unwrap();
        let path = root.join("maps/surf_summit.bsp");
        fs::write(&path, b"a user's different map build").unwrap();
        let error = install_map(
            &root,
            "surf_summit",
            &catalog().maps["surf_summit"],
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.contains("Existing file preserved"));
        assert_eq!(fs::read(path).unwrap(), b"a user's different map build");
    }

    #[test]
    fn empty_api_uses_dated_metadata_without_inventing_current_records() {
        for map in catalog().maps.keys() {
            let snapshot = record_manifest(map, Ok(Vec::new()), None).unwrap();
            assert_eq!(snapshot["snapshot"], true);
            assert!(snapshot["fetched_at"].is_string());
            assert!(snapshot["fetched_at_unix"].is_null());
            assert!(!normalize_records(snapshot["records"].clone())
                .unwrap()
                .is_empty());
        }
        let cached = json!({"records":[{"rank":1,"time":30.0,"file":""}],"fetched_at_unix":123});
        let snapshot =
            record_manifest("surf_cyberwave", Err("offline".into()), Some(cached)).unwrap();
        assert_eq!(snapshot["fetched_at_unix"], 123);
        assert_eq!(snapshot["records"][0]["file"], "");
    }

    #[test]
    fn rankings_do_not_depend_on_replay_availability() {
        let rows = normalize_records(json!([
            {"rank":2,"time":42.0,"file":"good.rec"},
            {"rank":1,"time":41.0,"file":null},
            {"rank":3,"time":43.0,"file":"../../escape.rec"},
            {"rank":11,"time":44.0,"file":"later.rec"}
        ]))
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["rank"], 1);
        assert_eq!(rows[0]["file"], "");
        assert_eq!(rows[2]["file"], "");
        assert!(normalize_records(json!({"error":"rate limited"})).is_err());
    }
}
