//! "Ramp bugs" on cyberwave (Max, 2026-09-06): at the edges of ramps — boarding
//! low at the leading end, exiting off the top — the player stopped dead while
//! *keeping* their speed, sometimes to be spat out a moment later. Stage-2 ramp
//! 2 was the reliable spot.
//!
//! Two things conspired. The thin-prism collision triangle carried no bevel
//! planes, so its expanded plane set was fatter than the real hull-swept volume
//! and a hull sliding over a triangle's boundary edge read as *entering* it;
//! the workaround (discard any edge entry outright) then let a sweep that also
//! crossed the face plane pass straight through the surface. Once inside, the
//! trace reported fraction 0 with no plane, and `try_player_move` had nothing
//! to push against: frozen, velocity intact. Now the prism is bevelled (the
//! same planes vbsp adds to brushes and Source's own displacement sweep tests),
//! "inside" means straddling the face within its outline, and a stuck hull is
//! nudged out along a known normal and re-swept (Momentum's `sv_ramp_fix`).
//!
//! This perturbs the KSF WR line around that ramp — sideways toward the edges,
//! up and down, with sideways velocity — and runs the real `tick()`.
//! Verified to fail on the old code: 636 of 8505 trials froze on this ramp
//! (9209 across the map's 20 ramps).

use std::path::PathBuf;

use surf_app::replay::Replay;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};
use surf_core::trace::point_contents_box;
use surf_map::LoadedMap;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn cyberwave_stage2_ramp2_edges_never_freeze_the_hull_with_speed() {
    let map_path = root().join("assets/maps/surf_cyberwave.bsp");
    let replay_path = root().join(
        "assets/replays/external/ksf/surf_cyberwave/imported/replay_css_3203_0_712551_1755980226.osxr",
    );
    if !map_path.is_file() || !replay_path.is_file() {
        eprintln!("surf_cyberwave assets absent; skipping");
        return;
    }
    let map = LoadedMap::load_path(&map_path).expect("map");
    let frames = Replay::load(&replay_path).expect("replay").frames_for_resim();
    let vars = MoveVars::momentum_surf();
    let hull = Hull::css_stand();

    // Stage-2 ramp 2 on the WR line (ramp_tour: ticks 2168..2277).
    let mut trials = 0usize;
    let mut frozen: Vec<(usize, f32, f32, f32, Vec3, Vec3)> = Vec::new();
    for t in (2168..=2277).step_by(3) {
        let f = &frames[t];
        let fwd = Vec3::new(f.velocity.x, f.velocity.y, 0.0);
        if fwd.length_2d() < 100.0 {
            continue;
        }
        let fwd = fwd.normalize();
        let side = Vec3::new(-fwd.y, fwd.x, 0.0);
        for lat in [-128.0, -96.0, -64.0, -32.0, -16.0, 0.0, 16.0, 32.0, 64.0, 96.0, 128.0] {
            for dz in [-32.0, -16.0, -8.0, 0.0, 8.0, 24.0] {
                for vside in [-600.0, -300.0, 0.0, 300.0, 600.0] {
                    let seed = f.origin + side * lat + Vec3::new(0.0, 0.0, dz);
                    if point_contents_box(&map.world, seed, hull.mins, hull.maxs) {
                        continue;
                    }
                    trials += 1;
                    let angles = Angle::new(f.pitch, f.yaw, 0.0);
                    let mut p = PlayerState {
                        origin: seed,
                        velocity: f.velocity + side * vside,
                        viewangles: angles,
                        grounded: false,
                        ..Default::default()
                    };
                    let cmd = UserCmd {
                        viewangles: angles,
                        ..UserCmd::default()
                    };
                    let mut run = 0;
                    for _ in 0..40 {
                        let next = surf_core::tick(&map.world, &p, &cmd, &vars);
                        let moved = (next.origin - p.origin).length();
                        let want = next.velocity.length() * vars.tick_interval;
                        // Airborne, fast, asked to move 5u+, moved under 0.5u:
                        // three ticks of that is the freeze, not a wall touch.
                        if !next.grounded && next.velocity.length() > 300.0 && want > 5.0 && moved < 0.5 {
                            run += 1;
                            if run == 3 {
                                frozen.push((t, lat, dz, vside, p.origin, p.velocity));
                                break;
                            }
                        } else {
                            run = 0;
                        }
                        p = next;
                    }
                }
            }
        }
    }
    assert!(trials > 5000, "scan too small: {trials}");
    assert!(
        frozen.is_empty(),
        "{} of {trials} trials froze with speed; first: seed tick {} lat {} dz {} vside {} at {:?} vel {:?}",
        frozen.len(),
        frozen[0].0,
        frozen[0].1,
        frozen[0].2,
        frozen[0].3,
        frozen[0].4,
        frozen[0].5
    );
}
