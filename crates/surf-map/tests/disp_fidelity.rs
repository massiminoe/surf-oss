//! Displacement collision must be the surface Source builds, not merely a
//! surface through the same vertices. Both tests are on boreas, whose ramps
//! sit against lumpy rock displacements, and both were verified to fail with
//! their A/B lever set (`SURF_OSS_DISP_SINGLE_DIAGONAL=1` /
//! `SURF_OSS_DISP_IGNORE_NOHULL=1`).

use std::path::PathBuf;

use surf_core::math::Vec3;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn boreas() -> LoadedMap {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/surf_boreas.bsp");
    LoadedMap::load_path(path).expect("load surf_boreas")
}

/// Which class of solid a trace hit belongs to.
fn class(map: &LoadedMap, idx: usize) -> &'static str {
    let brush_n = map.world.brushes.len();
    if idx < brush_n {
        "brush"
    } else if idx < brush_n + map.prop_tri_start {
        "disp"
    } else {
        "prop"
    }
}

/// Max (2026-09-02): "a small bit of rock poking through the ramps near the
/// bottom". On ramp 5 (`ramp_c1m` at 8096,10816,9696) that rock was our own:
/// every displacement quad was split on the same diagonal, but Source
/// alternates them on a checkerboard (`CCoreDispInfo::GenerateCollisionSurface`),
/// and on this rock face the wrong diagonal lifted the terrain 44u through
/// the ramp's bottom edge. With Source's split the ramp is the top solid here.
#[test]
fn boreas_ramp5_bottom_edge_is_ramp_not_terrain() {
    let map = boreas();
    let tiny = Vec3::new(0.5, 0.5, 0.5);
    let mut worst: Option<(f32, f32, &str, f32)> = None;
    for (x, y) in [(6448.0, 10456.0), (6472.0, 10456.0), (6496.0, 10456.0), (6448.0, 10480.0)] {
        let top = Vec3::new(x, y, 8700.0);
        let bot = Vec3::new(x, y, 8400.0);
        let tr = trace_box(&map.world, top, bot, -tiny, tiny);
        let hit = tr.hit.as_ref().expect("something solid under the ramp edge");
        let z = top.z + (bot.z - top.z) * tr.fraction;
        let c = class(&map, hit.brush_index);
        if c != "prop" {
            worst = Some((x, y, c, z));
        }
    }
    assert!(
        worst.is_none(),
        "terrain rises above ramp 5's bottom edge: {:?} (expected the ramp prop on top)",
        worst
    );
}

/// The mapper flagged boreas' end-area staircase displacements "No Hull
/// Collision" (Hammer flag 0x4, carried in `ddispinfo_t::minTess`). Source's
/// hull traces ignore such a displacement entirely; so must ours, while it is
/// still drawn.
#[test]
fn boreas_no_hull_collision_displacements_are_not_solid() {
    let map = boreas();
    let flagged = map
        .disp_flags
        .iter()
        .filter(|f| *f & surf_map::disp::DISP_NOHULL_COLL != 0)
        .count();
    assert!(flagged >= 50, "expected boreas' flagged staircase, found {flagged}");
    assert!(
        !map.disp_tri_owner.iter().any(|&d| map.disp_flags[d as usize] & surf_map::disp::DISP_NOHULL_COLL != 0),
        "a No-Hull-Collision displacement contributed collision triangles"
    );
    // disp #211: a stair tread at z=691 spanning x[10328,11232] y[10456,10552].
    let tiny = Vec3::new(0.5, 0.5, 0.5);
    let top = Vec3::new(10780.0, 10500.0, 900.0);
    let bot = Vec3::new(10780.0, 10500.0, 300.0);
    let tr = trace_box(&map.world, top, bot, -tiny, tiny);
    if let Some(hit) = tr.hit.as_ref() {
        let z = top.z + (bot.z - top.z) * tr.fraction;
        assert!(
            !(class(&map, hit.brush_index) == "disp" && (z - 691.0).abs() < 4.0),
            "the flagged stair tread at z=691 is still solid (hit {} at z={z:.1})",
            class(&map, hit.brush_index)
        );
    }
}
