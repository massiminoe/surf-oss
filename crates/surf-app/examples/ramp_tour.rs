//! Walk a KSF world-record line and name the ramps in order, then say which
//! static props sit in the corridor of one of them.
//!
//! Built for "on the third ramp, a rock pokes through". The decisive question is
//! not whether a prop *looks* wrong but whether it is somewhere the real game
//! let the player be: the WR ghost is a recording from real CS:S, so any prop
//! collision that the ghost's hull passes through is geometry **we** invented.
//!
//!   cargo run -p surf-app --example ramp_tour --release -- surf_boreas
//!   cargo run -p surf-app --example ramp_tour --release -- surf_boreas 3

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::Vec3;
use surf_core::movement::Hull;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

/// Ticks of non-contact that end a ramp. Short airtime mid-ramp is normal;
/// a real gap between ramps is much longer.
const GAP_TICKS: usize = 25;
/// Ignore blips: a "ramp" of a handful of ticks is a graze, not a ramp.
const MIN_SEGMENT: usize = 20;

struct Segment {
    start: usize,
    end: usize,
    lo: Vec3,
    hi: Vec3,
    entry_speed: f32,
    exit_speed: f32,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_boreas".into());
    let focus: Option<usize> = args.next().and_then(|s| s.parse().ok());

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
    let hull = Hull::css_stand();

    // --- segment the run into ramps -------------------------------------
    let on_ramp: Vec<bool> = frames
        .iter()
        .map(|f| surf_core::is_on_surf_ramp(&map.world, f.origin, &hull))
        .collect();

    let mut segments: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < on_ramp.len() {
        if !on_ramp[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut j = i;
        let mut gap = 0;
        while j < on_ramp.len() {
            if on_ramp[j] {
                end = j;
                gap = 0;
            } else {
                gap += 1;
                if gap > GAP_TICKS {
                    break;
                }
            }
            j += 1;
        }
        if end - start >= MIN_SEGMENT {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for f in &frames[start..=end] {
                for (k, v) in [f.origin.x, f.origin.y, f.origin.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(v);
                    hi[k] = hi[k].max(v);
                }
            }
            segments.push(Segment {
                start,
                end,
                lo: Vec3::new(lo[0], lo[1], lo[2]),
                hi: Vec3::new(hi[0], hi[1], hi[2]),
                entry_speed: frames[start].velocity.length_2d(),
                exit_speed: frames[end].velocity.length_2d(),
            });
        }
        i = j;
    }

    println!(
        "{map_name}: {} ticks, {} ramp segments, {} props ({} solid)",
        frames.len(),
        segments.len(),
        map.props.len(),
        map.props.iter().filter(|p| p.solid).count()
    );
    for (n, s) in segments.iter().enumerate() {
        println!(
            "  ramp {:<2} ticks {:>5}..{:<5} ({:.1}s)  x[{:.0},{:.0}] y[{:.0},{:.0}] z[{:.0},{:.0}]  {:.0} -> {:.0} u/s",
            n + 1,
            s.start,
            s.end,
            (s.end - s.start) as f32 * 0.015,
            s.lo.x, s.hi.x, s.lo.y, s.hi.y, s.lo.z, s.hi.z,
            s.entry_speed, s.exit_speed
        );
    }

    // --- props in one ramp's corridor -----------------------------------
    let Some(n) = focus else { return };
    let Some(seg) = segments.get(n - 1) else {
        eprintln!("no ramp {n}");
        return;
    };
    const PAD: f32 = 400.0;
    println!("\n-- props within {PAD:.0}u of the ramp {n} corridor --");
    let mut hits = 0;
    for p in &map.props {
        if p.bounds.maxs.x < seg.lo.x - PAD
            || p.bounds.mins.x > seg.hi.x + PAD
            || p.bounds.maxs.y < seg.lo.y - PAD
            || p.bounds.mins.y > seg.hi.y + PAD
            || p.bounds.maxs.z < seg.lo.z - PAD
            || p.bounds.mins.z > seg.hi.z + PAD
        {
            continue;
        }
        hits += 1;
        println!(
            "  {:<46} at ({:>8.0},{:>8.0},{:>8.0}) solid={} phy={} tris={}",
            p.model,
            p.origin.x,
            p.origin.y,
            p.origin.z,
            p.solid,
            p.from_phy,
            p.tris.len()
        );
    }
    println!("  ({hits} props)");

    // --- what is DRAWN inside the corridor the player flies through? ----
    // A prop whose drawn volume overlaps the swept player hull is, literally,
    // in the way: either the map really is like that, or we are drawing it
    // somewhere it does not belong.
    println!("\n-- props DRAWN inside the swept player hull on ramp {n} --");
    let mut drawn: Vec<(f32, &surf_map::PropInstance, usize)> = Vec::new();
    for p in &map.props {
        let b = p.render_bounds;
        if b.mins.x == b.maxs.x {
            continue; // never decoded, nothing drawn
        }
        let mut best: Option<(f32, usize)> = None;
        for (t, f) in frames[seg.start..=seg.end].iter().enumerate() {
            let lo = Vec3::new(
                f.origin.x + hull.mins.x,
                f.origin.y + hull.mins.y,
                f.origin.z + hull.mins.z,
            );
            let hi = Vec3::new(
                f.origin.x + hull.maxs.x,
                f.origin.y + hull.maxs.y,
                f.origin.z + hull.maxs.z,
            );
            if hi.x < b.mins.x
                || lo.x > b.maxs.x
                || hi.y < b.mins.y
                || lo.y > b.maxs.y
                || hi.z < b.mins.z
                || lo.z > b.maxs.z
            {
                continue;
            }
            // Penetration depth on the shallowest axis: how far inside it we are.
            let depth = (hi.x.min(b.maxs.x) - lo.x.max(b.mins.x))
                .min(hi.y.min(b.maxs.y) - lo.y.max(b.mins.y))
                .min(hi.z.min(b.maxs.z) - lo.z.max(b.mins.z));
            if best.map_or(true, |(d, _)| depth > d) {
                best = Some((depth, seg.start + t));
            }
        }
        if let Some((depth, tick)) = best {
            drawn.push((depth, p, tick));
        }
    }
    drawn.sort_by(|a, b| b.0.total_cmp(&a.0));
    if drawn.is_empty() {
        println!("  none");
    }
    for (depth, p, tick) in &drawn {
        let b = p.render_bounds;
        println!(
            "  {:<46} solid={} overlap={:.0}u at tick {tick}  drawn x[{:.0},{:.0}] y[{:.0},{:.0}] z[{:.0},{:.0}]",
            p.model, p.solid, depth,
            b.mins.x, b.maxs.x, b.mins.y, b.maxs.y, b.mins.z, b.maxs.z
        );
    }

    // --- triangle-level: which DRAWN triangles does the player hull sweep hit? --
    // The sharp version of "it is in the way of the path": exact triangle vs the
    // player's AABB at every recorded position, not the prop's bounding box.
    println!("\n-- clearance from the recorded line to each drawn prop on ramp {n} --");
    // Binary "does it hit" answers nothing when the answer is always no. This
    // grows the player hull until it touches, which reports how much room the
    // line actually has: 0u means the prop is in the path, 8u means a shoulder
    // brushes it, 200u means it is scenery.
    const MARGINS: [f32; 7] = [0.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];
    let mut clear: Vec<(f32, &surf_map::PropInstance, usize)> = Vec::new();
    for p in &map.props {
        if p.render_tris.is_empty() {
            continue;
        }
        let mut best: Option<(f32, usize)> = None;
        for (t, f) in frames[seg.start..=seg.end].iter().enumerate() {
            let c = f.origin + Vec3::new(0.0, 0.0, (hull.mins.z + hull.maxs.z) * 0.5);
            for m in MARGINS {
                let e = Vec3::new(
                    (hull.maxs.x - hull.mins.x) * 0.5 + m,
                    (hull.maxs.y - hull.mins.y) * 0.5 + m,
                    (hull.maxs.z - hull.mins.z) * 0.5 + m,
                );
                if p.render_bounds.maxs.x < c.x - e.x
                    || p.render_bounds.mins.x > c.x + e.x
                    || p.render_bounds.maxs.y < c.y - e.y
                    || p.render_bounds.mins.y > c.y + e.y
                    || p.render_bounds.maxs.z < c.z - e.z
                    || p.render_bounds.mins.z > c.z + e.z
                {
                    continue;
                }
                if map.mesh.tris[p.render_tris.clone()]
                    .iter()
                    .any(|tri| tri_intersects_aabb(tri.a, tri.b, tri.c, c, e))
                {
                    if best.map_or(true, |(bm, _)| m < bm) {
                        best = Some((m, seg.start + t));
                    }
                    break;
                }
            }
        }
        if let Some((m, tick)) = best {
            clear.push((m, p, tick));
        }
    }
    clear.sort_by(|a, b| a.0.total_cmp(&b.0));
    if clear.is_empty() {
        println!(
            "  nothing drawn within {}u of the line",
            MARGINS[MARGINS.len() - 1]
        );
    }
    for (m, p, tick) in clear.iter().take(12) {
        println!(
            "  {:<46} solid={} clearance<={:.0}u at tick {tick} origin=({:.0},{:.0},{:.0})",
            p.model, p.solid, m, p.origin.x, p.origin.y, p.origin.z
        );
    }

    // --- triangle-level: is the drawn mesh actually swallowing the player? --
    // AABB overlap says nothing for a 900u-wide rock. This casts a ray from
    // each recorded position and counts crossings of the prop's own drawn
    // triangles: an odd count means that position is INSIDE the drawn solid.
    // The ghost is a real CS:S recording, so "inside" means we draw geometry
    // where the real game had air.
    println!("\n-- drawn geometry the recorded line is INSIDE (ray parity) --");
    let mut inside: Vec<(&surf_map::PropInstance, usize, usize)> = Vec::new();
    for p in &map.props {
        if p.render_tris.is_empty() {
            continue;
        }
        let b = p.render_bounds;
        let mut ticks = 0usize;
        let mut first = usize::MAX;
        for (t, f) in frames.iter().enumerate() {
            let o = f.origin;
            if !b.contains_point(o) {
                continue;
            }
            let mut crossings = 0;
            for tri in &map.mesh.tris[p.render_tris.clone()] {
                if ray_hits_tri(o, tri.a, tri.b, tri.c) {
                    crossings += 1;
                }
            }
            if crossings % 2 == 1 {
                ticks += 1;
                first = first.min(t);
            }
        }
        if ticks > 0 {
            inside.push((p, ticks, first));
        }
    }
    inside.sort_by_key(|r| std::cmp::Reverse(r.1));
    if inside.is_empty() {
        println!("  none — the drawn mesh never swallows the recorded line");
    }
    for (p, ticks, first) in &inside {
        println!(
            "  {:<46} solid={} INSIDE for {ticks} ticks (first tick {first}) origin=({:.0},{:.0},{:.0})",
            p.model, p.solid, p.origin.x, p.origin.y, p.origin.z
        );
    }

    // --- does the recorded line pass through anything we made solid? -----
    // The ghost is ground truth: CS:S let the player be at these positions.
    println!("\n-- prop collision the WR line passes through (we invented these) --");
    let brush_n = map.world.brushes.len();
    let prop_start = brush_n + map.prop_tri_start;
    let mut blame: std::collections::HashMap<String, (usize, usize, Vec3)> =
        std::collections::HashMap::new();
    for (t, f) in frames.iter().enumerate() {
        // A zero-length sweep reports whatever the hull is standing inside.
        let tr = trace_box(&map.world, f.origin, f.origin, hull.mins, hull.maxs);
        if !(tr.startsolid || tr.allsolid) {
            continue;
        }
        let Some(hit) = tr.hit.as_ref() else { continue };
        if hit.brush_index < prop_start {
            continue;
        }
        let tri = hit.brush_index - prop_start;
        let name = map
            .props
            .iter()
            .find(|p| p.tris.contains(&tri))
            .map(|p| p.model.clone())
            .unwrap_or_else(|| format!("<tri {tri}>"));
        let e = blame.entry(name).or_insert((0, t, f.origin));
        e.0 += 1;
    }
    if blame.is_empty() {
        println!("  none — every prop we collide is outside the recorded line");
    } else {
        let mut rows: Vec<_> = blame.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1 .0));
        for (name, (ticks, first, at)) in rows {
            println!(
                "  {name:<46} {ticks:>5} ticks buried, first at tick {first} ({:.0},{:.0},{:.0})",
                at.x, at.y, at.z
            );
        }
    }
}

/// Möller–Trumbore against a +X ray from `o`. Used for inside/outside parity,
/// so it counts a hit for any positive `t` regardless of facing.
fn ray_hits_tri(o: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
    let dir = Vec3::new(1.0, 0.0, 0.0);
    let e1 = b - a;
    let e2 = c - a;
    let h = dir.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-9 {
        return false;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(h) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    e2.dot(q) * inv > 1e-6
}

/// Separating-axis test, triangle vs axis-aligned box given as centre + extents.
/// Thirteen axes: three box normals, the triangle normal, and the nine
/// edge cross-products.
fn tri_intersects_aabb(a: Vec3, b: Vec3, c: Vec3, centre: Vec3, e: Vec3) -> bool {
    let v = [a - centre, b - centre, c - centre];
    let f = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];

    let axis_test = |axis: Vec3| -> bool {
        let r = e.x * axis.x.abs() + e.y * axis.y.abs() + e.z * axis.z.abs();
        let p: [f32; 3] = [v[0].dot(axis), v[1].dot(axis), v[2].dot(axis)];
        let lo = p[0].min(p[1]).min(p[2]);
        let hi = p[0].max(p[1]).max(p[2]);
        lo > r || hi < -r // true = separated on this axis
    };

    for edge in f {
        for base in [Vec3::X, Vec3::Y, Vec3::Z] {
            let axis = base.cross(edge);
            if axis.length_squared() > 1e-12 && axis_test(axis) {
                return false;
            }
        }
    }
    for base in [Vec3::X, Vec3::Y, Vec3::Z] {
        if axis_test(base) {
            return false;
        }
    }
    let normal = f[0].cross(f[1]);
    if normal.length_squared() > 1e-12 && axis_test(normal) {
        return false;
    }
    true
}
