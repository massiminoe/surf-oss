//! Why does this map not play right? Dumps everything about the spawn area:
//! entity census, brush entities and static props near the spawn, displacement
//! powers, the collision actually present in the spawn column, and a trace
//! straight down.
//!
//!   cargo run -p surf-map --example diag_map --release -- assets/maps/surf_boreas.bsp
//!
//! This is how boreas' missing start platform was found: the trace fell 235u
//! onto a sloped displacement while the start zone floor sat at 14736, and the
//! prop list showed a `solid=Physics` `details/dek01.mdl` at exactly that height
//! being skipped by the loader.

use std::collections::BTreeMap;

use surf_core::math::Vec3;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn main() {
    let path = std::env::args().nth(1).expect("bsp path");
    let data = std::fs::read(&path).expect("read bsp");
    let bsp = vbsp::Bsp::read(&data).expect("parse bsp");

    let t0 = std::time::Instant::now();
    let map = LoadedMap::load_path(&path).expect("load");
    println!("load time: {:.2}s", t0.elapsed().as_secs_f32());
    println!("== {} ==", map.name);
    println!(
        "world brushes={} coll tris={} mesh tris={} kill_z={:.0}",
        map.world.brushes.len(),
        map.world.tris.len(),
        map.mesh.tris.len(),
        map.kill_z
    );
    println!(
        "bounds mins={:?} maxs={:?}",
        map.world_bounds.mins, map.world_bounds.maxs
    );
    println!("spawn {:?} angles {:?}", map.spawn_origin, map.spawn_angles);
    println!(
        "teleports={} pushes={} gravities={}",
        map.teleports.len(),
        map.pushes.len(),
        map.gravities.len()
    );
    println!("bsp models={}", bsp.models.len());

    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    for ent in bsp.entities.iter() {
        *census
            .entry(ent.prop("classname").unwrap_or("?").to_string())
            .or_insert(0) += 1;
    }
    println!("\n-- entity census --");
    for (k, v) in &census {
        println!("{v:5}  {k}");
    }

    // Entities near the spawn.
    let s = map.spawn_origin;
    println!("\n-- entities within 3000u of spawn --");
    for ent in bsp.entities.iter() {
        let class = ent.prop("classname").unwrap_or("?");
        let o = ent
            .prop("origin")
            .and_then(parse_vec3)
            .unwrap_or(Vec3::ZERO);
        let model = ent.prop("model").unwrap_or("");
        // brush ents have origin 0 with a *model — use the model bounds instead
        let near = if model.starts_with('*') {
            let idx: usize = model[1..].parse().unwrap_or(0);
            match bsp.models.get(idx) {
                Some(m) => {
                    let c = Vec3::new(
                        (m.mins.x + m.maxs.x) * 0.5,
                        (m.mins.y + m.maxs.y) * 0.5,
                        (m.mins.z + m.maxs.z) * 0.5,
                    ) + o;
                    (c - s).length() < 3000.0
                }
                None => false,
            }
        } else {
            (o - s).length() < 3000.0
        };
        if !near {
            continue;
        }
        let name = ent.prop("targetname").unwrap_or("");
        let solidity = ent
            .properties()
            .find_map(|(k, v)| k.eq_ignore_ascii_case("solidity").then_some(v))
            .unwrap_or("");
        let extra = if model.starts_with('*') {
            let idx: usize = model[1..].parse().unwrap_or(0);
            match bsp.models.get(idx) {
                Some(m) => format!(
                    " model_mins=({:.0},{:.0},{:.0}) maxs=({:.0},{:.0},{:.0})",
                    m.mins.x, m.mins.y, m.mins.z, m.maxs.x, m.maxs.y, m.maxs.z
                ),
                None => String::new(),
            }
        } else {
            String::new()
        };
        println!(
            "{class:28} name={name:20} model={model:6} solidity={solidity:2} origin=({:.0},{:.0},{:.0}){extra}",
            o.x, o.y, o.z
        );
    }

    // What is under the spawn?
    println!("\n-- trace down from spawn --");
    let mins = Vec3::new(-16.0, -16.0, 0.0);
    let maxs = Vec3::new(16.0, 16.0, 62.0);
    let end = s - Vec3::new(0.0, 0.0, 20000.0);
    let tr = trace_box(&map.world, s, end, mins, maxs);
    println!(
        "fraction={:.6} endpos=({:.1},{:.1},{:.1}) drop={:.1} startsolid={} hit_normal={:?}",
        tr.fraction,
        tr.endpos.x,
        tr.endpos.y,
        tr.endpos.z,
        s.z - tr.endpos.z,
        tr.startsolid,
        tr.hit.as_ref().map(|h| h.normal)
    );

    // Collision geometry near spawn (any brush/tri within 512u horizontally).
    let mut brushes_near = 0;
    for b in &map.world.brushes {
        let c = Vec3::new(
            (b.bounds.mins.x + b.bounds.maxs.x) * 0.5,
            (b.bounds.mins.y + b.bounds.maxs.y) * 0.5,
            (b.bounds.mins.z + b.bounds.maxs.z) * 0.5,
        );
        if (c.x - s.x).abs() < 1024.0 && (c.y - s.y).abs() < 1024.0 && (c.z - s.z).abs() < 1024.0 {
            brushes_near += 1;
        }
    }
    let mut tris_near = 0;
    for t in &map.world.tris {
        let c = Vec3::new(
            (t.bounds.mins.x + t.bounds.maxs.x) * 0.5,
            (t.bounds.mins.y + t.bounds.maxs.y) * 0.5,
            (t.bounds.mins.z + t.bounds.maxs.z) * 0.5,
        );
        if (c.x - s.x).abs() < 1024.0 && (c.y - s.y).abs() < 1024.0 && (c.z - s.z).abs() < 1024.0 {
            tris_near += 1;
        }
    }
    dump_models(&bsp, s);

    println!("\n-- world collision brushes in the spawn XY column --");
    let mut n = 0;
    for b in &map.world.brushes {
        if s.x >= b.bounds.mins.x
            && s.x <= b.bounds.maxs.x
            && s.y >= b.bounds.mins.y
            && s.y <= b.bounds.maxs.y
        {
            println!(
                "  brush z {:.0}..{:.0} planes={}",
                b.bounds.mins.z,
                b.bounds.maxs.z,
                b.planes.len()
            );
            n += 1;
        }
    }
    println!("  ({n} brushes)");

    println!("\n-- collision tris in the spawn XY column (top 12 by z) --");
    let mut zs: Vec<f32> = map
        .world
        .tris
        .iter()
        .filter(|t| {
            s.x >= t.bounds.mins.x
                && s.x <= t.bounds.maxs.x
                && s.y >= t.bounds.mins.y
                && s.y <= t.bounds.maxs.y
        })
        .map(|t| t.bounds.maxs.z)
        .collect();
    zs.sort_by(|a, b| b.partial_cmp(a).unwrap());
    println!("  count={} top={:?}", zs.len(), &zs[..zs.len().min(12)]);

    {
        // Every prop the loader treats as solid, grouped by model.
        use std::collections::BTreeMap;
        let mut by_model: BTreeMap<String, (usize, String)> = BTreeMap::new();
        for prop in &bsp.static_props.props.props {
            let name = bsp
                .static_props
                .dict
                .name
                .get(prop.prop_type as usize)
                .map(|n| n.as_str().to_string())
                .unwrap_or_else(|| "?".into());
            let e = by_model
                .entry(name)
                .or_insert((0, format!("{:?}", prop.solid)));
            e.0 += 1;
        }
        println!("\n-- static prop models (count, solid type) --");
        for (name, (n, solid)) in &by_model {
            println!("  {n:>5}  {solid:<8} {name}");
        }
    }

    {
        // A visible ramp with no collision near it is a fall-through ramp.
        // Boreas renders 9 `ramps/ramp_s1.mdl` at solid=None, which in Source
        // means the surfable surface must come from a world PLAYERCLIP brush.
        println!("\n-- ramp props: is there collision where they are drawn? --");
        for prop in &bsp.static_props.props.props {
            let name = bsp
                .static_props
                .dict
                .name
                .get(prop.prop_type as usize)
                .map(|n| n.as_str())
                .unwrap_or("?");
            if !name.contains("ramp") {
                continue;
            }
            let o = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
            let r = 512.0;
            let brushes = map
                .world
                .brushes
                .iter()
                .filter(|b| {
                    b.bounds.maxs.x > o.x - r
                        && b.bounds.mins.x < o.x + r
                        && b.bounds.maxs.y > o.y - r
                        && b.bounds.mins.y < o.y + r
                        && b.bounds.maxs.z > o.z - r
                        && b.bounds.mins.z < o.z + r
                })
                .count();
            let tris = map
                .world
                .tris
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.bounds.maxs.x > o.x - r
                        && t.bounds.mins.x < o.x + r
                        && t.bounds.maxs.y > o.y - r
                        && t.bounds.mins.y < o.y + r
                        && t.bounds.maxs.z > o.z - r
                        && t.bounds.mins.z < o.z + r
                })
                .fold((0usize, 0usize), |(d, p), (i, _)| {
                    if i < map.prop_tri_start {
                        (d + 1, p)
                    } else {
                        (d, p + 1)
                    }
                });
            println!(
                "  {:?} ({:.0},{:.0},{:.0}) {name}: within 512u -> brushes={brushes} disp_tris={} prop_tris={}",
                prop.solid, o.x, o.y, o.z, tris.0, tris.1
            );
        }
    }

    println!("\n-- static props within 1500u of spawn --");
    let mut np = 0;
    for prop in &bsp.static_props.props.props {
        let o = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
        if (o - s).length() > 1500.0 {
            continue;
        }
        let name = bsp
            .static_props
            .dict
            .name
            .get(prop.prop_type as usize)
            .map(|n| n.as_str())
            .unwrap_or("?");
        println!(
            "  {:?} ({:.0},{:.0},{:.0}) ang=({:.0},{:.0},{:.0}) {name}",
            prop.solid, o.x, o.y, o.z, prop.angles.pitch, prop.angles.yaw, prop.angles.roll
        );
        np += 1;
    }
    println!(
        "  ({np} props near; total props={})",
        bsp.static_props.props.props.len()
    );

    println!("\n-- displacement census --");
    let mut powers: std::collections::BTreeMap<u32, usize> = Default::default();
    let mut spawn_disps = Vec::new();
    for (i, d) in bsp.displacements.iter().enumerate() {
        *powers.entry(d.power as u32).or_insert(0) += 1;
        let h = vbsp::Handle::new(&bsp, d);
        if let Some(f) = h.face() {
            let vs: Vec<_> = f.vertices().map(|v| v.position).collect();
            if vs.len() == 4 {
                let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                let (mut mnz, mut mxz) = (f32::MAX, f32::MIN);
                for v in &vs {
                    mnx = mnx.min(v.x);
                    mxx = mxx.max(v.x);
                    mny = mny.min(v.y);
                    mxy = mxy.max(v.y);
                    mnz = mnz.min(v.z);
                    mxz = mxz.max(v.z);
                }
                if s.x >= mnx - 64.0
                    && s.x <= mxx + 64.0
                    && s.y >= mny - 64.0
                    && s.y <= mxy + 64.0
                    && mxz > s.z - 1000.0
                    && mnz < s.z + 500.0
                {
                    spawn_disps.push((i, d.power, mnx, mny, mnz, mxx, mxy, mxz, d.contents));
                }
            }
        }
    }
    println!(
        "  power histogram: {powers:?}  total disps={}",
        bsp.displacements.len()
    );
    println!("  disps over spawn:");
    for (i, pw, mnx, mny, mnz, mxx, mxy, mxz, contents) in spawn_disps {
        println!(
            "    disp {i} power={pw} contents=0x{contents:x} face xy ({mnx:.0},{mny:.0})..({mxx:.0},{mxy:.0}) z {mnz:.0}..{mxz:.0}"
        );
    }

    println!("\n-- tris near spawn XY (±96u) with z > spawn-400 --");
    for (i, t) in map.world.tris.iter().enumerate() {
        if t.bounds.maxs.z < s.z - 400.0 {
            continue;
        }
        if t.bounds.maxs.x < s.x - 96.0
            || t.bounds.mins.x > s.x + 96.0
            || t.bounds.maxs.y < s.y - 96.0
            || t.bounds.mins.y > s.y + 96.0
        {
            continue;
        }
        println!(
            "  tri {i} z {:.1}..{:.1} xy ({:.0},{:.0})..({:.0},{:.0}) planes={} n0={:?} n1={:?}",
            t.bounds.mins.z,
            t.bounds.maxs.z,
            t.bounds.mins.x,
            t.bounds.mins.y,
            t.bounds.maxs.x,
            t.bounds.maxs.y,
            t.planes().1,
            Some(t.normal),
            Some(-t.normal),
        );
    }

    println!("\n-- point trace down from spawn --");
    let ptr = trace_box(&map.world, s, end, Vec3::ZERO, Vec3::ZERO);
    println!(
        "  fraction={:.6} endpos_z={:.1} drop={:.1} n={:?}",
        ptr.fraction,
        ptr.endpos.z,
        s.z - ptr.endpos.z,
        ptr.hit.as_ref().map(|h| h.normal)
    );

    println!("collision near spawn (±1024u box): brushes={brushes_near} tris={tris_near}");
}

fn dump_models(bsp: &vbsp::Bsp, s: Vec3) {
    println!("\n-- bsp models whose XY column contains the spawn --");
    // map model index -> referencing entity
    let mut owner: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for ent in bsp.entities.iter() {
        if let Some(m) = ent.prop("model") {
            if let Some(rest) = m.strip_prefix('*') {
                if let Ok(i) = rest.parse::<usize>() {
                    owner.insert(
                        i,
                        format!(
                            "{} name={}",
                            ent.prop("classname").unwrap_or("?"),
                            ent.prop("targetname").unwrap_or("")
                        ),
                    );
                }
            }
        }
    }
    for (i, m) in bsp.models.iter().enumerate() {
        let o = owner
            .get(&i)
            .cloned()
            .unwrap_or_else(|| "<unreferenced>".into());
        let ox = 0.0;
        if s.x >= m.mins.x + ox && s.x <= m.maxs.x && s.y >= m.mins.y && s.y <= m.maxs.y {
            println!(
                "model *{i:<3} z {:.0}..{:.0}  xy ({:.0},{:.0})..({:.0},{:.0})  {o}",
                m.mins.z, m.maxs.z, m.mins.x, m.mins.y, m.maxs.x, m.maxs.y
            );
        }
    }
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let mut it = s.split_whitespace().map(|p| p.parse::<f32>().ok());
    Some(Vec3::new(it.next()??, it.next()??, it.next()??))
}
