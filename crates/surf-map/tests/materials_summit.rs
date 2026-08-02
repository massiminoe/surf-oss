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
    // Reserved white texel for unlit faces — atlas (0,0) area should be bright.
    let px = &map.lightmaps.rgba[0..4];
    assert!(
        px[0] > 200 && px[1] > 200 && px[2] > 200,
        "atlas origin should be reserved white, got {px:?}"
    );
}
