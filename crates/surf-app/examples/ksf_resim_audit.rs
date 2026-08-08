//! Two-layer KSF replay audit: pose probes (Layer A) + input resim grid (Layer B).
//!
//! ```text
//! cargo run -p surf-app --example ksf_resim_audit --release
//! cargo run -p surf-app --example ksf_resim_audit --release -- --maps surf_summit --preset ksf_css_66t
//! cargo run -p surf-app --example ksf_resim_audit --release -- --write-baselines
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_app::resim::{
    audit_poses, preset_by_name, preset_grid, resim_best_preset, MapBaseline, NamedPreset,
    PoseGates, ResimBaselines, V1_MAPS,
};
use surf_app::zones::{load_zones_file, zones_path_for_map};
use surf_map::LoadedMap;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn map_bsp(root: &Path, map: &str) -> PathBuf {
    root.join("assets/maps").join(format!("{map}.bsp"))
}

fn parse_args(args: &[String]) -> (Vec<String>, Option<String>, bool) {
    let mut maps: Vec<String> = Vec::new();
    let mut preset: Option<String> = None;
    let mut write_baselines = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--maps" => {
                i += 1;
                if i < args.len() {
                    maps = args[i]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
            "--preset" => {
                i += 1;
                if i < args.len() {
                    if args[i] == "all" {
                        preset = None;
                    } else {
                        preset = Some(args[i].clone());
                    }
                }
            }
            "--write-baselines" => write_baselines = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ksf_resim_audit [--maps m1,m2] [--preset all|name] [--write-baselines]"
                );
                std::process::exit(0);
            }
            other => eprintln!("warning: ignoring arg {other}"),
        }
        i += 1;
    }
    if maps.is_empty() {
        maps = V1_MAPS.iter().map(|s| (*s).to_string()).collect();
    }
    (maps, preset, write_baselines)
}

fn list_osxr(root: &Path, map: &str) -> Vec<PathBuf> {
    let dir = root.join(ksf_imported_dir(map));
    let mut files = Vec::new();
    let Ok(rd) = fs::read_dir(&dir) else {
        return files;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("osxr") {
            files.push(p);
        }
    }
    files.sort();
    files
}

fn baselines_path(root: &Path) -> PathBuf {
    root.join("assets/fixtures/ksf_resim_baselines.json")
}

fn load_baselines(root: &Path) -> Option<ResimBaselines> {
    let p = baselines_path(root);
    let text = fs::read_to_string(&p).ok()?;
    serde_json::from_str(&text).ok()
}

fn default_gates() -> PoseGates {
    // Pre-calibration soft defaults; --write-baselines replaces with measured floors.
    PoseGates {
        min_on_ramp_frac: 0.10,
        max_kill_z_frac: 0.001,
        max_buried_frac: 0.05,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (maps, preset_filter, write_baselines) = parse_args(&args);
    let root = repo_root();
    let baselines = load_baselines(&root);

    let presets: Vec<NamedPreset> = if let Some(name) = &preset_filter {
        match preset_by_name(name) {
            Some(p) => vec![p],
            None => {
                eprintln!("unknown preset {name}; known:");
                for p in preset_grid() {
                    eprintln!("  {}", p.name);
                }
                return ExitCode::from(2);
            }
        }
    } else {
        preset_grid()
    };

    println!(
        "ksf_resim_audit  maps={}  presets={}",
        maps.join(","),
        presets.iter().map(|p| p.name).collect::<Vec<_>>().join(",")
    );

    let mut failed = false;
    let mut new_baselines: Vec<MapBaseline> = Vec::new();

    for map_name in &maps {
        let bsp = map_bsp(&root, map_name);
        if !bsp.is_file() {
            eprintln!("SKIP {map_name}: missing {}", bsp.display());
            continue;
        }
        let ghosts = list_osxr(&root, map_name);
        if ghosts.is_empty() {
            eprintln!("SKIP {map_name}: no imported .osxr");
            continue;
        }

        let t0 = Instant::now();
        let loaded = match LoadedMap::load_path(&bsp) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("FAIL {map_name}: load BSP: {e}");
                failed = true;
                continue;
            }
        };
        let zones_path = zones_path_for_map(&bsp);
        let zones = match load_zones_file(&zones_path) {
            Ok(z) => z,
            Err(e) => {
                eprintln!("FAIL {map_name}: zones: {e}");
                failed = true;
                continue;
            }
        };
        println!(
            "\n=== {map_name}  brushes={}  load={:.1}s  ghosts={} ===",
            loaded.world.brushes.len(),
            t0.elapsed().as_secs_f32(),
            ghosts.len()
        );

        let gates = baselines
            .as_ref()
            .and_then(|b| b.pose_gates(map_name))
            .unwrap_or_else(default_gates);

        // Prefer WR-like first file for baseline write; audit all ghosts for Layer A.
        let mut map_best_p95 = f32::MAX;
        let mut map_best_preset = String::new();
        let mut map_on_ramp = 0.0_f32;
        let mut audited = 0usize;

        for ghost_path in &ghosts {
            let stem = ghost_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?");
            let replay = match Replay::load(ghost_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("  FAIL load {stem}: {e}");
                    failed = true;
                    continue;
                }
            };

            let pose = audit_poses(&loaded, &replay.frames);
            audited += 1;
            map_on_ramp = map_on_ramp.max(pose.on_ramp_frac());
            print!(
                "  {stem}: A on-ramp={:.1}% ({}/{})  killz={:.3}%  buried={:.2}%",
                100.0 * pose.on_ramp_frac(),
                pose.on_ramp,
                pose.frames,
                100.0 * pose.kill_z_frac(),
                100.0 * pose.buried_frac(),
            );
            if let Err(e) = gates.check(&pose) {
                println!("  FAIL A: {e}");
                failed = true;
            } else {
                println!("  OK A");
            }

            let t1 = Instant::now();
            let (all, best) =
                resim_best_preset(&loaded, &replay.frames, &zones, &presets);
            let dt = t1.elapsed().as_secs_f32();

            if let Some(ref b) = best {
                println!(
                    "         B best={}  p95={:.1}  max={:.1}  mean={:.1}  surv={:.0}%  end={}  teles={}  ({:.1}s, {} presets)",
                    b.preset,
                    b.p95_origin_err,
                    b.max_origin_err,
                    b.mean_origin_err,
                    100.0 * b.survival_frac(),
                    b.reached_end,
                    b.teleports,
                    dt,
                    all.len()
                );
                if b.catastrophic() {
                    println!("         FAIL B: catastrophic survival {:.0}%", 100.0 * b.survival_frac());
                    failed = true;
                } else if let Some(base) = baselines.as_ref().and_then(|x| x.for_map(map_name)) {
                    if b.p95_origin_err > base.max_p95_origin_err * 2.0 {
                        println!(
                            "         FAIL B: p95 {:.1} > 2× baseline {:.1}",
                            b.p95_origin_err, base.max_p95_origin_err
                        );
                        failed = true;
                    }
                }
                if b.p95_origin_err < map_best_p95 {
                    map_best_p95 = b.p95_origin_err;
                    map_best_preset = b.preset.clone();
                }
            } else {
                println!("         FAIL B: no preset results");
                failed = true;
            }

            // Detail line when a single preset was requested.
            if presets.len() == 1 {
                if let Some(s) = all.first() {
                    println!(
                        "         detail died_at={:?} compared={}/{}",
                        s.died_at,
                        s.compared,
                        s.ghost_frames.saturating_sub(1)
                    );
                }
            }
        }

        if write_baselines && audited > 0 {
            // Store near-observed values; CI applies ~2× headroom on p95.
            let min_on_ramp = (map_on_ramp * 0.5).max(0.08).min(map_on_ramp * 0.9);
            let p95_ceil = if map_best_p95.is_finite() {
                ((map_best_p95 * 1.05) / 100.0).ceil() * 100.0
            } else {
                50_000.0
            };
            new_baselines.push(MapBaseline {
                map: map_name.clone(),
                min_on_ramp_frac: (min_on_ramp * 1000.0).round() / 1000.0,
                max_kill_z_frac: 0.001,
                max_buried_frac: 0.05,
                max_p95_origin_err: p95_ceil,
                best_preset: map_best_preset,
            });
        }
    }

    if write_baselines {
        if new_baselines.is_empty() {
            eprintln!("--write-baselines: nothing to write");
            return ExitCode::from(1);
        }
        let out = ResimBaselines {
            maps: new_baselines,
        };
        let path = baselines_path(&root);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&out).expect("serialize baselines");
        fs::write(&path, json + "\n").expect("write baselines");
        println!("\nwrote {}", path.display());
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
