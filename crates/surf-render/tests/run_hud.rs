//! The in-run timer block: where it sits, and that its two relative readings
//! are coloured independently.
//!
//! The block moved from the bottom-left corner to the lower centre, and it now
//! carries a checkpoint split *and* a speed delta side by side. Those two can
//! disagree — you can be up on the clock and down on speed — so a single
//! shared colour would be a lie. This drives the real renderer through
//! `Offscreen` and reads the pixels back, because the failure being guarded
//! against (one reading silently taking the other's colour, or one of them
//! quietly not drawing at all) is invisible in the layout code.

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::{HudState, HudTimerPhase, Offscreen, ViewParams};

const W: u32 = 640;
const H: u32 = 400;

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

fn running(cp_delta: f32, speed_delta: f32) -> HudState {
    HudState {
        speed: 1800.0,
        time_secs: Some(31.42),
        timer_phase: HudTimerPhase::Running,
        cp_label: Some("CP3".into()),
        cp_delta_secs: Some(cp_delta),
        ghost_speed_delta: Some(speed_delta),
        time: 3.0,
        ..HudState::default()
    }
}

/// Mean x of the strongly green / strongly red pixels in the bottom third,
/// plus how many of each there were. The speed number is also tinted, so only
/// the lower part of the frame is sampled.
fn green_and_red(px: &[u8]) -> ((f32, usize), (f32, usize)) {
    let (mut gx, mut gn, mut rx, mut rn) = (0.0f32, 0usize, 0.0f32, 0usize);
    for y in (H * 3 / 5)..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let (r, g, b) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32);
            if g > r + 40.0 && g > b + 40.0 {
                gx += x as f32;
                gn += 1;
            } else if r > g + 40.0 && r > b + 20.0 {
                rx += x as f32;
                rn += 1;
            }
        }
    }
    (
        (if gn > 0 { gx / gn as f32 } else { 0.0 }, gn),
        (if rn > 0 { rx / rn as f32 } else { 0.0 }, rn),
    )
}

#[test]
fn the_split_and_the_speed_delta_are_coloured_independently() {
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

    // Ahead on the clock, slower than the ghost: green on the left half of the
    // row, red on the right.
    let px = off.capture_with_hud(eye, angles, ViewParams::default(), Some(running(-0.5, -120.0)));
    let ((gx, gn), (rx, rn)) = green_and_red(&px);
    assert!(
        gn > 20 && rn > 20,
        "expected both an ahead (green) and a behind (red) reading, got \
         {gn} green / {rn} red pixels — one of them is missing or they share a colour"
    );
    assert!(
        gx < rx,
        "split is ahead and speed is behind, so green must sit left of red \
         (green x={gx:.0}, red x={rx:.0})"
    );

    // Swap both signs and the colours must swap with them.
    let px = off.capture_with_hud(eye, angles, ViewParams::default(), Some(running(0.5, 120.0)));
    let ((gx, gn), (rx, rn)) = green_and_red(&px);
    assert!(
        gn > 20 && rn > 20,
        "with the signs reversed, got {gn} green / {rn} red pixels"
    );
    assert!(
        rx < gx,
        "split is behind and speed is ahead, so red must sit left of green \
         (red x={rx:.0}, green x={gx:.0})"
    );
}

#[test]
fn the_timer_block_sits_in_the_lower_centre_not_the_bottom_left() {
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

    let world = off.capture(eye, angles, ViewParams::default());
    let hud = off.capture_with_hud(eye, angles, ViewParams::default(), Some(running(-0.5, 120.0)));

    // Mean absolute difference over a rect, 0..255.
    let diff = |x0: u32, x1: u32, y0: u32, y1: u32| {
        let mut acc = 0.0f32;
        let mut n = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * W + x) * 4) as usize;
                for c in 0..3 {
                    acc += (world[i + c] as f32 - hud[i + c] as f32).abs();
                }
                n += 3.0;
            }
        }
        acc / n
    };

    let centre = diff(W / 3, W * 2 / 3, H * 3 / 5, H - 10);
    let bottom_left = diff(0, W / 4, H * 3 / 5, H - 10);
    assert!(
        centre > 8.0,
        "nothing was drawn in the lower centre (mean diff {centre:.2})"
    );
    assert!(
        bottom_left < 1.0,
        "the bottom-left corner is still being drawn into (mean diff \
         {bottom_left:.2}) — the timer block was supposed to move to the centre"
    );
}
