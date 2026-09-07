//! Step the real `tick()` from a hand-given pose and print what happens.
//!   cargo run -p surf-app --example tick_probe --release -- surf_boreas x,y,z vx,vy,vz [ticks]
use std::path::PathBuf;
use surf_core::math::Vec3;
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn v3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    Vec3::new(v[0], v[1], v[2])
}

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap();
    let origin = v3(&args.next().unwrap());
    let velocity = v3(&args.next().unwrap());
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map = LoadedMap::load_path(root.join(format!("assets/maps/{map_name}.bsp"))).expect("map");
    let vars = MoveVars::momentum_surf();
    let hull = Hull::css_stand();
    let mut p = PlayerState { origin, velocity, grounded: false, ..Default::default() };
    let cmd = UserCmd::default();
    for i in 0..n {
        let tr = trace_box(&map.world, p.origin, p.origin + p.velocity * vars.tick_interval, hull.mins, hull.maxs);
        println!("tick {i}: origin=({:.3},{:.3},{:.3}) vel=({:.2},{:.2},{:.2}) grounded={} | sweep frac={:.4} ss={} as={} hit={:?}",
            p.origin.x, p.origin.y, p.origin.z, p.velocity.x, p.velocity.y, p.velocity.z, p.grounded,
            tr.fraction, tr.startsolid, tr.allsolid, tr.hit.map(|h| (h.brush_index, h.plane_index, h.normal)));
        let next = surf_core::tick(&map.world, &p, &cmd, &vars);
        println!("        moved {:.3}u", (next.origin - p.origin).length());
        p = next;
    }
}
