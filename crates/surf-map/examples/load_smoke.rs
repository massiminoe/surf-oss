//! Batch-load BSPs and report spawn/teleport/brush stats.
//!
//!   cargo run -p surf-map --example load_smoke --release -- assets/maps/surf_boreas.bsp ...
//!   cargo run -p surf-map --example load_smoke --release -- --all-new

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<PathBuf> = if args.iter().any(|a| a == "--all-new") {
        let dir = Path::new("assets/maps");
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .expect("assets/maps")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|x| x.to_str()) == Some("bsp")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s != "surf_summit")
                        .unwrap_or(false)
            })
            .collect();
        v.sort();
        v
    } else if args.is_empty() {
        eprintln!("usage: load_smoke <map.bsp>... | --all-new");
        std::process::exit(2);
    } else {
        args.into_iter().map(PathBuf::from).collect()
    };

    let mut ok = 0usize;
    let mut fail = 0usize;
    for path in &paths {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let t = Instant::now();
        match surf_map::LoadedMap::load_path(path) {
            Ok(map) => {
                println!(
                    "OK  {name:22} {:.2?}  brushes={:<6} tris={:<7} teleports={:<4} mats={:<4} spawn=({:.0},{:.0},{:.0})",
                    t.elapsed(),
                    map.world.brushes.len(),
                    map.world.tris.len(),
                    map.teleports.len(),
                    map.materials.textured_count,
                    map.spawn_origin.x,
                    map.spawn_origin.y,
                    map.spawn_origin.z,
                );
                ok += 1;
            }
            Err(e) => {
                println!("ERR {name:22} {:.2?}  {e}", t.elapsed());
                fail += 1;
            }
        }
    }
    println!("done: {ok} ok, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
