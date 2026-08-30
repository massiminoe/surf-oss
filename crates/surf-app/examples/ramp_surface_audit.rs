//! Which surface class is costing speed? Replays a KSF ghost one tick at a time
//! and attributes every contact to a world brush, a displacement triangle, or a
//! static-prop triangle, then reports speed retention per class.
//!
//! Prop tris are built from the model's *render* mesh, not its `.phy` collision
//! hull, so they carry every decorative bump the artist put on the surface.
//! If prop contacts retain visibly less speed than brush/displacement contacts,
//! that is the cause, not the physics.
//!
//!   cargo run -p surf-app --example ramp_surface_audit --release -- surf_boreas

use std::path::PathBuf;

use surf_app::replay::{ksf_imported_dir, Replay};
use surf_core::math::Angle;
use surf_core::movement::{Hull, MoveVars, PlayerState};
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

#[derive(Default, Clone)]
struct Class {
    ticks: usize,
    kept: f64,
    worst: f32,
    worst_tick: usize,
    snags: usize,
    hard: Vec<(usize, f32, f32)>,
}

impl Class {
    fn add(&mut self, before: f32, after: f32, tick: usize) {
        self.ticks += 1;
        let ratio = if before > 1.0 { after / before } else { 1.0 };
        self.kept += ratio as f64;
        if before > 500.0 && ratio < 0.9 {
            self.snags += 1;
        }
        if self.ticks == 1 || ratio < self.worst {
            self.worst = ratio;
            self.worst_tick = tick;
        }
        // A hard snag: fast, then almost stopped. This is the class that reads
        // as "the ramp ate me", distinct from ordinary strafe losses.
        if before > 800.0 && ratio < 0.5 {
            self.hard.push((tick, before, after));
        }
    }
    fn report(&self, name: &str) {
        if self.ticks == 0 {
            println!("  {name:<12} (no contacts)");
            return;
        }
        println!(
            "  {name:<12} ticks={:<6} mean_speed_kept={:.4}  snags(<0.90)={:<5} worst={:.3} @tick {}",
            self.ticks,
            self.kept / self.ticks as f64,
            self.snags,
            self.worst,
            self.worst_tick
        );
        for (t, b, a) in self.hard.iter().take(12) {
            println!("      HARD SNAG tick {t}: {b:.0} -> {a:.0} u/s");
        }
    }
}

fn main() {
    let map_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "surf_boreas".to_string());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map_path = root.join(format!("assets/maps/{map_name}.bsp"));
    let map = LoadedMap::load_path(&map_path).expect("load map");

    let dir = ksf_imported_dir(&map_name);
    let mut ghosts: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no ghosts in {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("osxr"))
        .collect();
    ghosts.sort();

    let brush_n = map.world.brushes.len();
    let prop_start = brush_n + map.prop_tri_start;
    println!(
        "{map_name}: brushes={brush_n} disp_tris={} prop_tris={}",
        map.prop_tri_start,
        map.world.tris.len() - map.prop_tri_start
    );

    let vars = MoveVars::momentum_surf();
    let hull = Hull::css_stand();

    for g in &ghosts {
        let Ok(replay) = Replay::load(g) else {
            continue;
        };
        let frames = replay.frames_for_resim();
        let mut brush = Class::default();
        let mut disp = Class::default();
        let mut prop = Class::default();
        let mut airborne = 0usize;

        for (i, f) in frames.iter().enumerate() {
            let mut p = PlayerState {
                origin: f.origin,
                velocity: f.velocity,
                viewangles: Angle::new(0.0, f.yaw, 0.0),
                grounded: f.grounded,
                ..Default::default()
            };
            let before = p.velocity.length_2d();
            if before < 200.0 {
                continue;
            }
            // What is this tick's move about to touch?
            let end = p.origin + p.velocity * vars.tick_interval;
            let tr = trace_box(&map.world, p.origin, end, hull.mins, hull.maxs);
            if tr.fraction >= 1.0 || tr.hit.is_none() {
                airborne += 1;
                continue;
            }
            let idx = tr.hit.as_ref().unwrap().brush_index;

            p.basevelocity = map.touch_push(p.origin);
            p.gravity_scale = map.touch_gravity(p.origin);
            let after = surf_core::tick(&map.world, &p, &f.to_usercmd(), &vars)
                .velocity
                .length_2d();

            if idx < brush_n {
                brush.add(before, after, i);
            } else if idx < prop_start {
                disp.add(before, after, i);
            } else {
                prop.add(before, after, i);
            }
        }

        println!(
            "\n{} (free-air ticks={airborne})",
            g.file_name().unwrap().to_string_lossy()
        );
        brush.report("brush");
        disp.report("displacement");
        prop.report("prop-mesh");
    }
}
