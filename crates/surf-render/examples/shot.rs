//! Headless screenshot of a map.
//!
//!   cargo run -p surf-render --example shot --release -- \
//!       assets/maps/surf_boreas.bsp out.png [--at x,y,z] [--look yaw,pitch]
//!       [--size WxH] [--spawn-eye] [--orbit N] [--back D] [--up U]
//!
//! Default camera: the map's gameplay spawn, eye height 64, looking at the
//! spawn from behind and above so the start area is framed.

use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;
use surf_render::{Offscreen, ViewParams};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: shot <map.bsp> <out.png> [--at x,y,z] [--look yaw,pitch] [--size WxH] [--spawn-eye] [--orbit N] [--back D] [--up U]");
        std::process::exit(2);
    }
    let map_path = &args[0];
    let out = &args[1];

    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has = |name: &str| args.iter().any(|a| a == name);

    let (w, h) = flag("--size")
        .and_then(|s| {
            let (a, b) = s.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((1280u32, 720u32));

    let map = LoadedMap::load_path(map_path).expect("load map");
    println!(
        "{}: mesh {} tris, coll {} tris, spawn ({:.0},{:.0},{:.0})",
        map.name,
        map.mesh.tris.len(),
        map.world.tris.len(),
        map.spawn_origin.x,
        map.spawn_origin.y,
        map.spawn_origin.z
    );

    let back: f32 = flag("--back").and_then(|s| s.parse().ok()).unwrap_or(320.0);
    let up: f32 = flag("--up").and_then(|s| s.parse().ok()).unwrap_or(140.0);
    let orbit: usize = flag("--orbit").and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut off = Offscreen::new(&map, w, h).expect("offscreen");
    let view = ViewParams::default();

    let shot = |off: &mut Offscreen, eye: Vec3, ang: Angle, path: &str| {
        off.save_png(eye, ang, view, path).expect("save png");
        println!(
            "wrote {path}  eye=({:.0},{:.0},{:.0}) yaw={:.0} pitch={:.0}",
            eye.x, eye.y, eye.z, ang.yaw, ang.pitch
        );
    };

    if let Some(n) = flag("--bench") {
        let n: u32 = n.parse().unwrap_or(120);
        let target = map.spawn_origin + Vec3::new(0.0, 0.0, 40.0);
        let yaw = map.spawn_angles.yaw + 180.0;
        let r = yaw.to_radians();
        let eye = target + Vec3::new(-r.cos() * back, -r.sin() * back, up);
        let ang = Angle {
            yaw,
            pitch: (up / back).atan().to_degrees(),
            roll: 0.0,
        };
        let ms = off.bench(eye, ang, view, n);
        println!("bench: {ms:.2} ms/frame over {n} frames at {w}x{h}");
        return;
    }

    if orbit > 0 {
        // Ring of views around the spawn — the fastest way to see whether an
        // area is actually built or just missing geometry on one side.
        let target = map.spawn_origin + Vec3::new(0.0, 0.0, 40.0);
        let stem = out.strip_suffix(".png").unwrap_or(out);
        for i in 0..orbit {
            let yaw = 360.0 * i as f32 / orbit as f32;
            let r = yaw.to_radians();
            let eye = target + Vec3::new(-r.cos() * back, -r.sin() * back, up);
            let pitch = (up / back).atan().to_degrees();
            shot(
                &mut off,
                eye,
                Angle {
                    yaw,
                    pitch,
                    roll: 0.0,
                },
                &format!("{stem}_{i}.png"),
            );
        }
        return;
    }

    let (eye, ang) = if let Some(at) = flag("--at") {
        let p: Vec<f32> = at
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let eye = Vec3::new(p[0], p[1], p[2]);
        let la: Vec<f32> = flag("--look")
            .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![0.0, 0.0]);
        (
            eye,
            Angle {
                yaw: la[0],
                pitch: *la.get(1).unwrap_or(&0.0),
                roll: 0.0,
            },
        )
    } else if has("--spawn-eye") {
        (
            map.spawn_origin + Vec3::new(0.0, 0.0, 64.0),
            map.spawn_angles,
        )
    } else {
        let target = map.spawn_origin + Vec3::new(0.0, 0.0, 40.0);
        let yaw = map.spawn_angles.yaw + 180.0;
        let r = yaw.to_radians();
        let eye = target + Vec3::new(-r.cos() * back, -r.sin() * back, up);
        (
            eye,
            Angle {
                yaw,
                pitch: (up / back).atan().to_degrees(),
                roll: 0.0,
            },
        )
    };

    shot(&mut off, eye, ang, out);
}
