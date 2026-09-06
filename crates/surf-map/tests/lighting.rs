//! The CS:S lighting pass (2026-09-06): lightmaps on LZMA-compressed maps,
//! baked and ambient static-prop light, two-texture displacement blends and
//! texture mip chains. Each assertion is against the map's own data.

use surf_map::{LoadedMap, PropLighting, UNIT_LIGHT_BYTE};

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

/// Mean per-vertex light over a prop's drawn triangles.
fn mean_light(map: &LoadedMap, prop: &surf_map::PropInstance) -> [f32; 3] {
    let tris = &map.mesh.tris[prop.render_tris.clone()];
    let n = (tris.len() * 3).max(1) as f32;
    tris.iter()
        .flat_map(|t| t.light.iter())
        .fold([0.0f32; 3], |a, l| [a[0] + l[0] / n, a[1] + l[1] / n, a[2] + l[2] / n])
}

#[test]
fn boreas_lightmap_survives_its_lzma_compressed_lighting_lump() {
    // boreas (and aquaflow, botanica, demise, lovetunnel, tendies) compress
    // the lighting lump. Read raw, no face's luxels fit inside the lump, so
    // the map loaded with no lightmap at all and drew flat.
    let Some(map) = load("surf_boreas") else {
        return;
    };
    // boreas has 1351 faces with luxels (its terrain is displacement, and
    // displacement faces are lit through the same path).
    assert!(
        map.lightmaps.face_count >= 1_300,
        "only {} faces got luxels — the lighting lump was not decompressed",
        map.lightmaps.face_count
    );
    let px = map.lightmaps.rgba.chunks_exact(4).count();
    let non_unit = map
        .lightmaps
        .rgba
        .chunks_exact(4)
        .filter(|p| p[0] != UNIT_LIGHT_BYTE || p[1] != UNIT_LIGHT_BYTE || p[2] != UNIT_LIGHT_BYTE)
        .count();
    assert!(
        non_unit as f32 / px as f32 > 0.5,
        "{non_unit}/{px} atlas texels carry light; a flat atlas means no luxels were decoded"
    );
}

#[test]
fn summit_props_take_their_baked_vertex_light_in_bgr_order() {
    let Some(map) = load("surf_summit") else {
        return;
    };
    let baked = map
        .props
        .iter()
        .filter(|p| p.lighting == PropLighting::Baked)
        .count();
    assert!(baked > 500, "only {baked} summit props read their .vhv");
    // The torches are lit by their own flames: orange, so red must beat blue.
    // Read the colours as RGB they come out blue and the byte order is wrong.
    let torch = map
        .props
        .iter()
        .find(|p| p.model.contains("sum_torch") && p.lighting == PropLighting::Baked)
        .expect("a baked torch");
    let l = mean_light(&map, torch);
    assert!(
        l[0] > 2.0 * l[2] && l[0] > 0.3,
        "torch light {l:?} should be orange (baked as BGRA)"
    );
    // Props whose .vhv is empty (vrad wrote no colours) fall back to the
    // ambient cube instead of drawing flat.
    assert!(
        map.props.iter().any(|p| p.lighting == PropLighting::Ambient),
        "no summit prop fell back to ambient"
    );
}

#[test]
fn demise_multi_lod_trees_are_baked_not_ambient() {
    // The .vhv indexes vertices in strip-group upload order and the header's
    // count spans every LOD; matched against VVD ids, every multi-LOD model
    // (all of demise's forest) was rejected and lit from the cube only.
    let Some(map) = load("surf_demise") else {
        return;
    };
    let trees: Vec<_> = map
        .props
        .iter()
        .filter(|p| p.model.contains("tree_douglasfir") && !p.skybox)
        .collect();
    assert!(trees.len() > 100, "demise should have a forest, got {}", trees.len());
    let ambient = trees
        .iter()
        .filter(|p| p.lighting != PropLighting::Baked)
        .count();
    assert_eq!(ambient, 0, "{ambient} of {} trees are not baked", trees.len());
}

#[test]
fn aquaflow_props_without_baked_light_get_ambient_plus_sun() {
    // aquaflow ships no .vhv at all. Its props come from the leaf ambient
    // cube plus the compiled world lights; with only the cube (or the cube
    // read with a spurious /255) they were black against a lit floor.
    let Some(map) = load("surf_aquaflow") else {
        return;
    };
    let lit: Vec<_> = map
        .props
        .iter()
        .filter(|p| !p.skybox && !p.render_tris.is_empty())
        .collect();
    assert!(!lit.is_empty());
    assert!(
        lit.iter().all(|p| p.lighting == PropLighting::Ambient),
        "aquaflow has no baked prop light; every drawn prop should be ambient-lit"
    );
    let mut means: Vec<f32> = lit
        .iter()
        .map(|p| {
            let l = mean_light(&map, p);
            0.3 * l[0] + 0.6 * l[1] + 0.1 * l[2]
        })
        .collect();
    means.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = means[means.len() / 2];
    assert!(
        (0.02..0.6).contains(&median),
        "median prop light {median:.4}: below 0.02 the corals are black, above \
         0.6 something is scaled wrong (the map's sun is 32/255)"
    );
}

#[test]
fn summit_terrain_blends_two_textures_by_vertex_alpha() {
    // `castle/blend_grass_rock04` and friends: WorldVertexTransition paints
    // rock into grass with per-vertex alpha. One texture with a hard seam is
    // what the terrain looked like without it.
    let Some(map) = load("surf_summit") else {
        return;
    };
    let blended: Vec<_> = map.mesh.tris.iter().filter(|t| t.tex2 != 0).collect();
    assert!(blended.len() > 1000, "only {} blended tris", blended.len());
    let lo = blended
        .iter()
        .flat_map(|t| t.alpha.iter())
        .cloned()
        .fold(1.0f32, f32::min);
    let hi = blended
        .iter()
        .flat_map(|t| t.alpha.iter())
        .cloned()
        .fold(0.0f32, f32::max);
    assert!(lo < 0.1 && hi > 0.9, "blend alpha spans {lo}..{hi}; it should reach both textures");
    assert!(
        blended.iter().all(|t| t.tex2 != t.tex),
        "a second layer equal to the first is not a blend"
    );
}

#[test]
fn material_atlas_carries_a_full_mip_chain() {
    let Some(map) = load("surf_summit") else {
        return;
    };
    let size = map.materials.layer_size;
    assert!(size >= 512, "summit's 1024² textures should not be squashed to {size}");
    let levels = 1 + map.materials.mips.len() as u32;
    assert_eq!(levels, size.ilog2() + 1, "mip chain should run down to 1×1");
    let last = map.materials.mips.last().unwrap();
    assert_eq!(last.len(), 4 * map.materials.layer_count as usize);
    assert_eq!(
        map.materials.rgba.len(),
        (size * size * 4 * map.materials.layer_count) as usize
    );
}
