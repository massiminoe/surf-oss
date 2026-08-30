//! Dissect one tick of a KSF ghost: every solid the sweep touches, and why the
//! move stopped. Built for the "3606 -> 15 u/s" class of dead stop.
//!
//!   cargo run -p surf-app --example snag_probe --release -- surf_boreas 1682

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState};
use surf_core::trace::{trace_box, trace_planes_pub};
use surf_map::LoadedMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_boreas".into());
    let tick: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1682);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map = LoadedMap::load_path(root.join(format!("assets/maps/{map_name}.bsp"))).expect("map");
    let dir = ksf_imported_dir(&map_name);
    let mut ghosts: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("ghost dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("osxr"))
        .collect();
    ghosts.sort();
    let replay = Replay::load(&ghosts[0]).expect("ghost");
    let frames = replay.frames_for_resim();
    let f = &frames[tick];

    let vars = MoveVars::momentum_surf();
    let hull = Hull::css_stand();
    let brush_n = map.world.brushes.len();
    let prop_start = brush_n + map.prop_tri_start;

    let start = f.origin;
    let end = start + f.velocity * vars.tick_interval;
    println!(
        "{map_name} tick {tick}: origin=({:.1},{:.1},{:.1}) vel=({:.0},{:.0},{:.0}) |v2d|={:.0} grounded={}",
        start.x, start.y, start.z, f.velocity.x, f.velocity.y, f.velocity.z,
        f.velocity.length_2d(), f.grounded
    );
    println!("  move this tick = {:.1} units", (end - start).length());

    let tr = trace_box(&map.world, start, end, hull.mins, hull.maxs);
    println!(
        "  full trace: fraction={:.6} startsolid={} allsolid={} hit={:?}",
        tr.fraction,
        tr.startsolid,
        tr.allsolid,
        tr.hit
            .as_ref()
            .map(|h| (h.brush_index, h.plane_index, h.normal))
    );

    // Which individual solids claim this sweep, and what each says on its own.
    println!("  -- per-solid results along this sweep --");
    for (i, tri) in map.world.tris.iter().enumerate() {
        let solid = brush_n + i;
        let lo = Vec3::new(
            start.x.min(end.x) + hull.mins.x,
            start.y.min(end.y) + hull.mins.y,
            start.z.min(end.z) + hull.mins.z,
        );
        let hi = Vec3::new(
            start.x.max(end.x) + hull.maxs.x,
            start.y.max(end.y) + hull.maxs.y,
            start.z.max(end.z) + hull.maxs.z,
        );
        if hi.x < tri.bounds.mins.x
            || lo.x > tri.bounds.maxs.x
            || hi.y < tri.bounds.mins.y
            || lo.y > tri.bounds.maxs.y
            || hi.z < tri.bounds.mins.z
            || lo.z > tri.bounds.maxs.z
        {
            continue;
        }
        let r = trace_planes_pub(&tri.planes, solid, start, end, hull.mins, hull.maxs);
        if r.fraction >= 1.0 && !r.startsolid {
            continue;
        }
        let kind = if i < map.prop_tri_start {
            "disp"
        } else {
            "PROP"
        };
        println!(
            "    {kind} tri {i}: frac={:.4} startsolid={} allsolid={} plane={:?} n={:?}",
            r.fraction,
            r.startsolid,
            r.allsolid,
            r.hit.as_ref().map(|h| h.plane_index),
            r.hit.as_ref().map(|h| h.normal),
        );
    }
    let _ = prop_start;

    let mut p = PlayerState {
        origin: f.origin,
        velocity: f.velocity,
        viewangles: Angle::new(0.0, f.yaw, 0.0),
        grounded: f.grounded,
        ..Default::default()
    };
    p.basevelocity = map.touch_push(p.origin);
    p.gravity_scale = map.touch_gravity(p.origin);
    let after = surf_core::tick(&map.world, &p, &f.to_usercmd(), &vars);
    println!(
        "  tick result: |v2d| {:.0} -> {:.0}, origin moved {:.2}u",
        f.velocity.length_2d(),
        after.velocity.length_2d(),
        (after.origin - f.origin).length()
    );
}
