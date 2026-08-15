//! Two-layer KSF replay audit: pose probes (Layer A) + windowed input resim (Layer B).
//!
//! Layer B primary signal is short-horizon on-ramp windows (H=30/66/132).
//! Open-loop full-run is printed as a diagnostic only.
//!
//! ```text
//! cargo run -p surf-app --example ksf_resim_audit --release
//! cargo run -p surf-app --example ksf_resim_audit --release -- --maps surf_summit --preset ksf_css_66t
//! cargo run -p surf-app --example ksf_resim_audit --release -- --write-baselines
//! cargo run -p surf-app --example ksf_resim_audit --release -- --maps surf_summit --dump-worst
//! cargo run -p surf-app --example ksf_resim_audit --release -- --maps surf_summit --ghost 712551 --dump-window 2162
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use surf_app::replay::{ksf_imported_dir, Replay, ReplayFrame};
use surf_app::resim::{
    audit_poses, preset_by_name, preset_grid, resim_best_preset, resim_window_trace,
    resim_windows_best_preset, vars_for_map, MapBaseline, NamedPreset, PoseGates, ResimBaselines,
    WindowTickRow, SOFT_AUDIT_MAPS,
};
use surf_app::zones::{load_zones_file, zones_path_for_map, MapZones};
use surf_map::LoadedMap;

struct Args {
    maps: Vec<String>,
    preset: Option<String>,
    write_baselines: bool,
    dump_worst: bool,
    dump_windows: Vec<usize>,
    dump_horizon: usize,
    ghost_filter: Option<String>,
    skip_open_loop: bool,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn map_bsp(root: &Path, map: &str) -> PathBuf {
    root.join("assets/maps").join(format!("{map}.bsp"))
}

fn parse_args(args: &[String]) -> Args {
    let mut maps: Vec<String> = Vec::new();
    let mut preset: Option<String> = None;
    let mut write_baselines = false;
    let mut dump_worst = false;
    let mut dump_windows: Vec<usize> = Vec::new();
    let mut dump_horizon = 66usize;
    let mut ghost_filter: Option<String> = None;
    let mut skip_open_loop = false;
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
            "--dump-worst" => {
                dump_worst = true;
                skip_open_loop = true;
            }
            "--dump-window" => {
                i += 1;
                if i < args.len() {
                    if let Ok(n) = args[i].parse::<usize>() {
                        dump_windows.push(n);
                        skip_open_loop = true;
                    } else {
                        eprintln!("warning: bad --dump-window {}", args[i]);
                    }
                }
            }
            "--horizon" => {
                i += 1;
                if i < args.len() {
                    if let Ok(n) = args[i].parse::<usize>() {
                        dump_horizon = n.max(1);
                    }
                }
            }
            "--ghost" => {
                i += 1;
                if i < args.len() {
                    ghost_filter = Some(args[i].clone());
                }
            }
            "--no-open-loop" => skip_open_loop = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ksf_resim_audit [--maps m1,m2] [--preset all|name] [--write-baselines]\n\
                     \n  --dump-worst          per-tick dump of worst@66 window (best preset)\n\
                     \n  --dump-window N       per-tick dump seeded at tick N\n\
                     \n  --horizon N           dump length (default 66)\n\
                     \n  --ghost SUBSTR        only ghosts whose filename contains SUBSTR\n\
                     \n  --no-open-loop        skip open-loop diagnostic"
                );
                std::process::exit(0);
            }
            other => eprintln!("warning: ignoring arg {other}"),
        }
        i += 1;
    }
    if maps.is_empty() {
        maps = SOFT_AUDIT_MAPS.iter().map(|s| (*s).to_string()).collect();
    }
    Args {
        maps,
        preset,
        write_baselines,
        dump_worst,
        dump_windows,
        dump_horizon,
        ghost_filter,
        skip_open_loop,
    }
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
    PoseGates {
        min_on_ramp_frac: 0.10,
        max_kill_z_frac: 0.001,
        max_buried_frac: 0.05,
    }
}

fn print_window_dump(label: &str, stats: &surf_app::resim::WindowStats, rows: &[WindowTickRow]) {
    println!(
        "         --- dump {label} preset={} start={} H={} maxErr={:.1} final={:.1} died={} teles={} ---",
        stats.preset,
        stats.start_tick,
        stats.horizon,
        stats.max_origin_err,
        stats.final_origin_err,
        stats.died,
        stats.teleports
    );
    println!(
        "         tick  oErr   vErr  simSpd gstSpd  gnd(s/g) ramp(s/g)  dOxyz                    fwd  side   yaw  | simXYZ / gstXYZ"
    );
    let mut prev_oerr = 0.0_f32;
    for r in rows {
        let d = r.sim_origin - r.ghost_origin;
        let jump = r.origin_err > prev_oerr + 10.0 || r.vel_err > 40.0;
        println!(
            "         {:>4} {:>6.1} {:>6.1} {:>6.0} {:>6.0}   {}/{}     {}/{}   {:>7.1},{:>7.1},{:>7.1}  {:>4.0} {:>4.0} {:>6.1}{}{}",
            r.tick,
            r.origin_err,
            r.vel_err,
            r.sim_speed,
            r.ghost_speed,
            if r.sim_grounded { 'G' } else { '.' },
            if r.ghost_grounded { 'G' } else { '.' },
            if r.sim_on_ramp { 'R' } else { '.' },
            if r.ghost_on_ramp { 'R' } else { '.' },
            d.x,
            d.y,
            d.z,
            r.fwd,
            r.side,
            r.yaw,
            if r.teleported { " TELE" } else { "" },
            if jump {
                let hit = r.sweep_hit.as_ref().map_or_else(
                    || format!("sweep frac={:.3} (clear)", r.sweep_fraction),
                    |h| {
                        format!(
                            "sweep frac={:.3} n=({:.3},{:.3},{:.3}) brush={}",
                            r.sweep_fraction, h.normal.x, h.normal.y, h.normal.z, h.brush_index
                        )
                    },
                );
                format!(
                    "  | sim({:.0},{:.0},{:.0}) gst({:.0},{:.0},{:.0}) vS({:.0},{:.0},{:.0}) vG({:.0},{:.0},{:.0}) {hit}",
                    r.sim_origin.x,
                    r.sim_origin.y,
                    r.sim_origin.z,
                    r.ghost_origin.x,
                    r.ghost_origin.y,
                    r.ghost_origin.z,
                    r.sim_vel.x,
                    r.sim_vel.y,
                    r.sim_vel.z,
                    r.ghost_vel.x,
                    r.ghost_vel.y,
                    r.ghost_vel.z,
                )
            } else {
                String::new()
            },
        );
        prev_oerr = r.origin_err;
    }
}

fn dump_starts_for_ghost(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    zones: &MapZones,
    preset: &NamedPreset,
    starts: &[usize],
    horizon: usize,
) {
    let vars = vars_for_map(&preset.vars, zones);
    for &start in starts {
        let (stats, rows) =
            resim_window_trace(map, frames, start, horizon, &vars, preset.name);
        print_window_dump(&format!("window@{start}"), &stats, &rows);
    }
}

fn main() -> ExitCode {
    let raw: Vec<String> = env::args().skip(1).collect();
    let args = parse_args(&raw);
    let root = repo_root();
    let baselines = load_baselines(&root);

    let presets: Vec<NamedPreset> = if let Some(name) = &args.preset {
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
        args.maps.join(","),
        presets.iter().map(|p| p.name).collect::<Vec<_>>().join(",")
    );

    let mut failed = false;
    let mut new_baselines: Vec<MapBaseline> = Vec::new();

    for map_name in &args.maps {
        let bsp = map_bsp(&root, map_name);
        if !bsp.is_file() {
            eprintln!("SKIP {map_name}: missing {}", bsp.display());
            continue;
        }
        let mut ghosts = list_osxr(&root, map_name);
        if let Some(ref filt) = args.ghost_filter {
            ghosts.retain(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains(filt.as_str()))
            });
        }
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

        let mut map_best_med66 = f32::MAX;
        let mut map_best_preset = String::new();
        let mut map_open_p95 = f32::MAX;
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

            // KSF legacy imports store buttons lagged vs pose; align for re-sim.
            let frames = replay.frames_for_resim();
            if replay.buttons_lag_pose() {
                println!("         (aligned KSF buttons→pose for Layer B)");
            }

            let t1 = Instant::now();
            let (segments, starts, win_all, win_best) =
                resim_windows_best_preset(&loaded, &frames, &zones, &presets);
            let dt_win = t1.elapsed().as_secs_f32();

            if let Some(ref b) = win_best {
                let h30 = b.at_horizon(30);
                let h66 = b.at_horizon(66);
                let h132 = b.at_horizon(132);
                println!(
                    "         B windows segs={} starts={}  best={}  medMax@30={:.1} @66={:.1} @132={:.1}  p95Max@66={:.1}  ({:.1}s, {} presets)",
                    segments.len(),
                    starts.len(),
                    b.preset,
                    h30.map(|a| a.median_max_origin_err).unwrap_or(0.0),
                    h66.map(|a| a.median_max_origin_err).unwrap_or(0.0),
                    h132.map(|a| a.median_max_origin_err).unwrap_or(0.0),
                    h66.map(|a| a.p95_max_origin_err).unwrap_or(0.0),
                    dt_win,
                    win_all.len()
                );
                if let Some(h66) = h66 {
                    if let Some(w) = h66.worst.first() {
                        println!(
                            "         B worst@66 start={} maxErr={:.1} final={:.1} died={} teles={}",
                            w.start_tick,
                            w.max_origin_err,
                            w.final_origin_err,
                            w.died,
                            w.teleports
                        );
                    }
                    if let Some(base) = baselines.as_ref().and_then(|x| x.for_map(map_name)) {
                        if h66.median_max_origin_err > base.max_median_origin_err_at_66 * 2.0 {
                            println!(
                                "         FAIL B: median@66 {:.1} > 2× baseline {:.1}",
                                h66.median_max_origin_err, base.max_median_origin_err_at_66
                            );
                            failed = true;
                        }
                    }
                    if h66.median_max_origin_err < map_best_med66 {
                        map_best_med66 = h66.median_max_origin_err;
                        map_best_preset = b.preset.clone();
                    }

                    // Per-tick dumps.
                    let dump_preset = presets
                        .iter()
                        .find(|p| p.name == b.preset)
                        .or_else(|| presets.first());
                    if let Some(dp) = dump_preset {
                        let mut dump_starts = args.dump_windows.clone();
                        if args.dump_worst {
                            if let Some(w) = h66.worst.first() {
                                if !dump_starts.contains(&w.start_tick) {
                                    dump_starts.push(w.start_tick);
                                }
                            }
                        }
                        if !dump_starts.is_empty() {
                            dump_starts_for_ghost(
                                &loaded,
                                &frames,
                                &zones,
                                dp,
                                &dump_starts,
                                args.dump_horizon,
                            );
                        }
                    }
                }
            } else if starts.is_empty() {
                println!(
                    "         FAIL B: no on-ramp windows (segs={} min_len)",
                    segments.len()
                );
                failed = true;
            } else {
                println!("         FAIL B: no window preset results");
                failed = true;
            }

            if !args.skip_open_loop {
                let t2 = Instant::now();
                let (_open_all, open_best) =
                    resim_best_preset(&loaded, &frames, &zones, &presets);
                let dt_open = t2.elapsed().as_secs_f32();
                if let Some(ref o) = open_best {
                    println!(
                        "         open-loop (diag) best={}  p95={:.1}  max={:.1}  surv={:.0}%  end={}  ({:.1}s)",
                        o.preset,
                        o.p95_origin_err,
                        o.max_origin_err,
                        100.0 * o.survival_frac(),
                        o.reached_end,
                        dt_open
                    );
                    if o.p95_origin_err < map_open_p95 {
                        map_open_p95 = o.p95_origin_err;
                    }
                }
            }

            if presets.len() == 1 && !args.dump_worst && args.dump_windows.is_empty() {
                if let Some(r) = win_all.first() {
                    if let Some(h66) = r.at_horizon(66) {
                        println!(
                            "         detail windows@66 n={} med={:.1} p95={:.1}",
                            h66.n_windows, h66.median_max_origin_err, h66.p95_max_origin_err
                        );
                        for (i, w) in h66.worst.iter().take(3).enumerate() {
                            println!(
                                "           worst[{i}] start={} max={:.1} final={:.1} died={}",
                                w.start_tick, w.max_origin_err, w.final_origin_err, w.died
                            );
                        }
                    }
                }
            }
        }

        if args.write_baselines && audited > 0 {
            let min_on_ramp = (map_on_ramp * 0.5).max(0.08).min(map_on_ramp * 0.9);
            let med66_ceil = if map_best_med66.is_finite() {
                // Floor at 10 so near-perfect medians still leave soft CI headroom.
                (((map_best_med66 * 1.05) / 10.0).ceil() * 10.0).max(10.0)
            } else {
                50_000.0
            };
            // Open-loop may be skipped (--no-open-loop); keep 0 as "unset".
            let open_ceil = if map_open_p95.is_finite() && map_open_p95 < f32::MAX / 2.0 {
                ((map_open_p95 * 1.05) / 100.0).ceil() * 100.0
            } else {
                0.0
            };
            new_baselines.push(MapBaseline {
                map: map_name.clone(),
                min_on_ramp_frac: (min_on_ramp * 1000.0).round() / 1000.0,
                max_kill_z_frac: 0.001,
                max_buried_frac: 0.05,
                max_median_origin_err_at_66: med66_ceil,
                max_p95_origin_err: open_ceil,
                best_preset: map_best_preset,
            });
        }
    }

    if args.write_baselines {
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
