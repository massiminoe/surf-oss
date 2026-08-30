//! Corpus spawn sanity: where does each map spawn you, is the hull free there,
//! and how far do you fall? A stuck spawn is unrecoverable in game.
use surf_core::math::Vec3;
use surf_core::movement::Hull;
use surf_core::movement::{MoveVars, PlayerState, UserCmd};
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or("assets/maps".into());
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("maps dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "bsp"))
        .collect();
    paths.sort();
    let hull = Hull::css_stand();
    for p in paths {
        let map = match LoadedMap::load_path(&p) {
            Ok(m) => m,
            Err(e) => {
                println!("{:<22} LOAD FAIL {e}", p.file_stem().unwrap().to_string_lossy());
                continue;
            }
        };
        let s = map.spawn_origin;
        let stuck = surf_core::trace::point_contents_box(&map.world, s, hull.mins, hull.maxs);
        let down = trace_box(
            &map.world,
            s,
            s + Vec3::new(0.0, 0.0, -4096.0),
            hull.mins,
            hull.maxs,
        );
        let drop = if down.fraction >= 1.0 {
            f32::INFINITY
        } else {
            s.z - down.endpos.z
        };
        // The symptom Max reported is "cannot move", so measure that directly:
        // hold W for a second and see how far the player actually travelled.
        let vars = MoveVars::momentum_surf();
        let mut st = PlayerState {
            origin: s,
            viewangles: map.spawn_angles,
            ..PlayerState::default()
        };
        let cmd = UserCmd {
            viewangles: map.spawn_angles,
            forward_move: 1.0,
            side_move: 0.0,
            jump: false,
            duck: false,
        };
        for _ in 0..67 {
            st = surf_core::tick(&map.world, &st, &cmd, &vars);
        }
        let moved = (st.origin - s).length();
        println!(
            "{:<22} spawn ({:8.0},{:8.0},{:8.0}) yaw {:6.1} stuck={} drop={:.0} walk1s={:.0}u",
            map.name, s.x, s.y, s.z, map.spawn_angles.yaw, stuck, drop, moved
        );
    }
}
