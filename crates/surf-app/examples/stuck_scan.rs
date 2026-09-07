//! Hunt for "frozen with units": the hull stops moving while keeping its
//! velocity. Perturbs a KSF WR line around every ramp (lateral offsets toward
//! the edges, height offsets, sideways velocity) and runs the real `tick()`.
//!
//!   cargo run -p surf-app --example stuck_scan --release -- surf_cyberwave [ramp]

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};
use surf_core::trace::{point_contents_box, trace_box, trace_planes_pub};
use surf_map::LoadedMap;

const GAP_TICKS: usize = 25;
const MIN_SEGMENT: usize = 20;
const RUN_TICKS: usize = 40;

struct Frozen {
    origin: Vec3,
    velocity: Vec3,
    ticks_frozen: usize,
    tick_in_run: usize,
    seed_tick: usize,
    lateral: f32,
    dz: f32,
    vside: f32,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_cyberwave".into());
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
    let vars = MoveVars::momentum_surf();

    let on_ramp: Vec<bool> = frames
        .iter()
        .map(|f| surf_core::is_on_surf_ramp(&map.world, f.origin, &hull))
        .collect();
    let mut segments: Vec<(usize, usize)> = Vec::new();
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
        if end - start + 1 >= MIN_SEGMENT {
            segments.push((start, end));
        }
        i = end + 1;
    }

    let mut grand = 0usize;
    let mut worst: Option<Frozen> = None;
    for (si, (s0, s1)) in segments.iter().enumerate() {
        let n = si + 1;
        if focus.is_some_and(|f| f != n) {
            continue;
        }
        let mut count = 0usize;
        let mut trials = 0usize;
        let mut first: Option<Frozen> = None;
        let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
        for t in (*s0..=*s1).step_by(3) {
            let f = &frames[t];
            let v = f.velocity;
            let fwd = Vec3::new(v.x, v.y, 0.0);
            if fwd.length_2d() < 100.0 {
                continue;
            }
            let fwd = fwd.normalize();
            let side = Vec3::new(-fwd.y, fwd.x, 0.0);
            for lat in [-128.0, -96.0, -64.0, -48.0, -32.0, -16.0, 0.0, 16.0, 32.0, 48.0, 64.0, 96.0, 128.0] {
                for dz in [-32.0, -16.0, -8.0, 0.0, 8.0, 24.0] {
                    for vside in [-600.0, -300.0, 0.0, 300.0, 600.0] {
                        let seed = f.origin + side * lat + Vec3::new(0.0, 0.0, dz);
                        if point_contents_box(&map.world, seed, hull.mins, hull.maxs) {
                            continue;
                        }
                        trials += 1;
                        let mut p = PlayerState {
                            origin: seed,
                            velocity: v + side * vside,
                            viewangles: Angle::new(f.pitch, f.yaw, 0.0),
                            grounded: false,
                            ..Default::default()
                        };
                        let cmd = UserCmd {
                            viewangles: Angle::new(f.pitch, f.yaw, 0.0),
                            ..UserCmd::default()
                        };
                        let mut frozen_run = 0usize;
                        let mut hist: Vec<PlayerState> = vec![p.clone()];
                        for k in 0..RUN_TICKS {
                            let next = surf_core::tick(&map.world, &p, &cmd, &vars);
                            let moved = (next.origin - p.origin).length();
                            let want = next.velocity.length() * vars.tick_interval;
                            if !next.grounded && next.velocity.length() > 300.0 && want > 5.0 && moved < 0.5 {
                                frozen_run += 1;
                                if frozen_run == 3 {
                                    count += 1;
                                    let tr = trace_box(&map.world, p.origin, p.origin + p.velocity * vars.tick_interval, hull.mins, hull.maxs);
                                    let key = if tr.allsolid { "allsolid" } else if tr.startsolid { "startsolid" } else if tr.hit.is_some() { "hit" } else { "free" };
                                    *kinds.entry(key).or_insert(0usize) += 1;
                                    let fz = Frozen {
                                        origin: p.origin,
                                        velocity: p.velocity,
                                        ticks_frozen: frozen_run,
                                        tick_in_run: k,
                                        seed_tick: t,
                                        lateral: lat,
                                        dz,
                                        vside,
                                    };
                                    if first.is_none() {
                                        // Find the tick that put the hull inside a solid.
                                        let mut entry = None;
                                        for (h, st) in hist.iter().enumerate() {
                                            if point_contents_box(&map.world, st.origin, hull.mins, hull.maxs) {
                                                entry = Some(h);
                                                break;
                                            }
                                        }
                                        if let Some(h) = entry {
                                            let before = &hist[h - 1];
                                            println!("    entry: became solid at run tick {h}; tick before: origin=({:.2},{:.2},{:.2}) vel=({:.1},{:.1},{:.1})",
                                                before.origin.x, before.origin.y, before.origin.z, before.velocity.x, before.velocity.y, before.velocity.z);
                                            println!("           after: origin=({:.2},{:.2},{:.2}) vel=({:.1},{:.1},{:.1})",
                                                hist[h].origin.x, hist[h].origin.y, hist[h].origin.z, hist[h].velocity.x, hist[h].velocity.y, hist[h].velocity.z);
                                            dissect(&map, &hull, &vars, before.origin, before.velocity, 0);
                                            println!("    -- unswept at the solid position --");
                                            dissect(&map, &hull, &vars, hist[h].origin, Vec3::ZERO, 0);
                                        } else {
                                            println!("    entry: never startsolid (frozen some other way)");
                                        }
                                        first = Some(fz);
                                    }
                                    break;
                                }
                            } else {
                                frozen_run = 0;
                            }
                            p = next;
                            hist.push(p.clone());
                        }
                    }
                }
            }
        }
        grand += count;
        println!(
            "ramp {n:2} ticks {s0}..{s1}: {count} frozen trials of {trials} ({:.2}%)",
            100.0 * count as f32 / trials.max(1) as f32
        );
        println!("    mechanisms: {kinds:?}");
        if let Some(fz) = first {
            println!(
                "    first: seed tick {} lat {} dz {} vside {} -> frozen at run tick {} origin=({:.4},{:.4},{:.4}) vel=({:.4},{:.4},{:.4})",
                fz.seed_tick, fz.lateral, fz.dz, fz.vside, fz.tick_in_run,
                fz.origin.x, fz.origin.y, fz.origin.z, fz.velocity.x, fz.velocity.y, fz.velocity.z
            );
            if worst.is_none() {
                worst = Some(fz);
            }
        }
    }
    println!("total frozen trials: {grand}");

    if let Some(fz) = worst {
        dissect(&map, &hull, &vars, fz.origin, fz.velocity, fz.ticks_frozen);
    }
}

fn dissect(map: &LoadedMap, hull: &Hull, vars: &MoveVars, start: Vec3, vel: Vec3, _n: usize) {
    let end = start + vel * vars.tick_interval;
    let tr = trace_box(&map.world, start, end, hull.mins, hull.maxs);
    println!(
        "dissect: full trace fraction={:.5} startsolid={} allsolid={} hit={:?}",
        tr.fraction,
        tr.startsolid,
        tr.allsolid,
        tr.hit.as_ref().map(|h| (h.brush_index, h.plane_index, h.normal))
    );
    let brush_n = map.world.brushes.len();
    for (bi, b) in map.world.brushes.iter().enumerate() {
        let r = trace_planes_pub(&b.planes, bi, start, end, hull.mins, hull.maxs);
        if r.fraction >= 1.0 && !r.startsolid {
            continue;
        }
        println!("  brush {bi}: frac={:.4} startsolid={} allsolid={} hit={:?}", r.fraction, r.startsolid, r.allsolid, r.hit.as_ref().map(|h| (h.plane_index, h.normal)));
        for (pi, pl) in b.planes.iter().enumerate() {
            let ep_d = {
                // expanded distance
                let mut off = 0.0;
                for (n, mn, mx) in [(pl.normal.x, hull.mins.x, hull.maxs.x), (pl.normal.y, hull.mins.y, hull.maxs.y), (pl.normal.z, hull.mins.z, hull.maxs.z)] {
                    off += if n > 0.0 { -n * mn } else { -n * mx };
                }
                pl.normal.dot(start) - (pl.dist + off)
            };
            println!("      plane {pi} n=({:.3},{:.3},{:.3}) d_start={:.4}", pl.normal.x, pl.normal.y, pl.normal.z, ep_d);
        }
    }
    for (i, tri) in map.world.tris.iter().enumerate() {
        let lo = Vec3::new(start.x.min(end.x) + hull.mins.x, start.y.min(end.y) + hull.mins.y, start.z.min(end.z) + hull.mins.z);
        let hi = Vec3::new(start.x.max(end.x) + hull.maxs.x, start.y.max(end.y) + hull.maxs.y, start.z.max(end.z) + hull.maxs.z);
        if hi.x < tri.bounds.mins.x || lo.x > tri.bounds.maxs.x || hi.y < tri.bounds.mins.y || lo.y > tri.bounds.maxs.y || hi.z < tri.bounds.mins.z || lo.z > tri.bounds.maxs.z {
            continue;
        }
        let r = trace_planes_pub(&tri.planes().0[..tri.planes().1], brush_n + i, start, end, hull.mins, hull.maxs);
        if r.fraction >= 1.0 && !r.startsolid {
            continue;
        }
        let who = if i < map.prop_tri_start {
            format!("disp#{}", map.disp_tri_owner[i])
        } else {
            let off = i - map.prop_tri_start;
            map.props.iter().find(|p| p.tris.contains(&off)).map(|p| format!("prop {} @({:.0},{:.0},{:.0})", p.model, p.origin.x, p.origin.y, p.origin.z)).unwrap_or("prop ?".into())
        };
        println!("  tri {i} [{who}]: frac={:.4} startsolid={} allsolid={} hit={:?}", r.fraction, r.startsolid, r.allsolid, r.hit.as_ref().map(|h| (h.plane_index, h.normal)));
        let (tri_planes, tri_n) = tri.planes();
        for (pi, pl) in tri_planes[..tri_n].iter().enumerate() {
            let mut off = 0.0;
            for (n, mn, mx) in [(pl.normal.x, hull.mins.x, hull.maxs.x), (pl.normal.y, hull.mins.y, hull.maxs.y), (pl.normal.z, hull.mins.z, hull.maxs.z)] {
                off += if n > 0.0 { -n * mn } else { -n * mx };
            }
            let d = pl.normal.dot(start) - (pl.dist + off);
            let d2 = pl.normal.dot(end) - (pl.dist + off);
            println!("      plane {pi} n=({:.3},{:.3},{:.3}) d_start={:.4} d_end={:.4}", pl.normal.x, pl.normal.y, pl.normal.z, d, d2);
        }
    }
}
