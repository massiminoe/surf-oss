//! Watching a real replay end to end: the summit KSF world record plays
//! through `ReplayWatch` at 8x, reaches its last frame, and every camera it
//! offers along the way is a sane one — the first-person eye is the recorded
//! eye, and the chase camera never wanders further from the runner than it
//! is asked to.

use std::path::PathBuf;

use surf_app::leaderboard;
use surf_app::replay::Replay;
use surf_app::watch::{ReplayPick, ReplayWatch};
use surf_map::LoadedMap;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_summit_world_record_plays_to_its_last_frame_with_a_sane_camera() {
    let map_path = root().join("assets/maps/surf_summit.bsp");
    let Some(rec) = leaderboard::ksf_records("surf_summit").into_iter().next() else {
        eprintln!("summit records absent; skipping");
        return;
    };
    let Some(pick) = ReplayPick::ksf("surf_summit", &rec) else {
        eprintln!("summit WR replay absent; skipping");
        return;
    };
    if !map_path.is_file() {
        eprintln!("summit absent; skipping");
        return;
    }
    let map = LoadedMap::load_path(&map_path).expect("summit");
    let replay = Replay::load(&pick.path).expect("wr replay");
    let n = replay.frames.len();
    let first_origin_z = replay.frames[0].origin.z;
    let mut w = ReplayWatch::new(pick, replay, &map.world);

    // First person starts at the recorded eye (crouched in the start zone or
    // not — the hull decides the height).
    let eye_h = if w.ducked() { 47.0 } else { 64.0 };
    assert!((w.eye().0.z - (first_origin_z + eye_h)).abs() < 1e-3);

    // 8x, fed 120 Hz frames, with the chase camera checked as it goes.
    for _ in 0..3 {
        w.rate_step(1);
    }
    assert_eq!(w.rate(), 8.0);
    let mut frames_seen = 0usize;
    let mut worst_chase = 0.0f32;
    while !w.at_end() {
        w.advance(1.0 / 120.0);
        frames_seen += 1;
        let (eye, _) = w.chase_eye(&map.world);
        let d = (eye - w.origin()).length();
        worst_chase = worst_chase.max(d);
        assert!(frames_seen < 100_000, "never reached the end");
    }
    assert!(w.paused, "a finished clip holds paused");
    assert_eq!(w.frame_idx() + 1, n);
    assert!(
        (w.progress() - 1.0).abs() < 0.02,
        "progress {}",
        w.progress()
    );
    // Wall time spent: the run length over the rate, within a frame or two.
    let expected = (n as f32 * w.tick_interval()) / 8.0 * 120.0;
    assert!(
        (frames_seen as f32 - expected).abs() < 4.0,
        "8x took {frames_seen} frames, expected ~{expected:.0}"
    );
    // The chase eye is pulled in by the trace, never pushed out: at most the
    // 150u stand-off plus the hull-centre and lift offsets (36 + 24).
    assert!(
        worst_chase <= 212.0,
        "chase camera got {worst_chase:.0}u from the runner"
    );

    // Splits from the header land on the scrub bar in order.
    let marks = &w.splits;
    assert!(marks.windows(2).all(|p| p[0] < p[1]));
    assert_eq!(
        w.splits_passed(),
        marks.len(),
        "at the end every split is behind us"
    );
}
