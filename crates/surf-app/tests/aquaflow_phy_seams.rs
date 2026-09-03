//! aquaflow's ramps are `$concave` `.phy` solids — curved half-pipes stored as
//! a dozen convex pieces that touch face to face. The faces two pieces share
//! are interior to the union and nothing in Source collides with them, but we
//! traced every `.phy` triangle as its own thin prism (and flipped each one to
//! face +z), so the seam caps stood across the ramp as walls facing the
//! player. On the KSF WR line ramp 1 hit one at tick 51 (878 → 290 u/s) and
//! the run had 93 prop snags in total; Max saw "weird artifacts being
//! registered as ramps". Verified to fail with `MX_SURF_PHY_RAW=1`
//! (the pre-fix decode): "tick 51 kept 33% of its speed".

use std::path::PathBuf;

use surf_app::replay::Replay;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{MoveVars, PlayerState};
use surf_map::LoadedMap;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn aquaflow_first_ramp_does_not_snag_on_phy_seam_faces() {
    let map_path = root().join("assets/maps/surf_aquaflow.bsp");
    let replay_path = root().join(
        "assets/replays/external/ksf/surf_aquaflow/imported/replay_css_4078_0_748490_1762805973.osxr",
    );
    if !map_path.is_file() || !replay_path.is_file() {
        eprintln!("surf_aquaflow assets absent; skipping");
        return;
    }
    let map = LoadedMap::load_path(&map_path).expect("map");
    let frames = Replay::load(&replay_path).expect("replay").frames;
    let vars = MoveVars::momentum_surf();

    // Ramp 1 of the WR is ticks 18..157. Step our sim one tick from each
    // recorded pose and compare the speed it keeps with the recording's.
    let mut worst: Option<(usize, f32)> = None;
    for tick in 18..157 {
        let f = frames[tick];
        let before = f.velocity.length_2d();
        if before < 300.0 {
            continue;
        }
        let p = PlayerState {
            origin: f.origin,
            velocity: f.velocity,
            viewangles: Angle::new(0.0, f.yaw, 0.0),
            grounded: f.grounded,
            ..Default::default()
        };
        let after = surf_core::tick(&map.world, &p, &f.to_usercmd(), &vars);
        let kept = after.velocity.length_2d() / before;
        let recorded = frames[tick + 1].velocity.length_2d() / before;
        // Allow ordinary ramp-contact loss; a seam wall costs >50% in one tick.
        if kept < recorded - 0.25 && worst.map_or(true, |(_, k)| kept < k) {
            worst = Some((tick, kept));
        }
    }
    assert!(
        worst.is_none(),
        "tick {} kept {:.0}% of its speed on ramp 1 (CS:S kept it)",
        worst.unwrap().0,
        worst.unwrap().1 * 100.0
    );
    let _ = Vec3::ZERO;
}
