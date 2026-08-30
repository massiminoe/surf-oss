//! `$color2` / `$color` / `$additive` must reach the atlas.
//!
//! cyberwave is the case that exposed all three: Source ships a plain white
//! `lights/white.vtf` and the map recolours it twelve times, so *every* neon
//! strip on the map resolved to the same white fallback layer and the whole
//! city rendered monochrome. The map's own materials are the specification
//! here — these assert against the values written in its VMTs, not against a
//! screenshot.

use std::collections::HashSet;

use surf_map::{LoadedMap, MaterialAtlas};

fn map_path(name: &str) -> String {
    format!(
        "{}/../../assets/maps/{name}.bsp",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn load(name: &str) -> Option<LoadedMap> {
    let path = map_path(name);
    if !std::path::Path::new(&path).exists() {
        eprintln!("skipping: {path} not present");
        return None;
    }
    Some(LoadedMap::load_path(&path).expect("load map"))
}

fn layer_for(atlas: &MaterialAtlas, material: &str) -> Option<u32> {
    atlas
        .layer_of
        .iter()
        .find(|(n, _)| n == material)
        .map(|(_, id)| *id)
}

/// Mean RGB of a layer, and the mean alpha.
fn layer_stats(atlas: &MaterialAtlas, layer: u32) -> ([f32; 3], f32) {
    let n = (atlas.layer_size * atlas.layer_size) as usize;
    let start = layer as usize * n * 4;
    let px = &atlas.rgba[start..start + n * 4];
    let mut sum = [0.0f64; 4];
    for c in px.chunks_exact(4) {
        for i in 0..4 {
            sum[i] += c[i] as f64;
        }
    }
    let m = |i: usize| (sum[i] / n as f64) as f32;
    ([m(0), m(1), m(2)], m(3))
}

#[test]
fn cyberwave_neon_strips_keep_their_six_colours() {
    let Some(map) = load("surf_cyberwave") else {
        return;
    };
    // Straight from the map's own VMTs: `$color2` over a `lights/white` base.
    // Each entry names the channels that must come out bright and the ones that
    // must stay dark, which is the part a dropped tint destroys.
    let expected: [(&str, &[usize], &[usize]); 6] = [
        ("models/cyberwave/neon_blue", &[2], &[0, 1]),
        ("models/cyberwave/neon_red", &[0], &[1, 2]),
        ("models/cyberwave/neon_green", &[1, 2], &[0]),
        ("models/cyberwave/neon_yellow", &[0, 1], &[2]),
        ("models/cyberwave/neon_orange", &[0], &[2]),
        ("models/cyberwave/neon_magenta", &[0, 2], &[1]),
    ];

    let mut layers = HashSet::new();
    for (name, bright, dark) in expected {
        let layer =
            layer_for(&map.materials, name).unwrap_or_else(|| panic!("{name} was never resolved"));
        assert_ne!(
            layer, 0,
            "{name} fell back to the white missing-texture layer — its \
             basetexture is stock `lights/white`, so `$color2` is the only \
             thing that carries its colour"
        );
        layers.insert(layer);
        let (rgb, _) = layer_stats(&map.materials, layer);
        let lo = bright.iter().map(|&i| rgb[i]).fold(f32::MAX, f32::min);
        let hi = dark.iter().map(|&i| rgb[i]).fold(f32::MIN, f32::max);
        assert!(
            lo > hi + 40.0,
            "{name} is {rgb:?}: channels {bright:?} should clearly beat {dark:?}"
        );
    }

    assert_eq!(
        layers.len(),
        6,
        "the six neon materials share one white VTF; without the tint in the \
         atlas dedupe key they collapse into one layer"
    );
}

#[test]
fn cyberwave_additive_energy_ball_is_not_an_opaque_disc() {
    let Some(map) = load("surf_cyberwave") else {
        return;
    };
    // `effects/emp_ball1` is `$additive`: a blue glow painted on black, drawn
    // over the ring above the start. Opaque, it is a black disc across the
    // skyline. Its peak luminance is only ~102/255, so this also pins that the
    // keying does not simply erase the sprite.
    let layer = layer_for(&map.materials, "effects/emp_ball1").expect("emp_ball1 resolved");
    let n = (map.materials.layer_size * map.materials.layer_size) as usize;
    let start = layer as usize * n * 4;
    let px = &map.materials.rgba[start..start + n * 4];
    let cut = px.chunks_exact(4).filter(|c| c[3] < 128).count() as f32 / n as f32;
    assert!(
        cut > 0.20,
        "only {:.1}% of the energy ball is cut out — an additive sprite drawn \
         opaque is a black hole in the sky",
        cut * 100.0
    );
    assert!(
        cut < 0.95,
        "{:.1}% cut out — the sprite has been erased, not keyed",
        cut * 100.0
    );
}

#[test]
fn hourglass_flat_colour_sky_is_not_blown_out_by_the_shader_scale() {
    let Some(map) = load("surf_hourglass") else {
        return;
    };
    // `masog/surf_time/unlitflatsky` is `$color {220 220 220}` with no
    // basetexture at all. The fragment shader draws textured surfaces as
    // `albedo * light * 2`, so a layer stored at face value clips to pure
    // white. Stored halved, it comes back as the overcast grey the author
    // asked for.
    let layer =
        layer_for(&map.materials, "masog/surf_time/unlitflatsky").expect("unlitflatsky resolved");
    assert_ne!(layer, 0, "a $color-only material still needs a real layer");
    let (rgb, _) = layer_stats(&map.materials, layer);
    let doubled = |c: f32| {
        let lin = if c / 255.0 <= 0.04045 {
            c / 255.0 / 12.92
        } else {
            ((c / 255.0 + 0.055) / 1.055).powf(2.4)
        };
        (lin * 2.0).min(1.0)
    };
    for c in rgb {
        assert!(
            c < 245.0,
            "the stored layer is already {c:.0}/255 — doubling it in the \
             shader can only clip"
        );
        let out = doubled(c);
        assert!(
            (out - 0.7).abs() < 0.15,
            "after the shader's x2 this reads {out:.2} linear; {{220 220 220}} \
             should land near 0.70"
        );
    }
}
