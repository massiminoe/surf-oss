//! The 2D skybox must actually reach the framebuffer.
//!
//! The skybox is a cube drawn from the *inside*, so the faces that must survive
//! are the ones whose winding reads as front-facing from the eye. The pipeline
//! culled `Face::Front`, which discarded all six on every map: the loader
//! decoded the sky correctly, the renderer built the texture array, and then
//! nothing was rasterised, leaving the main pass's clear colour showing through
//! as a flat blue "sky". It looked plausible enough to survive review — which is
//! why this asserts against the clear colour specifically rather than just
//! checking that some sky exists.

use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;
use surf_render::{Offscreen, ViewParams};

/// The main pass clear colour (pipeline.rs), converted to 8-bit sRGB. Any sky
/// pixel equal to this is a hole where the skybox should have been drawn.
const CLEAR_SRGB: [u8; 3] = [179, 206, 237];

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

#[test]
fn skybox_faces_are_not_culled_away() {
    let path = map_path("surf_cyberwave");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let map = LoadedMap::load_path(&path).expect("load map");
    assert!(
        !map.skybox.is_empty(),
        "surf_cyberwave ships its own 6-face sky; the loader must decode it"
    );

    let (w, h) = (320u32, 200u32);
    let Ok(mut off) = Offscreen::new(&map, w, h) else {
        eprintln!("skipping: no usable GPU adapter in this environment");
        return;
    };

    // Look up from the spawn: the top of the frame is sky in every direction.
    let eye = map.spawn_origin + Vec3::new(0.0, 0.0, 64.0);
    let px = off.capture(
        eye,
        Angle {
            pitch: -60.0,
            yaw: 90.0,
            roll: 0.0,
        },
        ViewParams::default(),
    );

    // Sample the top row band, where geometry cannot reach at this pitch.
    let mut clear = 0usize;
    let mut total = 0usize;
    for y in 0..8 {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let rgb = [px[i], px[i + 1], px[i + 2]];
            total += 1;
            // Allow a little slack for filtering, but the clear colour is flat.
            if rgb
                .iter()
                .zip(CLEAR_SRGB.iter())
                .all(|(a, b)| a.abs_diff(*b) <= 1)
            {
                clear += 1;
            }
        }
    }
    assert!(total > 0);
    let frac = clear as f32 / total as f32;
    assert!(
        frac < 0.10,
        "{:.0}% of the sky band is the render-pass clear colour — the skybox is \
         not being drawn (all six cube faces culled?)",
        frac * 100.0
    );
}
