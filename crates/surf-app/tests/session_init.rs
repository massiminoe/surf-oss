//! `Session` owns the spawn/settle sequence that used to live inline in
//! `App::new`, plus the per-map movevar overrides. Nothing else in the suite
//! exercises that path — `ksf_resim_audit` drives `surf_app::resim` and
//! `surf_core::tick` directly and never touches it — so pin it here against a
//! reference computed independently from `surf_core`.

use surf_app::session::{Level, Session};
use surf_app::zones::{load_zones_file, zones_path_for_map};
use surf_core::graybox;
use surf_core::math::Angle;
use surf_core::movement::{MoveVars, PlayerState, UserCmd};
use surf_core::tick;

/// The settle loop is 10 default ticks at the spawn pose. If that count or the
/// vars it runs under ever drift, every run on every map starts from a
/// different place.
///
/// The spawn is deliberately lifted clear of the floor first: at the arena's
/// own spawn the player is already at rest, so the settle loop is a fixed point
/// and *any* tick count agrees. Falling makes the count observable, which is
/// what gives this test teeth.
#[test]
fn graybox_session_settles_exactly_like_the_reference_sequence() {
    let mut arena = graybox::surf_ramp_arena();
    arena.spawn_origin.z += 512.0;
    let spawn_origin = arena.spawn_origin;
    let spawn_yaw = arena.spawn_yaw;

    let session = Session::new(Level::Graybox(arena), None, None, None);

    let arena = {
        let mut a = graybox::surf_ramp_arena();
        a.spawn_origin.z += 512.0;
        a
    };
    let vars = MoveVars::momentum_surf();
    let mut expect = PlayerState {
        origin: spawn_origin,
        viewangles: Angle::new(0.0, spawn_yaw, 0.0),
        grounded: true,
        ..PlayerState::default()
    };
    for _ in 0..10 {
        expect = tick(&arena.world, &expect, &UserCmd::default(), &vars);
    }
    assert_ne!(
        expect.origin, spawn_origin,
        "the lifted spawn must actually fall, or this test proves nothing"
    );

    // Exact f32 — this is a determinism boundary, not an approximation.
    assert_eq!(
        session.player.origin,
        expect.origin,
        "settled spawn origin drifted"
    );
    assert_eq!(
        session.player.velocity,
        expect.velocity,
        "settled spawn velocity drifted"
    );
    assert_eq!(session.player.grounded, expect.grounded);
    // The interpolation span must start collapsed, or the first frame smears.
    assert_eq!(session.prev_origin, session.player.origin);
    assert_eq!(session.accumulator, 0.0);
}

/// `airaccelerate` is a feel preference and follows the player across a map
/// change; `maxvelocity` belongs to the map and must not.
#[test]
fn airaccelerate_carries_across_a_map_change_but_maxvelocity_does_not() {
    let carried = Session::graybox(None, Some(275.0));
    assert_eq!(carried.vars.airaccelerate, 275.0);

    let fresh = Session::graybox(None, None);
    assert_eq!(
        fresh.vars.airaccelerate,
        MoveVars::momentum_surf().airaccelerate,
        "no carry value should leave the default alone"
    );
    // Graybox has no zone file, so it keeps the stock cap.
    assert_eq!(
        fresh.vars.maxvelocity,
        MoveVars::momentum_surf().maxvelocity
    );
}

/// Summit's zone file sets `maxVelocity: 0` (uncapped). A session built for it
/// has to pick that up — this is the override that makes summit's WR possible.
#[test]
fn a_maps_zone_file_overrides_maxvelocity() {
    let map = std::path::Path::new("assets/maps/surf_summit.bsp");
    let zpath = zones_path_for_map(map);
    if !zpath.is_file() {
        eprintln!("summit zones absent; skipping");
        return;
    }
    let zones = load_zones_file(&zpath).expect("summit zones");
    let Some(expected) = zones.max_velocity else {
        panic!("summit zones no longer declare maxVelocity");
    };

    // Build against the graybox world so the test does not need the BSP — the
    // override path is the same either way.
    let session = Session::new(
        Level::Graybox(graybox::surf_ramp_arena()),
        Some(zones),
        None,
        None,
    );
    assert_eq!(session.vars.maxvelocity, expected);
    assert_eq!(expected, 0.0, "summit is expected to be uncapped");
}

/// A fresh session must never come up already tainted, or a real run could be
/// silently downgraded to practice.
#[test]
fn a_new_session_is_not_in_practice_mode() {
    let session = Session::graybox(None, None);
    assert!(!session.practice_mode, "practice mode must be off on load");
    assert!(!session.run_timer.practice);
    assert!(session.pb_ghost.is_none());
    assert!(session.notice_line.is_none());
    assert!(!session.finish_recorded);
}
