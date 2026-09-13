//! `$translucent` materials blend; they are not a 0.5 cutout.
//!
//! aquaflow marks its map transitions with `portalcard` — a big soft fade card
//! hung across the route (Source dissolves it with a PlayerProximity proxy).
//! Alpha-tested at 0.5 it became a flat light-grey sheet with a hard edge
//! where the gradient crossed the threshold, standing across the tunnel mouth.
//! Max: "it renders as a solid colour… they shouldn't be blocking our vision.
//! I feel they were probably meant to be some kind of glow."
//!
//! Verified to fail with `SURF_OSS_NO_VMT_SHADING=translucent` (which draws
//! translucent materials through the opaque cutout again): the card's edge
//! becomes a 4.1-luma step between neighbouring columns instead of a 1.3 ramp.

use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;
use surf_render::{Offscreen, ViewParams};

const W: u32 = 1200;
const H: u32 = 750;

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

fn layer_of(map: &LoadedMap, material: &str) -> usize {
    map.materials
        .layer_of
        .iter()
        .find(|(name, _)| name == material)
        .map(|(_, layer)| *layer as usize)
        .unwrap_or_else(|| panic!("{material} did not resolve to a layer"))
}

/// Mean luma of each column in `x0..x1`, over the rows `y0..y1`.
fn column_luma(px: &[u8], x0: u32, x1: u32, y0: u32, y1: u32) -> Vec<f32> {
    (x0..x1)
        .map(|x| {
            let mut sum = 0.0f32;
            for y in y0..y1 {
                let i = ((y * W + x) * 4) as usize;
                sum +=
                    0.2126 * px[i] as f32 + 0.7152 * px[i + 1] as f32 + 0.0722 * px[i + 2] as f32;
            }
            sum / (y1 - y0) as f32
        })
        .collect()
}

#[test]
fn aquaflow_transition_card_fades_instead_of_ending_in_a_hard_edge() {
    let path = map_path("surf_aquaflow");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let map = LoadedMap::load_path(&path).expect("load map");

    let Ok(mut off) = Offscreen::new(&map, W, H) else {
        eprintln!("skipping: no usable GPU adapter in this environment");
        return;
    };
    // The KSF WR's own eye at tick 2050, on approach to the second transition:
    // the card hangs across the left half of the frame against open water.
    let px = off.capture(
        Vec3::new(10047.0, -780.0, -5028.0),
        Angle {
            pitch: -2.0,
            yaw: -111.0,
            roll: 0.0,
        },
        ViewParams::default(),
    );

    // A blended gradient changes by a little over many columns; a cutout ends
    // in one step. Measured across the card's right edge on this view: worst
    // column-to-column step 1.3 blended, 4.1 as a cutout.
    let profile = column_luma(&px, 380, 500, 40, 300);
    let (worst, at) = profile
        .windows(2)
        .enumerate()
        .map(|(i, w)| ((w[1] - w[0]).abs(), 380 + i as u32))
        .fold((0.0f32, 0u32), |acc, x| if x.0 > acc.0 { x } else { acc });
    assert!(
        worst < 2.5,
        "the card's edge steps {worst:.1} luma between columns {at} and {} — \
         that is a cutout boundary, not a blended fade",
        at + 1
    );

    // Corroborating detail, checked after the picture so the picture is what
    // fails when the pass regresses.
    let card = layer_of(&map, "portalcard");
    assert!(
        map.materials.translucent_layers[card],
        "portalcard is $translucent and must be flagged for the blended pass"
    );
    assert!(
        !map.materials.additive_layers[card],
        "portalcard blends over, it does not add"
    );
}
