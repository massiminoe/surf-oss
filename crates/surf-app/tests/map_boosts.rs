//! Map boosters, checked against the tick the real CS:S recording moved on.
//!
//! Both maps' boosts were missing entirely until the entity-I/O path existed —
//! tendies' whole run starts with one — so these guard the two mechanisms that
//! make them work: `AddOutput basevelocity` released as a one-shot impulse, and
//! a `trigger_push` paying out when you leave it.

use std::path::PathBuf;

use surf_app::replay::Replay;
use surf_core::math::Vec3;
use surf_core::movement::{apply_base_velocity_momentum, check_velocity};
use surf_map::{FieldState, LoadedMap};

const DT: f32 = 0.015;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Walk a WR recording, returning `(tick, released)` for every payout. The tick
/// is the one the payout is *armed* at; it acts on the following move.
fn payouts(map: &LoadedMap, frames: &[surf_app::replay::ReplayFrame]) -> Vec<(usize, Vec3)> {
    let mut state = FieldState::new();
    let mut out = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let eff = state.update(&map.fields, f.origin, map.touch_push(f.origin));
        if eff.released != Vec3::ZERO {
            out.push((i, eff.released));
        }
    }
    out
}

fn load(map_file: &str, replay_file: &str) -> Option<(LoadedMap, Vec<surf_app::replay::ReplayFrame>)> {
    let map_path = root().join("assets/maps").join(map_file);
    let replay_path = root().join(replay_file);
    if !map_path.is_file() || !replay_path.is_file() {
        eprintln!("{map_file} assets absent; skipping");
        return None;
    }
    let map = LoadedMap::load_path(&map_path).expect("map");
    let frames = Replay::load(&replay_path).expect("replay").frames;
    Some((map, frames))
}

/// tendies opens with `OnEndTouch !activator,AddOutput,basevelocity 1337 0 0`,
/// gated by a negated name filter so it fires once. The WR gains +1347.0 u/s of
/// x in a single tick — 1337 × (1 + dt/2) — and everything downstream of the
/// first ramp depends on carrying that speed in.
#[test]
fn tendies_start_booster_matches_the_wr_tick_and_magnitude() {
    let Some((map, frames)) = load(
        "surf_tendies.bsp",
        "assets/replays/external/ksf/surf_tendies/imported/replay_css_4074_0_712551_1752625913.osxr",
    ) else {
        return;
    };

    let fired = payouts(&map, &frames);
    assert_eq!(
        fired.len(),
        1,
        "the start boost is name-gated to one payout per run, got {fired:?}"
    );
    let (tick, released) = fired[0];
    assert_eq!(released, Vec3::new(1337.0, 0.0, 0.0));

    // Armed at the end of `tick`, so it acts on the move of `tick + 1`, which is
    // where the recording steps.
    let recorded_step = frames[tick + 1].velocity.x - frames[tick].velocity.x;
    let mut v = frames[tick].velocity;
    apply_base_velocity_momentum(&mut v, released, DT);
    assert!(
        (v.x - frames[tick].velocity.x - recorded_step).abs() < 1.0,
        "tick {tick}: we add {:.1}, CS:S added {recorded_step:.1}",
        v.x - frames[tick].velocity.x
    );
}

/// lovetunnel's 3500 u/s tunnel is a plain `trigger_push`: it carries you while
/// you are inside (no lasting speed) and folds into velocity on the tick you
/// leave, where sv_maxvelocity then clamps x to exactly -3500.
#[test]
fn lovetunnel_push_pays_out_on_exit_and_clamps_like_the_wr() {
    let Some((map, frames)) = load(
        "surf_lovetunnel.bsp",
        "assets/replays/external/ksf/surf_lovetunnel/imported/replay_css_3737_0_540902_1752122118.osxr",
    ) else {
        return;
    };

    let fired = payouts(&map, &frames);
    let (tick, released) = *fired
        .iter()
        .find(|(_, r)| r.x < -3000.0)
        .expect("the 3500 u/s tunnel must pay out");
    assert!((released.x + 3500.0).abs() < 1.0, "released {released:?}");

    let mut v = frames[tick].velocity;
    apply_base_velocity_momentum(&mut v, released, DT);
    let mut origin = frames[tick].origin;
    check_velocity(&mut v, &mut origin, 3500.0);
    assert!(
        (v.x - frames[tick + 1].velocity.x).abs() < 1.0,
        "tick {tick}: we reach {:.1} u/s of x, CS:S reached {:.1}",
        v.x,
        frames[tick + 1].velocity.x
    );
}

/// The launch pads at the bottom of lovetunnel are `AddOutput gravity -100`,
/// restored to 1 on the way out. Without entity I/O they did nothing at all.
#[test]
fn lovetunnel_launch_pads_invert_gravity_while_you_stand_in_them() {
    let map_path = root().join("assets/maps/surf_lovetunnel.bsp");
    if !map_path.is_file() {
        eprintln!("lovetunnel absent; skipping");
        return;
    }
    let map = LoadedMap::load_path(&map_path).expect("lovetunnel");

    let pad = map
        .fields
        .iter()
        .find(|f| {
            f.on_start
                .iter()
                .any(|a| matches!(a, surf_map::FieldAction::Gravity(g) if *g < 0.0))
        })
        .expect("lovetunnel has anti-gravity pads");
    let centre = Vec3::new(
        (pad.bounds.mins.x + pad.bounds.maxs.x) * 0.5,
        (pad.bounds.mins.y + pad.bounds.maxs.y) * 0.5,
        pad.bounds.mins.z,
    );
    let outside = Vec3::new(pad.bounds.mins.x - 4096.0, centre.y, centre.z);

    let mut state = FieldState::new();
    assert_eq!(state.update(&map.fields, outside, Vec3::ZERO).gravity_scale, 1.0);
    assert_eq!(
        state.update(&map.fields, centre, Vec3::ZERO).gravity_scale,
        -100.0,
        "standing in the pad must invert gravity"
    );
    assert_eq!(
        state.update(&map.fields, outside, Vec3::ZERO).gravity_scale,
        1.0,
        "leaving must restore it"
    );
}
