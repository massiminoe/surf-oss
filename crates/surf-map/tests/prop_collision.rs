//! Static-prop collision comes from the model's `.phy` hull and nothing else.
//!
//! Source builds a `prop_static`'s collision from its `vcollide` — the `.phy`
//! that ships beside the `.mdl` — and never traces a render mesh. A model with
//! no `.phy` is simply not solid, which is why mappers leave `prop_static` on
//! its default "Use VPhysics" without a second thought.
//!
//! botanica is the case that made this matter: 971 props marked `solid=Physics`,
//! not one of them carrying a `.phy` — grass, flowers and ivy included. The old
//! render-mesh fallback turned them into 1.44M triangles of invisible fence, and
//! the worst of it sat across the stage-end portals, which had to be threaded.
//!
//! Lever for the old behaviour: `SURF_OSS_PROP_RENDER_COLLISION=1`.

use surf_core::math::Vec3;
use surf_core::trace::trace_box;
use surf_map::LoadedMap;

fn load(name: &str) -> Option<LoadedMap> {
    let path = format!(
        "{}/../../assets/maps/{name}.bsp",
        env!("CARGO_MANIFEST_DIR")
    );
    if !std::path::Path::new(&path).exists() {
        eprintln!("{name} absent; skipping");
        return None;
    }
    Some(LoadedMap::load_path(&path).unwrap_or_else(|e| panic!("load {name}: {e}")))
}

/// The stage-2 exit portal on botanica is a `trigger_teleport` plane at
/// y = -5986 spanning x 1672..2432, z -3648..-2976. `portal2.mdl` (the ring you
/// fly through) and `portal_steps.mdl` sit inside it, both `solid=Physics`, both
/// without a `.phy`. Driving a standing hull at the middle of the opening used
/// to stop at y = -5963 — short of the trigger — so the transition could only be
/// hit by threading the gaps in the ring's render mesh.
///
/// Asserted as the gameplay fact (the hull reaches the trigger), not as "prop
/// count is zero": the ring is still drawn, and still would be if some future
/// change gave it a real hull.
#[test]
fn botanica_stage_portals_are_not_blocked_by_their_own_scenery() {
    let Some(map) = load("surf_botanica") else {
        return;
    };
    let hull_mins = Vec3::new(-16.0, -16.0, 0.0);
    let hull_maxs = Vec3::new(16.0, 16.0, 62.0);

    // Straight at the portal plane down the middle of the opening.
    let from = Vec3::new(2053.0, -5700.0, -3620.0);
    let to = Vec3::new(2053.0, -6200.0, -3620.0);
    let trigger_y = -5986.0;

    let hit = trace_box(&map.world, from, to, hull_mins, hull_maxs);
    assert!(
        !hit.startsolid,
        "hull starts solid in front of the portal at {from:?}"
    );
    assert!(
        hit.endpos.y < trigger_y,
        "hull stopped at y={:.0} before reaching the stage-2 exit portal at y={trigger_y:.0} \
         (fraction {:.3}) — scenery is blocking the transition",
        hit.endpos.y,
        hit.fraction,
    );
}

/// The mechanism behind the test above, stated once so a regression names itself:
/// no prop contributes collision unless its collision came from a `.phy`.
#[test]
fn no_prop_collides_from_its_render_mesh() {
    for name in ["surf_botanica", "surf_lovetunnel", "surf_demise"] {
        let Some(map) = load(name) else { continue };
        let offenders: Vec<_> = map
            .props
            .iter()
            .filter(|p| p.solid && !p.from_phy)
            .map(|p| format!("{} x{}", p.model, p.tris.len()))
            .collect();
        assert!(
            offenders.is_empty(),
            "{name}: {} props collide from their render mesh, e.g. {:?}",
            offenders.len(),
            &offenders[..offenders.len().min(5)]
        );
    }
}
