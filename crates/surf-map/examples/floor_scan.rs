//! Sample the floor height across a box (default: the spawn's start zone
//! footprint) with the standing hull and print a histogram of landing
//! heights plus the min/max normal.z — a flat platform should be one bin.
//!
//!   cargo run -p surf-map --example floor_scan --release -- <bsp> x0 x1 y0 y1 ztop [step]
use std::collections::BTreeMap;
use surf_core::math::Vec3;
use surf_core::movement::Hull;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let map = LoadedMap::load_path(&a[0]).expect("load");
    let f = |i: usize| a[i].parse::<f32>().unwrap();
    let (x0, x1, y0, y1, ztop) = (f(1), f(2), f(3), f(4), f(5));
    let step = a.get(6).map(|s| s.parse().unwrap()).unwrap_or(32.0);
    let hull = Hull::css_stand();
    let mut hist: BTreeMap<i32, usize> = BTreeMap::new();
    let (mut nz_min, mut nz_max) = (1.0f32, 0.0f32);
    let mut x = x0;
    while x <= x1 {
        let mut y = y0;
        while y <= y1 {
            let s = Vec3::new(x, y, ztop);
            let tr = trace_box(
                &map.world,
                s,
                s - Vec3::new(0.0, 0.0, 4096.0),
                hull.mins,
                hull.maxs,
            );
            let z = if tr.startsolid {
                -1
            } else {
                tr.endpos.z.round() as i32
            };
            *hist.entry(z).or_default() += 1;
            if let Some(h) = &tr.hit {
                nz_min = nz_min.min(h.normal.z);
                nz_max = nz_max.max(h.normal.z);
            }
            y += step;
        }
        x += step;
    }
    for (z, n) in hist {
        println!("z={z:>7}  cells={n}");
    }
    println!("normal.z range {nz_min:.3}..{nz_max:.3}  (-1 = hull started solid)");
}
