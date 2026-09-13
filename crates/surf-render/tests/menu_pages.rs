//! The two menu backgrounds must actually behave differently.
//!
//! A shell page (title screen, map picker, leaderboard, loading) paints its own
//! procedural backdrop and the world behind it is not drawn at all — before
//! this the "menu background" was whatever map happened to be loaded, dimmed,
//! which on a cold start meant the graybox arena. The pause overlay is the
//! opposite: it must keep the world visible behind it, because you are still
//! standing in it.
//!
//! Both are asserted against a world-only capture of the same view, so a
//! regression that quietly swapped one behaviour for the other is caught.

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::{HudState, MenuPage, MenuPanel, MenuRow, Offscreen, ViewParams};

const W: u32 = 320;
const H: u32 = 200;

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

fn page(backdrop: bool) -> MenuPage {
    MenuPage {
        title: "SURF-OSS".into(),
        subtitle: "test".into(),
        panel: MenuPanel {
            rows: vec![MenuRow::item("Play", ""), MenuRow::item("Quit", "")],
            ..Default::default()
        },
        backdrop,
        ..Default::default()
    }
}

/// Mean RGB of a band across the bottom of the frame — below the panel on every
/// page, so it samples background and nothing else.
fn bottom_band(px: &[u8]) -> [f32; 3] {
    let mut acc = [0f32; 3];
    let mut n = 0f32;
    for y in (H - 30)..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            for c in 0..3 {
                acc[c] += px[i + c] as f32;
            }
            n += 1.0;
        }
    }
    [acc[0] / n, acc[1] / n, acc[2] / n]
}

/// Luminance spread across the same band. A flat fill scores ~0; a drawn scene
/// (grid lines, ramp silhouettes, vignette) does not.
fn bottom_band_spread(px: &[u8]) -> f32 {
    let mut vals: Vec<f32> = Vec::new();
    for y in (H - 30)..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            vals.push(0.299 * px[i] as f32 + 0.587 * px[i + 1] as f32 + 0.114 * px[i + 2] as f32);
        }
    }
    let mean = vals.iter().sum::<f32>() / vals.len() as f32;
    (vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / vals.len() as f32).sqrt()
}

#[test]
fn a_shell_page_replaces_the_world_and_the_pause_overlay_does_not() {
    let path = map_path("surf_summit");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let map = LoadedMap::load_path(&path).expect("load map");
    let Ok(mut off) = Offscreen::new(&map, W, H) else {
        eprintln!("skipping: no usable GPU adapter in this environment");
        return;
    };

    let eye = map.spawn_origin + Vec3::new(0.0, 0.0, 64.0);
    let angles = map.spawn_angles;

    let world = bottom_band(&off.capture(eye, angles, ViewParams::default()));
    let shell_px = off.capture_with_hud(
        eye,
        angles,
        ViewParams::default(),
        Some(HudState {
            page: Some(page(true)),
            time: 3.0,
            ..HudState::default()
        }),
    );
    let shell_spread = bottom_band_spread(&shell_px);
    let shell = bottom_band(&shell_px);
    let pause = bottom_band(&off.capture_with_hud(
        eye,
        angles,
        ViewParams::default(),
        Some(HudState {
            page: Some(page(false)),
            time: 3.0,
            ..HudState::default()
        }),
    ));

    let dist = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };

    // The shell backdrop is its own image: nothing of the world survives.
    assert!(
        dist(shell, world) > 40.0,
        "shell page background is still the world ({shell:?} vs {world:?})"
    );
    // …and it is a dark backdrop, not a flat fill of the clear colour.
    assert!(
        shell.iter().all(|c| *c < 90.0),
        "shell backdrop is too bright to be the dark blueprint ground: {shell:?}"
    );
    // It is a drawn scene, not a flat wash: the ground grid and the ramp
    // silhouettes give the band real structure.
    assert!(
        shell_spread > 3.0,
        "shell backdrop is flat ({shell_spread:.2} luma stddev) — the grid and \
         ramps are not being drawn"
    );
    // The accent lifts green well above where the base ink alone would put it
    // (ink_high is blue-dominant at g/b = 0.60).
    assert!(
        shell[1] / shell[2].max(1.0) > 0.70,
        "shell backdrop shows no accent contribution: {shell:?}"
    );

    // The pause overlay dims the world but keeps it: darker than the raw world,
    // and still nothing like the shell backdrop.
    let world_lum = world.iter().sum::<f32>();
    let pause_lum = pause.iter().sum::<f32>();
    assert!(
        pause_lum < world_lum * 0.75,
        "pause overlay barely dims the world ({pause_lum} vs {world_lum})"
    );
    assert!(
        pause_lum > world_lum * 0.10,
        "pause overlay blacks the world out instead of dimming it \
         ({pause_lum} vs {world_lum})"
    );
}
