//! `$additive` materials are composited `dst += src`, not drawn opaque.
//!
//! aquaflow's ramps carry a `ramp_glow_volumetric` skirt — an additive soft
//! glow along the foot of every ramp. Drawn opaque (the only pass we had) it
//! was a pinkish slab standing at the bottom of each ramp; Max: "they look
//! kinda like bumper bars". With the blended pass the skirt only brightens
//! what is behind it. Verified to fail with `SURF_OSS_NO_VMT_SHADING=additive`
//! (which draws additive materials opaque again): the glow region comes out
//! darker than the sand around it.

use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;
use surf_render::{Offscreen, ViewParams};

fn map_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("assets/maps/{name}.bsp"))
}

fn mean_luma(px: &[u8], w: u32, x0: u32, x1: u32, y0: u32, y1: u32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            sum += 0.2126 * px[i] as f32 + 0.7152 * px[i + 1] as f32 + 0.0722 * px[i + 2] as f32;
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn aquaflow_ramp_glow_brightens_instead_of_standing_as_a_slab() {
    let path = map_path("surf_aquaflow");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let map = LoadedMap::load_path(&path).expect("load map");
    let glow = map
        .materials
        .layer_of
        .iter()
        .find(|(name, _)| name == "models/ramps/ramp_glow_volumetric")
        .map(|(_, layer)| *layer)
        .expect("ramp_glow_volumetric resolved");
    assert!(
        map.materials.additive_layers[glow as usize],
        "ramp_glow_volumetric is $additive and must be flagged for the blended pass"
    );

    let (w, h) = (640u32, 400u32);
    let Ok(mut off) = Offscreen::new(&map, w, h) else {
        eprintln!("skipping: no usable GPU adapter in this environment");
        return;
    };
    // Looking at the foot of ramp 1 from beside its exit: the glow skirt sits
    // left of the ramp over open sand.
    let px = off.capture(
        Vec3::new(-10000.0, -4400.0, 6000.0),
        Angle {
            pitch: 8.0,
            yaw: 100.0,
            roll: 0.0,
        },
        ViewParams::default(),
    );
    let glow_region = mean_luma(&px, w, 80, 125, 215, 245);
    let sand = mean_luma(&px, w, 200, 400, 300, 350);
    assert!(
        glow_region >= sand,
        "the glow region (luma {glow_region:.0}) is darker than the sand beside it \
         ({sand:.0}) — an additive skirt can only brighten; this is the opaque slab"
    );
}
