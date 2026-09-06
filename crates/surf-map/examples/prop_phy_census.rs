//! Per-map census of solid static props split by whether their collision came
//! from a real `.phy` hull or from the render-mesh fallback.
//!
//!   cargo run -p surf-map --example prop_phy_census --release -- [--all | <map.bsp>…]
//!   VERBOSE=1 …   also lists each fallback model and its instance count
//!
//! Source only ever collides a static prop against its `.phy`, so the `render`
//! column should read 0 everywhere. It is kept as a tool because the column is
//! the fastest way to see what `MX_SURF_PROP_RENDER_COLLISION=1` puts back, and
//! what a newly imported map would have been colliding under the old rule.
use std::collections::BTreeMap;
use surf_map::LoadedMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<String> = if args.first().map(|s| s == "--all").unwrap_or(false) {
        let mut v: Vec<String> = std::fs::read_dir("assets/maps")
            .unwrap()
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "bsp").then(|| p.to_string_lossy().into_owned())
            })
            .collect();
        v.sort();
        v
    } else {
        args
    };
    println!("{:<20} {:>7} {:>7} {:>9} {:>9}", "map", "phy", "render", "phy_tris", "rend_tris");
    for path in &paths {
        let Ok(map) = LoadedMap::load_path(path) else { continue };
        let (mut np, mut nr, mut tp, mut tr) = (0, 0, 0, 0);
        let mut models: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for p in &map.props {
            if !p.solid { continue; }
            if p.from_phy { np += 1; tp += p.tris.len(); }
            else {
                nr += 1;
                tr += p.tris.len();
                let e = models.entry(p.model.as_str()).or_default();
                e.0 += 1;
                e.1 = p.tris.len();
            }
        }
        println!("{:<20} {np:>7} {nr:>7} {tp:>9} {tr:>9}", map.name);
        if std::env::var_os("VERBOSE").is_some() {
            for (m, (n, t)) in &models {
                println!("      render-mesh x{n:<4} {t:>6} tris  {m}");
            }
        }
    }
}
