//! CI gate for KSF summit WR: hard Layer A (ramps loaded) + soft Layer B vs baselines.
//!
//! Skips silently when BSP / ghost / baselines are absent.

use std::path::PathBuf;

use surf_app::replay::Replay;
use surf_app::resim::{
    audit_poses, preset_grid, resim_best_preset, ResimBaselines, SUMMIT_WR_OSXR,
};
use surf_app::zones::{load_zones_file, zones_path_for_map};
use surf_map::LoadedMap;

#[test]
fn summit_wr_pose_and_soft_resim() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map_path = root.join("assets/maps/surf_summit.bsp");
    let ghost_path = root
        .join("assets/replays/external/ksf/surf_summit/imported")
        .join(SUMMIT_WR_OSXR);
    let baselines_path = root.join("assets/fixtures/ksf_resim_baselines.json");

    if !map_path.is_file() || !ghost_path.is_file() || !baselines_path.is_file() {
        eprintln!(
            "summit KSF assets absent; skipping (map={} ghost={} baselines={})",
            map_path.is_file(),
            ghost_path.is_file(),
            baselines_path.is_file()
        );
        return;
    }

    let baselines: ResimBaselines =
        serde_json::from_str(&std::fs::read_to_string(&baselines_path).unwrap())
            .expect("parse baselines");
    let gates = baselines
        .pose_gates("surf_summit")
        .expect("surf_summit baseline");
    let soft = baselines.for_map("surf_summit").unwrap();

    let loaded = LoadedMap::load_path(&map_path).expect("load summit");
    let zones = load_zones_file(&zones_path_for_map(&map_path)).expect("zones");
    let replay = Replay::load(&ghost_path).expect("load WR ghost");

    let pose = audit_poses(&loaded, &replay.frames);
    gates
        .check(&pose)
        .unwrap_or_else(|e| panic!("Layer A failed: {e} (on-ramp={:.1}%)", 100.0 * pose.on_ramp_frac()));

    // Missing PLAYERCLIP / disp would collapse on-ramp well below the floor.
    assert!(
        pose.on_ramp_frac() >= gates.min_on_ramp_frac,
        "on-ramp frac {:.3}",
        pose.on_ramp_frac()
    );

    let presets = preset_grid();
    let (_all, best) = resim_best_preset(&loaded, &replay.frames, &zones, &presets);
    let best = best.expect("resim produced stats");
    assert!(
        !best.catastrophic(),
        "catastrophic survival {:.0}% on preset {}",
        100.0 * best.survival_frac(),
        best.preset
    );
    assert!(
        best.p95_origin_err <= soft.max_p95_origin_err * 2.0,
        "p95 {:.1} > 2× baseline {:.1} (best preset {})",
        best.p95_origin_err,
        soft.max_p95_origin_err,
        best.preset
    );

    eprintln!(
        "summit WR OK: on-ramp={:.1}%  best={} p95={:.0} surv={:.0}%",
        100.0 * pose.on_ramp_frac(),
        best.preset,
        best.p95_origin_err,
        100.0 * best.survival_frac()
    );
}
