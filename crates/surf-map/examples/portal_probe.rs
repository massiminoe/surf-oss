//! Which solid props sit inside / beside a map's stage-transition triggers?
//!
//!   cargo run -p surf-map --example portal_probe --release -- <map.bsp> [pad]
//!
//! A stage transition you have to *thread* is the symptom; this names the thing
//! doing the blocking. Every `trigger_teleport` targeting a `stageN` destination
//! and every `zone_stage*_start` / `zone_map_end` volume is listed with its world
//! box, then every solid prop whose collision bounds land inside it (`IN`) or
//! within `pad` units of it (`near`, default 256), with the model name, whether
//! its collision came from a `.phy`, and its triangle count.
//!
//! This is how botanica's blocked portals were found: `portal2.mdl` — the ring
//! you are supposed to fly through — was `IN` its own transition trigger, with
//! `phy=false`, i.e. collided against its render mesh.
use surf_core::math::Vec3;
use surf_map::LoadedMap;

fn main() {
    let path = std::env::args().nth(1).expect("bsp path");
    let data = std::fs::read(&path).expect("read bsp");
    let bsp = vbsp::Bsp::read(&data).expect("parse bsp");
    let map = LoadedMap::load_path(&path).expect("load");

    let mut boxes: Vec<(String, Vec3, Vec3)> = Vec::new();
    for ent in bsp.entities.iter() {
        let class = ent.prop("classname").unwrap_or("");
        let target = ent.prop("target").unwrap_or("");
        let name = ent.prop("targetname").unwrap_or("");
        let interesting = (class == "trigger_teleport" && target.starts_with("stage"))
            || (class == "trigger_multiple" && (name.starts_with("zone_stage") || name == "zone_map_end"));
        if !interesting { continue; }
        let model = ent.prop("model").unwrap_or("");
        if !model.starts_with('*') { continue; }
        let idx: usize = model[1..].parse().unwrap_or(0);
        let m = match bsp.models.get(idx) { Some(m) => m, None => continue };
        let o = ent.prop("origin").and_then(pv).unwrap_or(Vec3::ZERO);
        boxes.push((
            format!("{class} {}{}", if name.is_empty() { format!("->{target}") } else { name.to_string() }, format!(" [{model}]")),
            Vec3::new(m.mins.x, m.mins.y, m.mins.z) + o,
            Vec3::new(m.maxs.x, m.maxs.y, m.maxs.z) + o,
        ));
    }
    // Keep only ones above z = 0 (fail planes live far below on this map).
    boxes.sort_by(|a, b| a.1.z.partial_cmp(&b.1.z).unwrap());
    let pad: f32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(256.0);
    for (label, mins, maxs) in &boxes {
        println!("\n== {label}  mins={:?} maxs={:?}", tup(*mins), tup(*maxs));
        let lo = Vec3::new(mins.x - pad, mins.y - pad, mins.z - pad);
        let hi = Vec3::new(maxs.x + pad, maxs.y + pad, maxs.z + pad);
        let mut n = 0;
        for p in &map.props {
            if !p.solid || p.skybox { continue; }
            let b = &p.bounds;
            if b.maxs.x < lo.x || b.mins.x > hi.x || b.maxs.y < lo.y || b.mins.y > hi.y || b.maxs.z < lo.z || b.mins.z > hi.z { continue; }
            let inside = b.maxs.x >= mins.x && b.mins.x <= maxs.x && b.maxs.y >= mins.y && b.mins.y <= maxs.y && b.maxs.z >= mins.z && b.mins.z <= maxs.z;
            println!("  {} #{} {} phy={} tris={} origin={:?} bounds={:?}..{:?}",
                if inside { "IN " } else { "near" }, p.index, p.model, p.from_phy, p.tris.len(), tup(p.origin), tup(b.mins), tup(b.maxs));
            n += 1;
        }
        if n == 0 { println!("  (no solid props within {pad}u)"); }
    }
}

fn tup(v: Vec3) -> (i32, i32, i32) { (v.x as i32, v.y as i32, v.z as i32) }
fn pv(s: &str) -> Option<Vec3> {
    let mut it = s.split_whitespace().map(|t| t.parse::<f32>().ok());
    Some(Vec3::new(it.next()??, it.next()??, it.next()??))
}
