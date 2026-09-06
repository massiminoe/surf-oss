//! M3: pakfile VMT/VTF resolve for summit custom materials.

use surf_map::LoadedMap;

#[test]
fn summit_loads_pakfile_albedos() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_summit.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load summit");
    assert!(
        map.materials.textured_count >= 5,
        "expected several CASTLE albedos from pakfile, got {}",
        map.materials.textured_count
    );
    assert!(
        map.materials.layer_count > map.materials.textured_count,
        "layer 0 is reserved white"
    );
    let textured_tris = map.mesh.tris.iter().filter(|t| t.tex > 0).count();
    assert!(
        textured_tris > 1000,
        "expected many textured tris, got {textured_tris}"
    );
    assert_eq!(map.skyname.as_deref(), Some("ff_sky_clearblue"));
    assert!(
        !map.skybox.is_empty(),
        "summit pakfile should embed 2D skybox faces"
    );
    assert!(
        map.lightmaps.face_count > 500,
        "summit LDR lightmap atlas should pack many faces, got {}",
        map.lightmaps.face_count
    );
    assert!(map.lightmaps.width >= 512 && map.lightmaps.height >= 64);
    // Lightmap UVs must be normalized 0..1 (not leftover atlas pixels).
    let bad = map
        .mesh
        .tris
        .iter()
        .flat_map(|t| [t.lm_a, t.lm_b, t.lm_c])
        .filter(|uv| uv[0] < -0.01 || uv[0] > 1.01 || uv[1] < -0.01 || uv[1] > 1.01)
        .count();
    assert_eq!(bad, 0, "lm UVs outside 0..1 after normalize");
    // Reserved "L = 1" texel for faces without luxels (and for static props,
    // whose light is per vertex): atlas (0,0) stores sRGB(0.5), which the
    // shader's overbright ×2 turns back into unity.
    let px = &map.lightmaps.rgba[0..4];
    let unit = surf_map::UNIT_LIGHT_BYTE;
    assert!(
        px[0] == unit && px[1] == unit && px[2] == unit,
        "atlas origin should be the reserved unit-light texel {unit}, got {px:?}"
    );
}
