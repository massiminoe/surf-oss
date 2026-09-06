//! The replay viewer's scrub bar: it is drawn along the bottom edge, and the
//! playhead sits where the progress says. Read back from the real renderer,
//! because the failure being guarded — the bar silently not drawing, or the
//! thumb parked at one end whatever the progress — is invisible in the layout
//! code.

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::{HudState, HudTimerPhase, Offscreen, ReplayHud, ViewParams};

const W: u32 = 640;
const H: u32 = 400;

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

fn watching(progress: f32) -> HudState {
    HudState {
        speed: 1500.0,
        time_secs: Some(20.0 * progress),
        timer_phase: HudTimerPhase::Running,
        replay: Some(ReplayHud {
            label: "KSF #1 test".into(),
            transport: "1x".into(),
            progress,
            total_secs: 20.0,
            split_marks: vec![0.5],
            chase: false,
        }),
        time: 3.0,
        ..HudState::default()
    }
}

/// Mean x of the strongly green pixels in the bottom 40 rows — the accent
/// playhead is the only accent there (the speed number sits at the top).
fn accent_x(px: &[u8]) -> Option<f32> {
    let (mut sx, mut n) = (0.0f32, 0usize);
    for y in (H - 40)..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let (r, g, b) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32);
            if g > r + 40.0 && g > b + 40.0 {
                sx += x as f32;
                n += 1;
            }
        }
    }
    (n > 0).then(|| sx / n as f32)
}

#[test]
fn the_scrub_bar_playhead_tracks_progress_along_the_bottom_edge() {
    let path = map_path("surf_summit");
    if !path.is_file() {
        eprintln!("summit absent; skipping");
        return;
    }
    let map = LoadedMap::load_path(&path).expect("summit");
    let mut off = Offscreen::new(&map, W, H).expect("offscreen");
    let eye = map.spawn_origin + Vec3::new(0.0, 0.0, 64.0);
    let angles = map.spawn_angles;

    let at = |off: &mut Offscreen, p: f32| {
        let px = off.capture_with_hud(eye, angles, ViewParams::default(), Some(watching(p)));
        accent_x(&px).unwrap_or_else(|| panic!("no playhead drawn at progress {p}"))
    };
    let x0 = at(&mut off, 0.05);
    let x5 = at(&mut off, 0.5);
    let x9 = at(&mut off, 0.95);
    assert!(
        x0 < x5 && x5 < x9,
        "playhead does not move with progress: {x0:.0} / {x5:.0} / {x9:.0}"
    );
    assert!(
        (x9 - x0) > W as f32 * 0.4,
        "bar spans only {:.0}px of a {W}px frame",
        x9 - x0
    );

    // And without a replay there is no playhead at all.
    let plain = off.capture_with_hud(
        eye,
        angles,
        ViewParams::default(),
        Some(HudState {
            speed: 1500.0,
            time_secs: Some(10.0),
            timer_phase: HudTimerPhase::Running,
            time: 3.0,
            ..HudState::default()
        }),
    );
    assert!(
        accent_x(&plain).is_none(),
        "accent drawn along the bottom with no replay"
    );
}
