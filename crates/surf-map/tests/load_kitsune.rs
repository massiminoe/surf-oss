//! Integration: load a real corpus map and sanity-check collision/mesh.

use surf_core::math::Vec3;
use surf_map::LoadedMap;

#[test]
fn load_kitsune_has_collision_and_spawn() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_kitsune.bsp"
    );
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
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_summit.bsp"
    );
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

/// The gameplay spawn must land in the map's own start zone, standing free.
///
/// Coordinates are deliberately *not* hard-coded here: the earlier version of
/// this test pinned the exact origins the picker happened to produce, and three
/// of them (fornax / frost / lovetunnel) were trigger-volume centres — lovetunnel's
/// left the hull startsolid, i.e. unable to move at all. Assert the properties
/// that matter instead, against `assets/zones/<map>.json`:
///   * inside the main start box (so: not a bonus start, not the lobby),
///   * hull free (not wedged in geometry),
///   * a floor within a short drop underneath.
#[test]
fn aesthetic_maps_spawn_in_start_zone_not_bonus() {
    let hull_mins = Vec3::new(-16.0, -16.0, 0.0);
    let hull_maxs = Vec3::new(16.0, 16.0, 62.0);
    for name in [
        "surf_nyx",
        "surf_fornax",
        "surf_frost",
        "surf_lovetunnel",
        "surf_void",
        "surf_hourglass",
        "surf_lux",
        "surf_pantheon",
    ] {
        let path = format!(
            "{}/../../assets/maps/{name}.bsp",
            env!("CARGO_MANIFEST_DIR")
        );
        let map = LoadedMap::load_path(&path).unwrap_or_else(|e| panic!("load {name}: {e}"));
        let spawn = map.spawn_origin;

        let zone: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../../assets/zones/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap_or_else(|e| panic!("zones for {name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse zones for {name}: {e}"));
        let start = &zone["tracks"]["main"]["start"];
        let axis = |k: &str, i: usize| start[k][i].as_f64().expect("zone bound") as f32;
        for (i, v) in [spawn.x, spawn.y, spawn.z].into_iter().enumerate() {
            assert!(
                v >= axis("mins", i) && v <= axis("maxs", i),
                "{name}: spawn {spawn:?} outside start zone on axis {i} \
                 ({}..{})",
                axis("mins", i),
                axis("maxs", i)
            );
        }

        let tr = surf_core::trace::trace_box(
            &map.world,
            spawn,
            spawn - Vec3::new(0.0, 0.0, 4096.0),
            hull_mins,
            hull_maxs,
        );
        assert!(
            !tr.startsolid,
            "{name}: spawn {spawn:?} is wedged in geometry — the player cannot move"
        );
        assert!(tr.fraction < 1.0, "{name}: nothing under the spawn");
        let drop = spawn.z - tr.endpos.z;
        assert!(
            drop < 400.0,
            "{name}: spawn is {drop:.0}u above the floor, not on a start platform"
        );
    }
}

/// lovetunnel names its only teleport destination `reset`, and the score table
/// penalises fail/reset pads — so the picker fell through to the `startzone`
/// *trigger*, whose centre is 144u of air flush against a wall corner. The hull
/// touched that corner, `trace_box` reported startsolid, and every move clipped
/// to fraction 0: hovering, unable to walk, fall or jump.
///
/// A teleport destination standing inside the start trigger is the map start,
/// whatever it is called.
#[test]
fn lovetunnel_spawns_on_its_start_platform_not_in_the_startzone_wall() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_lovetunnel.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load lovetunnel");
    let spawn = map.spawn_origin;
    let hull_mins = Vec3::new(-16.0, -16.0, 0.0);
    let hull_maxs = Vec3::new(16.0, 16.0, 62.0);

    // The old pick, kept explicit so the failure mode stays recognisable.
    let wedged = Vec3::new(-6096.0, -11552.0, 5248.0);
    assert!(
        surf_core::trace::point_contents_box(&map.world, wedged, hull_mins, hull_maxs),
        "the startzone centre is supposed to be the wedged case"
    );
    assert!(
        (spawn - wedged).length() > 1.0,
        "spawn is still the startzone trigger centre"
    );

    let tr = surf_core::trace::trace_box(
        &map.world,
        spawn,
        spawn - Vec3::new(0.0, 0.0, 4096.0),
        hull_mins,
        hull_maxs,
    );
    assert!(!tr.startsolid, "spawn {spawn:?} still wedged");
    let normal = tr.hit.as_ref().expect("floor under spawn").normal;
    assert!(normal.z > 0.99, "start platform should be flat, got {normal:?}");
    assert!(
        (tr.endpos.z - 5104.0).abs() < 8.0,
        "landed at z={:.1}, expected the start-platform floor at 5104",
        tr.endpos.z
    );
}

#[test]
fn summit_teleports_use_entity_origin() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_summit.bsp"
    );
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
        map.touch_teleport(Vec3::new(-1472.0, 0.0, 1377.0))
            .is_none(),
        "cp2 should not soft-reset"
    );
    assert!(
        map.touch_teleport(Vec3::ZERO).is_none(),
        "world origin must not be a fail net after origin transform"
    );
}

#[test]
fn cement_thin_stage2_teleport_fires_with_hull() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_cement.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load cement");
    // Stage-1 end slab is ~2u thick at z≈7425. Point probes miss it; hull touch
    // must fire once feet enter the volume (WR falls through here → stage2).
    let hit = map
        .touch_teleport(Vec3::new(-14720.0, 12800.0, 7425.0))
        .expect("cement tos2 thin floor should teleport");
    assert!(
        (hit.0.x - 4352.0).abs() < 1.0
            && (hit.0.y - 8192.0).abs() < 1.0
            && (hit.0.z - 14464.0).abs() < 1.0,
        "expected stage2 dest, got {:?}",
        hit.0
    );
    // Still above the slab: feet at WR pre-tele frame must not fire yet.
    assert!(
        map.touch_teleport(Vec3::new(-14739.0, 12845.0, 7440.0))
            .is_none(),
        "hull above thin slab should not teleport early"
    );
}

/// boreas' start platform is a `solid=Physics` static prop
/// (`project_tendies/details/dek01.mdl`), not world brushes or a displacement.
/// While prop collision was gated to model paths containing "ramp", the deck was
/// dropped: the player spawned at z=14870 over open terrain and fell 235u onto a
/// sloped displacement instead of landing on a flat deck at the start-zone floor
/// (14736). Guard the landing, not the gate — any future prop filter that loses
/// the deck fails here.
#[test]
fn boreas_spawns_onto_its_start_deck_not_the_terrain_below() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_boreas.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load boreas");

    let spawn = map.spawn_origin;
    let hull_mins = Vec3::new(-16.0, -16.0, 0.0);
    let hull_maxs = Vec3::new(16.0, 16.0, 62.0);
    let down = spawn - Vec3::new(0.0, 0.0, 4096.0);
    let tr = surf_core::trace::trace_box(&map.world, spawn, down, hull_mins, hull_maxs);

    assert!(!tr.startsolid, "spawn is inside geometry");
    assert!(tr.fraction < 1.0, "nothing at all under the boreas spawn");

    let drop = spawn.z - tr.endpos.z;
    assert!(
        drop < 200.0,
        "expected a platform just under the spawn, fell {drop:.1}u"
    );

    let normal = tr.hit.as_ref().expect("hit surface").normal;
    assert!(
        normal.z > 0.99,
        "start deck should be flat, landed on normal {normal:?}"
    );
    // The zone's floor is 14736; landing on the deck puts the player on it.
    assert!(
        (tr.endpos.z - 14736.0).abs() < 8.0,
        "landed at z={:.1}, expected the start-zone floor at 14736",
        tr.endpos.z
    );
}

/// Prop collision must come from the model's `.phy` hull, not its render mesh.
///
/// Two things this pins, both of which cost real speed when they were wrong:
///   * the hull is used at all — boreas' `ramp_c1` is 684 render triangles but
///     128 collision triangles, and the render mesh's decorative folds formed
///     90° creases that dead-stopped a 3606 u/s graze at 15 u/s;
///   * the whole ledge tree is walked. A node is a leaf when `offset_right == 0`;
///     using `offset_ledge != 0` instead collapses the tree to one ledge (74 of
///     128 triangles on this model) and silently loses most of the ramp.
#[test]
fn boreas_prop_collision_comes_from_phy_hulls() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_boreas.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load boreas");

    let prop_tris = map.world.tris.len() - map.prop_tri_start;
    // 11 solid props: 1 deck (72 tris) + 10 ramps (80 or 128 each) = 1208.
    assert!(
        (1100..1350).contains(&prop_tris),
        "expected ~1208 .phy collision tris for boreas' solid props, got {prop_tris} \
         (the render-mesh fallback would give ~6098; a ledge tree collapsed by the \
         wrong leaf test gives ~679)"
    );
}

/// boreas' 3D skybox is a pine forest 26,000 units below the map, drawn at full
/// scale because we have no skybox pass. Source renders that area scaled around
/// the `sky_camera`; drawing it as world geometry cost **51% of the entire draw
/// mesh** for something the player can never reach or correctly see.
///
/// Scope: this covers static props, which were all of the cost that mattered.
/// The skybox's own brush faces and displacements are still drawn — 46k tris on
/// boreas, ~3% of what is left — because culling those needs a face-to-leaf
/// mapping the mesh builder does not carry yet.
#[test]
fn boreas_does_not_draw_its_3d_skybox_props_as_world_geometry() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/maps/surf_boreas.bsp"
    );
    let map = LoadedMap::load_path(path).expect("load boreas");

    let drawn: Vec<_> = map
        .props
        .iter()
        .filter(|p| p.origin.z < -5000.0 && !p.render_tris.is_empty())
        .collect();
    assert!(
        drawn.is_empty(),
        "{} skybox props are still drawn, e.g. {}",
        drawn.len(),
        drawn[0].model
    );

    // A solid skybox cloud or tree is a trap in the trace grid, not just cost.
    let solid = map
        .props
        .iter()
        .filter(|p| p.origin.z < -5000.0 && !p.tris.is_empty())
        .count();
    assert_eq!(solid, 0, "{solid} skybox props still have collision");

    // The map's own area partition must have been read: boreas puts 640 of its
    // 1587 props in the skybox area, and the census keeps every prop either way.
    let culled = map.props.iter().filter(|p| p.skybox).count();
    assert!(
        culled >= 600,
        "expected ~640 props culled as 3D skybox, got {culled} \u{2014} \
         has the sky_camera leaf lookup stopped resolving?"
    );

    // Guard the win: before the cull boreas drew 2.75M triangles.
    assert!(
        map.mesh.tris.len() < 1_600_000,
        "boreas mesh is {} tris, expected the skybox forest to be gone",
        map.mesh.tris.len()
    );
}
