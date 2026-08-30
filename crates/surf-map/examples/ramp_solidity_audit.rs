//! Does the ramp you can *see* actually hold you up?
//!
//! A prop's `.phy` is a simplified shell, so it is normal for collision to be
//! smaller than the render mesh — the parts that differ are usually decorative
//! trim the player never touches, or are covered by a clip brush. What is *not*
//! normal is a large, surfable, upward-facing sheet of drawn ramp with no solid
//! of any kind behind it: the player aims at it and falls straight through.
//!
//! For every drawn triangle belonging to a ramp prop this probes just behind the
//! surface for a solid (brush, displacement or prop — whatever owns it) and
//! reports the unsupported fraction by area, so a few stray trim triangles do
//! not read the same as half a missing ramp.
//!
//!   cargo run -p surf-map --example ramp_solidity_audit --release -- <map.bsp> [--all]

use surf_core::math::Vec3;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

/// Only sheets you could plausibly ride: steeper than this is a wall.
const MIN_NZ: f32 = 0.2;
/// Half-extent of the probe hull. Small enough to sit on one triangle, big
/// enough not to slip between two neighbouring collision triangles.
const PROBE: f32 = 2.0;
/// How far to probe either side of the drawn surface along its normal.
const REACH: f32 = 12.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: ramp_solidity_audit <map.bsp> [--all]");
        std::process::exit(2);
    }
    let all = args.iter().any(|a| a == "--all");
    let map = LoadedMap::load_path(&args[0]).expect("load map");
    println!("== {} ==", map.name);

    let mins = Vec3::new(-PROBE, -PROBE, -PROBE);
    let maxs = Vec3::new(PROBE, PROBE, PROBE);

    let mut rows: Vec<(String, Vec3, f64, f64, bool, bool)> = Vec::new();
    for prop in &map.props {
        if prop.skybox {
            continue;
        }
        if !all && !prop.model.contains("ramp") {
            continue;
        }
        let mut area_all = 0.0f64;
        let mut area_unsupported = 0.0f64;
        // Centre of the largest unsupported patch, for eyeballing it later.
        let mut worst = Vec3::ZERO;
        let mut worst_area = 0.0f64;
        for t in prop.render_tris.clone() {
            let Some(tri) = map.mesh.tris.get(t) else {
                continue;
            };
            let e1 = tri.b - tri.a;
            let e2 = tri.c - tri.a;
            let cross = e1.cross(e2);
            let len = cross.length();
            if len <= 1e-6 {
                continue;
            }
            let n = cross * (1.0 / len);
            let area = (len * 0.5) as f64;
            if n.z.abs() < MIN_NZ {
                continue;
            }
            // Face the probe along whichever side is "up" for this sheet.
            let n = if n.z < 0.0 { n * -1.0 } else { n };
            let c = (tri.a + tri.b + tri.c) * (1.0 / 3.0);
            area_all += area;
            let start = c + n * REACH;
            let end = c - n * REACH;
            let hit = trace_box(&map.world, start, end, mins, maxs);
            if hit.fraction >= 1.0 && !hit.startsolid {
                area_unsupported += area;
                if area > worst_area {
                    worst_area = area;
                    worst = c;
                }
            }
        }
        if area_all <= 0.0 {
            continue;
        }
        let frac = area_unsupported / area_all;
        rows.push((
            prop.model.clone(),
            worst,
            frac,
            area_unsupported,
            prop.solid,
            prop.from_phy,
        ));
    }

    rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    println!(
        "{:<44} {:>7} {:>11}  {:<6} {:<5} where",
        "model", "unsup%", "unsup area", "solid", "phy"
    );
    let mut bad = 0;
    for (model, at, frac, area, solid, phy) in &rows {
        if *frac < 0.02 {
            continue;
        }
        bad += 1;
        println!(
            "{:<44} {:>6.1}% {:>11.0}  {:<6} {:<5} ({:.0},{:.0},{:.0})",
            model,
            frac * 100.0,
            area,
            solid,
            phy,
            at.x,
            at.y,
            at.z
        );
    }
    println!(
        "\n{bad} of {} ramp-prop instances have >2% of their surfable drawn area unsupported",
        rows.len()
    );
}
