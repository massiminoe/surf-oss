//! Scan a ramp prop's ENTIRE footprint (not just the WR corridor) and report
//! two different things that both read as "a rock pokes through the ramp":
//!   * solid geometry that rises above the ramp's own surface (physics — the
//!     player will hit it), attributed to brush / displacement / prop model;
//!   * drawn, non-solid geometry that rises above the ramp surface (visual
//!     only — the player passes through it, but it hides the line).
//!
//!   cargo run -p surf-app --example ramp_footprint --release -- surf_boreas 12032,8128,11040

use std::collections::HashMap;
use std::path::PathBuf;

use surf_core::math::Vec3;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

const STEP: f32 = 24.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_boreas".into());
    let at: Vec<f32> = args
        .next()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map = LoadedMap::load_path(root.join(format!("assets/maps/{map_name}.bsp"))).expect("map");

    if at.is_empty() {
        // --all: one summary line per ramp prop.
        let mut ramps: Vec<&surf_map::PropInstance> = map
            .props
            .iter()
            .filter(|p| p.model.contains("ramp") && !p.render_tris.is_empty() && !p.skybox)
            .collect();
        ramps.sort_by(|a, b| b.origin.z.total_cmp(&a.origin.z));
        for r in ramps {
            scan(&map, r, false);
        }
        return;
    }
    let ramp = map
        .props
        .iter()
        .filter(|p| p.model.contains("ramp") && !p.render_tris.is_empty())
        .min_by(|a, b| {
            let d = |p: &surf_map::PropInstance| {
                (p.origin - Vec3::new(at[0], at[1], at[2])).length_squared()
            };
            d(a).total_cmp(&d(b))
        })
        .expect("ramp prop");
    scan(&map, ramp, true);
}

fn scan(map: &LoadedMap, ramp: &surf_map::PropInstance, verbose: bool) {
    let rb = ramp.render_bounds;
    let map_name = &map.name;
    if verbose { println!(
        "{map_name}: {} at ({:.0},{:.0},{:.0}) solid={} drawn x[{:.0},{:.0}] y[{:.0},{:.0}] z[{:.0},{:.0}]",
        ramp.model, ramp.origin.x, ramp.origin.y, ramp.origin.z, ramp.solid,
        rb.mins.x, rb.maxs.x, rb.mins.y, rb.maxs.y, rb.mins.z, rb.maxs.z
    ); }

    let brush_n = map.world.brushes.len();
    let prop_start = brush_n + map.prop_tri_start;
    let owner = |idx: usize| -> String {
        if idx < brush_n {
            format!("brush#{idx}")
        } else if idx < prop_start {
            let d = map.disp_tri_owner[idx - brush_n] as usize;
            format!("disp#{d} flags={:#x}", map.disp_flags[d])
        } else {
            let t = idx - prop_start;
            map.props
                .iter()
                .find(|p| p.tris.contains(&t))
                .map(|p| format!("prop {} @({:.0},{:.0},{:.0})", p.model, p.origin.x, p.origin.y, p.origin.z))
                .unwrap_or_else(|| format!("prop tri {t}"))
        }
    };

    // Drawn props overlapping the ramp footprint (excluding the ramp itself).
    let others: Vec<&surf_map::PropInstance> = map
        .props
        .iter()
        .filter(|p| !std::ptr::eq(*p, ramp) && !p.render_tris.is_empty() && !p.skybox)
        .filter(|p| {
            let b = p.render_bounds;
            b.maxs.x >= rb.mins.x && b.mins.x <= rb.maxs.x && b.maxs.y >= rb.mins.y && b.mins.y <= rb.maxs.y
                && b.maxs.z >= rb.mins.z
        })
        .collect();

    // model -> (cells above ramp, max height above ramp, sample point, min frac-from-top)
    let mut solid_hits: HashMap<String, (usize, f32, Vec3, f32)> = HashMap::new();
    let mut drawn_hits: HashMap<String, (usize, f32, Vec3, f32)> = HashMap::new();
    let mut ramp_cells = 0usize;
    let mut solid_cells = 0usize;
    let mut ramp_missing_solid = 0usize;
    // Small protrusions: (height above ramp, cell, owner, ramp normal z)
    let mut small: Vec<(f32, Vec3, String, f32)> = Vec::new();

    let mins = Vec3::new(-0.5, -0.5, -0.5);
    let maxs = Vec3::new(0.5, 0.5, 0.5);
    let top_z = rb.maxs.z + 64.0;
    let bot_z = rb.mins.z - 64.0;

    let mut x = rb.mins.x;
    while x <= rb.maxs.x {
        let mut y = rb.mins.y;
        while y <= rb.maxs.y {
            let o = Vec3::new(x, y, top_z);
            // Ramp's own drawn surface (highest hit).
            let ramp_hit = map.mesh.tris[ramp.render_tris.clone()]
                .iter()
                .filter_map(|t| ray_down_z(o, t.a, t.b, t.c).map(|z| (z, (t.b - t.a).cross(t.c - t.a))))
                .fold(None::<(f32, Vec3)>, |acc, (z, n)| Some(match acc { Some((az, an)) if az >= z => (az, an), _ => (z, n) }));
            let Some((ramp_z, rn)) = ramp_hit else {
                y += STEP;
                continue;
            };
            ramp_cells += 1;
            let frac = (rb.maxs.z - ramp_z) / (rb.maxs.z - rb.mins.z).max(1.0);

            // Topmost solid.
            let tr = trace_box(&map.world, o, Vec3::new(x, y, bot_z), mins, maxs);
            if let Some(h) = tr.hit.as_ref() {
                solid_cells += 1;
                let z = top_z + (bot_z - top_z) * tr.fraction;
                let is_ramp = h.brush_index >= prop_start && ramp.tris.contains(&(h.brush_index - prop_start));
                if !is_ramp && z > ramp_z + 1.0 && z < ramp_z + 120.0 {
                    let l = rn.length().max(1e-6);
                    small.push((z - ramp_z, Vec3::new(x, y, z), owner(h.brush_index), (rn.z / l).abs()));
                }
                if !is_ramp && z > ramp_z + 1.0 {
                    let e = solid_hits.entry(owner(h.brush_index)).or_insert((0, 0.0, o, 1.0));
                    e.0 += 1;
                    if z - ramp_z > e.1 {
                        e.1 = z - ramp_z;
                        e.2 = Vec3::new(x, y, z);
                    }
                    e.3 = e.3.min(frac);
                }
                if is_ramp && (z - ramp_z).abs() > 8.0 {
                    ramp_missing_solid += 1;
                }
            }

            // Drawn non-ramp geometry above the ramp surface.
            for p in &others {
                let b = p.render_bounds;
                if x < b.mins.x || x > b.maxs.x || y < b.mins.y || y > b.maxs.y {
                    continue;
                }
                let z = map.mesh.tris[p.render_tris.clone()]
                    .iter()
                    .filter_map(|t| ray_down_z(o, t.a, t.b, t.c))
                    .fold(None::<f32>, |acc, z| Some(acc.map_or(z, |a: f32| a.max(z))));
                if let Some(z) = z {
                    if z > ramp_z + 1.0 {
                        let key = format!(
                            "{} @({:.0},{:.0},{:.0}) solid={}",
                            p.model, p.origin.x, p.origin.y, p.origin.z, p.solid
                        );
                        let e = drawn_hits.entry(key).or_insert((0, 0.0, o, 1.0));
                        e.0 += 1;
                        if z - ramp_z > e.1 {
                            e.1 = z - ramp_z;
                            e.2 = Vec3::new(x, y, z);
                        }
                        e.3 = e.3.min(frac);
                    }
                }
            }
            y += STEP;
        }
        x += STEP;
    }

    if !verbose {
        let sm: Vec<_> = small.iter().filter(|c| c.0 >= 4.0 && c.0 < 64.0).collect();
        let worst = sm.iter().max_by(|a, b| a.0.total_cmp(&b.0));
        println!(
            "  {:<44} @({:>6.0},{:>6.0},{:>6.0}) cells={:>5} small(4..64u)={:>3}{}",
            ramp.model.trim_start_matches("models/project_tendies/ramps/"), ramp.origin.x, ramp.origin.y, ramp.origin.z, ramp_cells, sm.len(),
            worst.map(|w| format!("  worst +{:.1}u at ({:.0},{:.0},{:.0}) {}", w.0, w.1.x, w.1.y, w.1.z, w.2)).unwrap_or_default()
        );
        return;
    }
    println!("  {ramp_cells} cells on the drawn ramp surface ({STEP:.0}u grid), {solid_cells} with a solid below; ramp solid differs from drawn by >8u in {ramp_missing_solid} cells");
    println!("\n-- small solid protrusions (1..120u) above the ramp face, by cell --");
    small.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut hist = [0usize; 6];
    for (h, ..) in &small {
        let b = match *h { x if x < 4.0 => 0, x if x < 8.0 => 1, x if x < 16.0 => 2, x if x < 32.0 => 3, x if x < 64.0 => 4, _ => 5 };
        hist[b] += 1;
    }
    println!("  histogram <4:{} <8:{} <16:{} <32:{} <64:{} <120:{}", hist[0], hist[1], hist[2], hist[3], hist[4], hist[5]);
    for (h, at, who, nz) in small.iter() {
        println!("  +{h:>6.1}u at ({:.0},{:.0},{:.0}) ramp|nz|={nz:.2} {who}", at.x, at.y, at.z);
    }
    println!("\n-- SOLID geometry above the ramp surface (the player hits this) --");
    report(&solid_hits);
    println!("\n-- DRAWN geometry above the ramp surface (visual; passes through if solid=false) --");
    report(&drawn_hits);
}

fn report(m: &HashMap<String, (usize, f32, Vec3, f32)>) {
    if m.is_empty() {
        println!("  none");
        return;
    }
    let mut rows: Vec<_> = m.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1 .0));
    for (k, (n, h, at, frac)) in rows {
        println!(
            "  {k:<60} {n:>4} cells, up to {h:>6.1}u above ramp, worst at ({:.0},{:.0},{:.0}), starts {:.0}% down the ramp",
            at.x, at.y, at.z, frac * 100.0
        );
    }
}

/// Z of the intersection of a -Z ray from `o` with the triangle, if any.
fn ray_down_z(o: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let e1 = b - a;
    let e2 = c - a;
    let h = dir.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(h) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then(|| o.z - t)
}
