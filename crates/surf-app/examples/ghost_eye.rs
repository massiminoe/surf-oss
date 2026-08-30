//! Render the map from a KSF ghost's own eye, at chosen ticks. This is the view
//! the player actually has, which is the only view a "that looks wrong"
//! complaint can be checked against — a free-camera screenshot can make
//! geometry look intersecting that never enters the player's frame, and vice
//! versa.
//!
//!   cargo run -p surf-app --example ghost_eye --release -- surf_boreas out/ 580 600 620

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;

/// CS:S standing eye height.
const EYE_Z: f32 = 64.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let map_name = args.next().unwrap_or_else(|| "surf_boreas".into());
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let ticks: Vec<usize> = args.filter_map(|s| s.parse().ok()).collect();
    std::fs::create_dir_all(&out_dir).expect("out dir");

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

    for t in ticks {
        let Some(f) = frames.get(t) else {
            eprintln!("tick {t} past end ({} frames)", frames.len());
            continue;
        };
        let eye = f.origin + Vec3::new(0.0, 0.0, EYE_Z);
        // Recorded pitch is not in the resim frame set, so aim along travel:
        // that is where a surfer is looking to within a few degrees.
        let v = f.velocity;
        let yaw = v.y.atan2(v.x).to_degrees();
        let pitch = (-v.z).atan2(v.length_2d()).to_degrees() * 0.5;
        let path = out_dir.join(format!("{map_name}_t{t}.png"));
        match surf_render::render_to_png(&map, eye, Angle::new(pitch, yaw, 0.0), 1200, 750, &path) {
            Ok(()) => {
                println!(
                "tick {t}: eye=({:.0},{:.0},{:.0}) yaw={yaw:.0} pitch={pitch:.0} speed={:.0} -> {}",
                eye.x, eye.y, eye.z, v.length_2d(), path.display()
            )
            }
            Err(e) => eprintln!("tick {t}: {e}"),
        }
    }
}
