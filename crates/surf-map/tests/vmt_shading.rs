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
fn cyberwave_additive_energy_ball_is_flagged_for_the_blended_pass() {
    let Some(map) = load("surf_cyberwave") else {
        return;
    };
    // `effects/emp_ball1` is `$additive`: a blue glow painted on black, drawn
    // over the ring above the start. Drawn opaque it is a black disc across
    // the skyline. The renderer now composites additive layers as `dst += src`
    // in a second pass, so the layer must be flagged for it and its texels
    // left as painted — the old alpha keying would punch holes in the glow.
    let layer = layer_for(&map.materials, "effects/emp_ball1").expect("emp_ball1 resolved");
    assert!(
        map.materials.additive_layers[layer as usize],
        "emp_ball1 is not flagged additive — it will draw as an opaque black disc"
    );
    let n = (map.materials.layer_size * map.materials.layer_size) as usize;
    let start = layer as usize * n * 4;
    let px = &map.materials.rgba[start..start + n * 4];
    let keyed = px.chunks_exact(4).filter(|c| c[3] < 255).count();
    assert_eq!(keyed, 0, "{keyed} texels have a keyed alpha; the blend pass needs them opaque");
    let non_additive = map.materials.additive_layers.iter().filter(|a| !**a).count();
    assert!(non_additive > 1, "the world itself must not be flagged additive");
}

#[test]
fn demise_flat_colour_sky_is_not_blown_out_by_the_shader_scale() {
    let Some(map) = load("surf_demise") else {
        return;
    };
    // `elly/fakeskies/sky_demise_05_red` is a `$color` with no basetexture at
    // all — the map's fake red sky. The fragment shader draws textured surfaces
    // as `albedo * light * 2`, so a layer stored at face value clips to pure
    // white and the sky reads as a flat glare. Stored halved, the brightest
    // channel comes back through the x2 at just under 1.0, which is the colour
    // the author actually asked for.
    //
    // (This guard used to run on hourglass' `{220 220 220}` overcast sky; that
    // map left the corpus on 2026-09-06. demise is the stronger fixture anyway:
    // its red channel sits exactly at the clipping boundary, so an unhalved
    // layer cannot sneak past.)
    let layer = layer_for(&map.materials, "elly/fakeskies/sky_demise_05_red")
        .expect("sky_demise_05_red resolved");
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
    }
    // The red channel is the one that would clip: halved it doubles back to
    // ~1.0, unhalved it would have been driven to 2.0 and flattened to white.
    let out = doubled(rgb[0]);
    assert!(
        (0.85..=1.0).contains(&out),
        "after the shader's x2 the red channel reads {out:.2} linear; a $color \
         sky stored halved should land just under 1.0"
    );
}
