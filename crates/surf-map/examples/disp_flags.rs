//! Dump displacement minTess/contents values, and name the displacements
//! whose footprint contains given XY points.
//!   cargo run -p surf-map --example disp_flags --release -- assets/maps/surf_boreas.bsp 6450,10470 9300,10500
use std::collections::BTreeMap;
use vbsp::{Bsp, Handle};
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("map");
    let pts: Vec<[f32; 2]> = args
        .map(|s| { let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect(); [v[0], v[1]] })
        .collect();
    let data = std::fs::read(&path).unwrap();
    let bsp = Bsp::read(&data).unwrap();
    let mut hist: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    for (i, d) in bsp.displacements.iter().enumerate() {
        *hist.entry((d.minimum_tesselation, d.contents)).or_default() += 1;
        let h = Handle::new(&bsp, d);
        let Some(f) = h.face() else { continue };
        let vs: Vec<_> = f.vertices().map(|v| v.position).collect();
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for v in &vs { x0 = x0.min(v.x); x1 = x1.max(v.x); y0 = y0.min(v.y); y1 = y1.max(v.y); }
        if surf_map::disp::disp_flags(d) & surf_map::disp::DISP_NOHULL_COLL != 0 {
            let zs: Vec<f32> = vs.iter().map(|v| v.z).collect();
            println!("NOHULL disp #{i} flags={:#x} contents={:#x} x[{x0:.0},{x1:.0}] y[{y0:.0},{y1:.0}] z[{:.0},{:.0}]", surf_map::disp::disp_flags(d), d.contents, zs.iter().cloned().fold(f32::MAX, f32::min), zs.iter().cloned().fold(f32::MIN, f32::max));
        }
        for p in &pts {
            if p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1 {
                println!("point ({},{}) in disp #{i}: power={} minTess={:#x} contents={:#x} face={} corners x[{x0:.0},{x1:.0}] y[{y0:.0},{y1:.0}] z[{:.0},{:.0}]",
                    p[0], p[1], d.power, d.minimum_tesselation, d.contents, d.map_face,
                    vs.iter().map(|v| v.z).fold(f32::MAX, f32::min), vs.iter().map(|v| v.z).fold(f32::MIN, f32::max));
            }
        }
    }
    println!("{} displacements; (minTess, contents) -> count:", bsp.displacements.len());
    for ((m, c), n) in hist { println!("  minTess={m:#x} contents={c:#x}: {n}"); }
}
