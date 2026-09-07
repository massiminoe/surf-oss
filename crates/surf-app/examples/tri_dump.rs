//! Print the vertices of collision triangles by index, plus the hull box at a
//! point, for understanding a specific snag.
//!   cargo run -p surf-app --example tri_dump --release -- surf_cyberwave 7884 7885 7890 [--at x,y,z]
use std::path::PathBuf;
use surf_core::math::Vec3;
use surf_map::LoadedMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_cyberwave".into());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map = LoadedMap::load_path(root.join(format!("assets/maps/{map_name}.bsp"))).expect("map");
    let mut at: Option<Vec3> = None;
    let mut sweep: Option<Vec3> = None;
    let mut idx = Vec::new();
    while let Some(a) = args.next() {
        if a == "--sweep" {
            let v: Vec<f32> = args.next().unwrap().split(',').map(|s| s.parse().unwrap()).collect();
            sweep = Some(Vec3::new(v[0], v[1], v[2]));
        } else if a == "--at" {
            let v: Vec<f32> = args.next().unwrap().split(',').map(|s| s.parse().unwrap()).collect();
            at = Some(Vec3::new(v[0], v[1], v[2]));
        } else {
            idx.push(a.parse::<usize>().unwrap());
        }
    }
    if let Some(o) = at {
        println!("hull at ({:.2},{:.2},{:.2}): x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}]",
            o.x, o.y, o.z, o.x - 16.0, o.x + 16.0, o.y - 16.0, o.y + 16.0, o.z, o.z + 62.0);
        // Also list every tri within 8u of the hull box.
        let lo = Vec3::new(o.x - 24.0, o.y - 24.0, o.z - 8.0);
        let hi = Vec3::new(o.x + 24.0, o.y + 24.0, o.z + 70.0);
        for (i, t) in map.world.tris.iter().enumerate() {
            let b = t.bounds;
            if b.maxs.x < lo.x || b.mins.x > hi.x || b.maxs.y < lo.y || b.mins.y > hi.y || b.maxs.z < lo.z || b.mins.z > hi.z {
                continue;
            }
            if !idx.contains(&i) { idx.push(i); }
        }
    }
    if let (Some(o), Some(v)) = (at, sweep) {
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let end = o + v * 0.015;
        let full = surf_core::trace::trace_box(&map.world, o, end, mins, maxs);
        println!("full trace: frac={:.5} startsolid={} allsolid={} hit={:?}", full.fraction, full.startsolid, full.allsolid, full.hit.map(|h| (h.brush_index, h.plane_index, h.normal)));
        for &i in &idx {
            let t = &map.world.tris[i];
            let (pl, n) = t.planes();
            let r = surf_core::trace::trace_planes_pub(&pl[..n], i, o, end, mins, maxs);
            let (thin, tn) = t.planes_with(0.0);
            let ov = surf_core::trace::trace_planes_pub(&thin[..tn], i, o, o, mins, maxs);
            println!("tri {i}: away={:.3} frac={:.5} startsolid={} hit={:?} | straddle={}", v.dot(t.normal) * 0.015, r.fraction, r.startsolid, r.hit.map(|h| (h.plane_index, h.normal)), ov.startsolid);
        }
    } else if let Some(o) = at {
        for &i in &idx {
            planes_at(&map, i, o);
        }
    }
    for i in idx {
        let t = &map.world.tris[i];
        let who = if i < map.prop_tri_start {
            format!("disp#{}", map.disp_tri_owner[i])
        } else {
            let off = i - map.prop_tri_start;
            map.props.iter().find(|p| p.tris.contains(&off)).map(|p| format!("{} @({:.0},{:.0},{:.0}) tris {:?}", p.model, p.origin.x, p.origin.y, p.origin.z, p.tris)).unwrap_or("?".into())
        };
        println!("tri {i} [{who}] n=({:.3},{:.3},{:.3})", t.normal.x, t.normal.y, t.normal.z);
        for v in t.verts {
            println!("    ({:.2}, {:.2}, {:.2})", v.x, v.y, v.z);
        }
    }
}

#[allow(dead_code)]
pub fn planes_at(map: &LoadedMap, i: usize, o: Vec3) {
    let t = &map.world.tris[i];
    let mins = Vec3::new(-16.0, -16.0, 0.0);
    let maxs = Vec3::new(16.0, 16.0, 62.0);
    for thick in [t.thickness, 0.0] {
        let (pl, n) = t.planes_with(thick);
        println!("tri {i} thickness {thick}: {n} planes");
        for (k, p) in pl[..n].iter().enumerate() {
            let mut off = 0.0;
            for (nn, mn, mx) in [(p.normal.x, mins.x, maxs.x), (p.normal.y, mins.y, maxs.y), (p.normal.z, mins.z, maxs.z)] {
                off += if nn > 0.0 { -nn * mn } else { -nn * mx };
            }
            let d = p.normal.dot(o) - (p.dist + off);
            println!("   {k:2} n=({:.6},{:.6},{:.6}) dist={:.3} d={:.4}", p.normal.x, p.normal.y, p.normal.z, p.dist, d);
        }
    }
}
