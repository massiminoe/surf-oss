//! Layer B regression: mesh triangle edge planes must not "1px wall" stop a run.
//!
//! Recreates the nyx ghost@979 failure (2629→378 u/s on a CollisionTri edge hit)
//! and checks cyberwave windows for the same class of snag.

use std::path::PathBuf;

use surf_app::replay::Replay;
use surf_app::resim::{find_speed_snags, resim_window_trace};
use surf_core::movement::MoveVars;
use surf_map::LoadedMap;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn nyx_ghost_tick_979_does_not_speed_snag() {
    let map_path = root().join("assets/maps/surf_nyx.bsp");
    let ghost_path = root().join(
        "assets/replays/external/ksf/surf_nyx/imported/replay_css_3450_0_540902_1724927513.osxr",
    );
    if !map_path.is_file() || !ghost_path.is_file() {
        eprintln!("nyx assets absent; skipping");
        return;
    }

    let map = LoadedMap::load_path(&map_path).expect("nyx");
    let frames = Replay::load(&ghost_path)
        .expect("ghost")
        .frames_for_resim();
    let vars = MoveVars::momentum_surf();

    // Window covering the known seam snag (was tick 980 in the 958.. window).
    let (_stats, rows) = resim_window_trace(&map, &frames, 958, 66, &vars, "momentum_surf");
    let snags = find_speed_snags(&rows, 500.0, 0.4);
    assert!(
        snags.is_empty(),
        "mesh edge 1px wall snag(s) on nyx: {:?}",
        snags
    );

    // Direct single-tick: ghost pose 979 must keep speed (was 2629→378).
    let f = &frames[979];
    let mut p = surf_core::movement::PlayerState {
        origin: f.origin,
        velocity: f.velocity,
        viewangles: surf_core::math::Angle::new(0.0, f.yaw, 0.0),
        grounded: f.grounded,
        ..Default::default()
    };
    let before = p.velocity.length_2d();
    let path: Vec<_> = frames[..=979].iter().map(|f| f.origin).collect();
    let mut fields = map.field_state_at(&path);
    map.arm_fields_from_recording(&mut p, &mut fields);
    p = surf_core::tick(&map.world, &p, &f.to_usercmd(), &vars);
    let after = p.velocity.length_2d();
    assert!(
        after > before * 0.85,
        "nyx ghost@979 seam snag: {before:.0}→{after:.0}"
    );
}

#[test]
fn cyberwave_windows_have_no_speed_snags() {
    let map_path = root().join("assets/maps/surf_cyberwave.bsp");
    let ghost_path = root().join(
        "assets/replays/external/ksf/surf_cyberwave/imported/replay_css_3203_0_712551_1755980226.osxr",
    );
    if !map_path.is_file() || !ghost_path.is_file() {
        eprintln!("cyberwave assets absent; skipping");
        return;
    }

    let map = LoadedMap::load_path(&map_path).expect("cyberwave");
    let frames = Replay::load(&ghost_path)
        .expect("ghost")
        .frames_for_resim();
    let vars = MoveVars::momentum_surf();

    // Former worst window (start=862) froze / snagged on prop-mesh edges.
    let (_stats, rows) = resim_window_trace(&map, &frames, 862, 66, &vars, "momentum_surf");
    let snags = find_speed_snags(&rows, 500.0, 0.4);
    assert!(
        snags.is_empty(),
        "cyberwave mesh edge snag(s): {:?}",
        snags
    );
}
