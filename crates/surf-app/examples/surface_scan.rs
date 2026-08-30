//! Scan the collision surface across a ramp and report what the player would
//! actually ride, cell by cell: height, normal, and which class of solid owns
//! it (world brush / displacement / static prop).
//!
//! A surf ramp should be one smooth plane. Anything that breaks that — a step
//! in height, a normal that swings away from its neighbours, a different solid
//! class in the middle of the sheet — is a thing the player can catch on, and
//! it is invisible to any test that only looks at what is *drawn*: a clipped
//! rock is solid without being visible, and a decorative rock is visible
//! without being solid.
//!
//!   cargo run -p surf-app --example surface_scan --release -- surf_boreas 3

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::Vec3;
use surf_core::movement::Hull;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

/// Grid pitch across the ramp footprint, in units.
const STEP: f32 = 16.0;
/// How far out to either side of the recorded line to scan.
const HALF_WIDTH: f32 = 400.0;
/// A height difference between neighbouring cells this large is a step, not a slope.
const STEP_HEIGHT: f32 = 24.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_boreas".into());
    let want: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);

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

    // Re-derive the ramp segments the same way ramp_tour does.
    let on_ramp: Vec<bool> = frames
        .iter()
        .map(|f| surf_core::is_on_surf_ramp(&map.world, f.origin, &hull))
        .collect();
    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < on_ramp.len() {
        if !on_ramp[i] {
            i += 1;
            continue;
        }
        let (start, mut end, mut gap, mut j) = (i, i, 0, i);
        while j < on_ramp.len() {
            if on_ramp[j] {
                end = j;
                gap = 0;
            } else {
                gap += 1;
                if gap > 25 {
                    break;
                }
            }
            j += 1;
        }
        if end - start >= 20 {
            segs.push((start, end));
        }
        i = j;
    }
    let Some(&(start, end)) = segs.get(want - 1) else {
        eprintln!("no ramp {want} (found {})", segs.len());
        return;
    };

    let brush_n = map.world.brushes.len();
    let prop_start = brush_n + map.prop_tri_start;
    let class = |idx: usize| {
        if idx < brush_n {
            "brush"
        } else if idx < prop_start {
            "disp"
        } else {
            "prop"
        }
    };

    println!(
        "{map_name} ramp {want}: ticks {start}..{end}, scanning +/-{HALF_WIDTH:.0}u around the line at {STEP:.0}u"
    );

    // Sample perpendicular to travel at every few ticks along the segment.
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut steps: Vec<(f32, f32, f32, f32, &str, &str)> = Vec::new();
    let mut nz_min = 1.0f32;
    let mut cells = 0usize;

    let mut tick = start;
    while tick <= end {
        let f = &frames[tick];
        let v = f.velocity;
        let len = v.length_2d().max(1.0);
        // Unit vector to the player's right, in the ground plane.
        let right = Vec3::new(v.y / len, -v.x / len, 0.0);

        let mut row: Vec<Option<(f32, Vec3, &str)>> = Vec::new();
        let mut off = -HALF_WIDTH;
        while off <= HALF_WIDTH {
            let base = f.origin + right * off;
            let top = base + Vec3::new(0.0, 0.0, 300.0);
            let bot = base - Vec3::new(0.0, 0.0, 500.0);
            let tr = trace_box(&map.world, top, bot, hull.mins, hull.maxs);
            let cell = tr.hit.as_ref().map(|h| {
                let z = top.z + (bot.z - top.z) * tr.fraction;
                (z, h.normal, class(h.brush_index))
            });
            if let Some((_, n, c)) = cell {
                *counts.entry(c).or_insert(0) += 1;
                nz_min = nz_min.min(n.z);
                cells += 1;
            }
            row.push(cell);
            off += STEP;
        }

        // A step between neighbouring columns is something to catch on.
        for w in 0..row.len().saturating_sub(1) {
            if let (Some((z0, _, c0)), Some((z1, _, c1))) = (row[w], row[w + 1]) {
                if (z1 - z0).abs() > STEP_HEIGHT {
                    let off = -HALF_WIDTH + w as f32 * STEP;
                    steps.push((tick as f32, off, z0, z1, c0, c1));
                }
            }
        }
        tick += 3;
    }

    println!("  {cells} surface cells: {counts:?}, min normal.z = {nz_min:.3}");
    println!(
        "  {} height steps > {STEP_HEIGHT:.0}u between adjacent cells",
        steps.len()
    );
    for (t, off, z0, z1, c0, c1) in steps.iter().take(20) {
        println!(
            "    tick {t:.0} offset {off:+.0}u: {z0:.0} ({c0}) -> {z1:.0} ({c1})   step {:+.0}u",
            z1 - z0
        );
    }
}
