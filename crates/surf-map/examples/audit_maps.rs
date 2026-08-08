//! Audit gameplay spawn vs zone start AABB, sky, and material coverage.
//!
//!   cargo run -p surf-map --example audit_maps --release -- --all-new

use std::path::{Path, PathBuf};

use serde::Deserialize;
use surf_core::math::Vec3;
use surf_core::Aabb;

#[derive(Deserialize)]
struct ZonesFile {
    tracks: Tracks,
}

#[derive(Deserialize)]
struct Tracks {
    main: Track,
}

#[derive(Deserialize)]
struct Track {
    start: ZoneAabb,
}

#[derive(Deserialize)]
struct ZoneAabb {
    mins: [f32; 3],
    maxs: [f32; 3],
}

impl ZoneAabb {
    fn contains_spawn(&self, o: Vec3) -> bool {
        // Soft: origin inside start AABB, or CS:S standing hull overlaps it.
        let zone = Aabb::from_mins_maxs(
            Vec3::new(self.mins[0], self.mins[1], self.mins[2]),
            Vec3::new(self.maxs[0], self.maxs[1], self.maxs[2]),
        );
        if zone.contains_point(o) {
            return true;
        }
        let hull_mins = o + Vec3::new(-16.0, -16.0, 0.0);
        let hull_maxs = o + Vec3::new(16.0, 16.0, 72.0);
        hull_mins.x <= zone.maxs.x
            && hull_maxs.x >= zone.mins.x
            && hull_mins.y <= zone.maxs.y
            && hull_maxs.y >= zone.mins.y
            && hull_mins.z <= zone.maxs.z
            && hull_maxs.z >= zone.mins.z
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_miss = args.iter().any(|a| a == "--miss");
    let paths: Vec<PathBuf> = if args.iter().any(|a| a == "--all-new") {
        let dir = Path::new("assets/maps");
        let skip = [
            "surf_summit",
            "surf_utopia_njv",
            "surf_utopia_v3",
            "surf_kitsune",
            "surf_beginner",
            "surf_mesa",
        ];
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .expect("assets/maps")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|x| x.to_str()) == Some("bsp")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| !skip.contains(&s))
                        .unwrap_or(false)
            })
            .collect();
        v.sort();
        v
    } else if args.is_empty() {
        eprintln!("usage: audit_maps <map.bsp>... | --all-new");
        std::process::exit(2);
    } else {
        args.into_iter().map(PathBuf::from).collect()
    };

    let mut bad_spawn = 0usize;
    let mut bad_sky = 0usize;

    println!(
        "{:<22} {:>7} {:>5}/{:<4} {:>5} {:>4} {:<8} spawn",
        "map", "mesh", "ok", "miss", "lm", "sky", "inStart"
    );

    for path in &paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let map = match surf_map::LoadedMap::load_path(path) {
            Ok(m) => m,
            Err(e) => {
                println!("{stem:<22} ERR {e}");
                continue;
            }
        };

        let zones_path = PathBuf::from("assets/zones").join(format!("{stem}.json"));
        let in_start = if zones_path.exists() {
            match std::fs::read_to_string(&zones_path)
                .ok()
                .and_then(|s| serde_json::from_str::<ZonesFile>(&s).ok())
            {
                Some(z) => z.tracks.main.start.contains_spawn(map.spawn_origin),
                None => false,
            }
        } else {
            false
        };

        let sky = if map.skybox.is_empty() { "MISS" } else { "ok" };
        let mesh_n = map.mesh.tris.len();
        let mats = map.materials.textured_count;
        let miss = map.materials.missing_count;
        let mark = if in_start { "YES" } else { "NO" };
        if !in_start {
            bad_spawn += 1;
        }
        if map.skybox.is_empty() {
            bad_sky += 1;
        }

        println!(
            "{stem:<22} {mesh_n:>7} {mats:>5}/{miss:<4} {:>4}x{:<4} {sky:>4} {mark:<8} ({:>7.0},{:>7.0},{:>7.0})",
            map.lightmaps.width,
            map.lightmaps.height,
            map.spawn_origin.x,
            map.spawn_origin.y,
            map.spawn_origin.z,
        );
        if miss > 0 && show_miss {
            for n in &map.materials.missing_names {
                println!("  MISS {n}");
            }
            if miss as usize > map.materials.missing_names.len() {
                println!(
                    "  … {} more",
                    miss as usize - map.materials.missing_names.len()
                );
            }
        }
    }

    println!("summary: spawn_out={bad_spawn} sky_miss={bad_sky}");
    if bad_spawn > 0 {
        std::process::exit(1);
    }
}
