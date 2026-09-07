//! Why is the player stuck at a point? Reports every solid whose prism the
//! standing hull starts inside, named by prop model where possible.
use surf_core::math::Vec3;
use surf_core::movement::Hull;
use surf_core::trace::trace_planes_pub;
use surf_map::LoadedMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: stuck_probe <bsp> [x y z]");
    let map = LoadedMap::load_path(&path).expect("load");
    let at = match (args.next(), args.next(), args.next()) {
        (Some(x), Some(y), Some(z)) => Vec3::new(
            x.parse().unwrap(),
            y.parse().unwrap(),
            z.parse().unwrap(),
        ),
        _ => map.spawn_origin,
    };
    let hull = Hull::css_stand();
    println!("map {} spawn {:?} probe {:?}", map.name, map.spawn_origin, at);
    println!("hull mins {:?} maxs {:?}", hull.mins, hull.maxs);

    let w = &map.world;
    let mut n = 0;
    for (i, b) in w.brushes.iter().enumerate() {
        let tr = trace_planes_pub(&b.planes, i, at, at, hull.mins, hull.maxs);
        if tr.startsolid || tr.allsolid {
            n += 1;
            println!(
                "  BRUSH {i} startsolid={} allsolid={} bounds {:?}..{:?} planes={}",
                tr.startsolid, tr.allsolid, b.bounds.mins, b.bounds.maxs, b.planes.len()
            );
        }
    }
    for (ti, tri) in w.tris.iter().enumerate() {
        let tr = trace_planes_pub(&tri.planes().0[..tri.planes().1], ti, at, at, hull.mins, hull.maxs);
        if tr.startsolid || tr.allsolid {
            n += 1;
            let src = if ti < map.prop_tri_start {
                "disp".to_string()
            } else {
                map.props
                    .iter()
                    .find(|p| p.tris.contains(&(ti - map.prop_tri_start)))
                    .map(|p| format!("prop {} @ {:?}", p.model, p.origin))
                    .unwrap_or_else(|| "prop ?".into())
            };
            println!(
                "  TRI {ti} ({src}) startsolid={} allsolid={} bounds {:?}..{:?}",
                tr.startsolid, tr.allsolid, tri.bounds.mins, tri.bounds.maxs
            );
        }
    }
    println!("total solids containing hull: {n}");
    let down = surf_core::trace::trace_box(
        w,
        at,
        at + Vec3::new(0.0, 0.0, -512.0),
        hull.mins,
        hull.maxs,
    );
    println!(
        "hull trace down 512: frac={:.4} endpos={:?} startsolid={} normal={:?}",
        down.fraction,
        down.endpos,
        down.startsolid,
        down.hit.map(|h| h.normal)
    );
}
