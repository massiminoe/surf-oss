//! Integration: load a real corpus map and sanity-check collision/mesh.

use surf_core::math::Vec3;
use surf_map::LoadedMap;

#[test]
fn load_kitsune_has_collision_and_spawn() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/surf_kitsune.bsp");
    let map = LoadedMap::load_path(path).expect("load kitsune");
    assert!(
        map.world.brushes.len() > 100,
        "expected hundreds of player-solid brushes, got {}",
        map.world.brushes.len()
    );
    assert!(
        map.mesh.tris.len() > 1000,
        "expected a renderable mesh, got {} tris",
        map.mesh.tris.len()
    );
    assert!(
        !map.teleports.is_empty(),
        "kitsune should have trigger_teleport"
    );
    // Spawn should be somewhere near the map, not at origin by accident only.
    let o = map.spawn_origin;
    assert!(
        o.x.abs() + o.y.abs() + o.z.abs() > 1.0,
        "spawn looks unset: {o:?}"
    );
}

#[test]
fn summit_prefers_td_mapstart_over_lobby() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/surf_summit.bsp");
    let map = LoadedMap::load_path(path).expect("load summit");
    // Cosmetic T/CT lobby is ~(-224,-3152,11432); stage start is td_mapstart.
    assert!(
        (map.spawn_origin.x - 1600.0).abs() < 1.0
            && (map.spawn_origin.y - 0.0).abs() < 1.0
            && (map.spawn_origin.z - 11552.0).abs() < 1.0,
        "expected td_mapstart (1600,0,11552), got {:?}",
        map.spawn_origin
    );
    assert!(
        map.world.tris.len() > 100_000,
        "summit should load disp collision tris, got {}",
        map.world.tris.len()
    );
}

#[test]
fn aesthetic_maps_spawn_in_start_zone_not_bonus() {
    // Regression: `bonus1_start`.contains(`s1_start`) used to steal gameplay spawn.
    let cases = [
        ("surf_nyx", 14120.0, -8248.0, 2216.0),
        ("surf_fornax", -13920.0, -3776.0, 15232.0),
        ("surf_frost", -4032.0, -11904.0, 576.0),
        ("surf_lovetunnel", -6096.0, -11552.0, 5248.0),
    ];
    for (name, x, y, z) in cases {
        let path = format!(
            "{}/../../assets/maps/{name}.bsp",
            env!("CARGO_MANIFEST_DIR")
        );
        let map = LoadedMap::load_path(&path).unwrap_or_else(|e| panic!("load {name}: {e}"));
        assert!(
            (map.spawn_origin.x - x).abs() < 1.0
                && (map.spawn_origin.y - y).abs() < 1.0
                && (map.spawn_origin.z - z).abs() < 1.0,
            "{name}: expected gameplay spawn ({x},{y},{z}), got {:?}",
            map.spawn_origin
        );
    }
}

#[test]
fn summit_teleports_use_entity_origin() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/surf_summit.bsp");
    let map = LoadedMap::load_path(path).expect("load summit");

    // Early fail volume under first drop (entity origin 5536 0 10496; brushes
    // are stepped — probe inside a solid piece, not the entity origin Z).
    let early = map
        .touch_teleport(Vec3::new(5536.0, 0.0, 10200.0))
        .expect("early fail floor should teleport");
    assert!(
        (early.0.x - 1600.0).abs() < 1.0 && (early.0.z - 11552.0).abs() < 1.0,
        "expected td_mapstart, got {:?}",
        early.0
    );

    // Mid-map on the line (near cp2) must not hit a phantom origin-space net.
    assert!(
        map.touch_teleport(Vec3::new(-1472.0, 0.0, 1377.0)).is_none(),
        "cp2 should not soft-reset"
    );
    assert!(
        map.touch_teleport(Vec3::ZERO).is_none(),
        "world origin must not be a fail net after origin transform"
    );
}
