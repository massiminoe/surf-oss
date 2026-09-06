//! Replay viewer: play any `.osxr` back through the recorded poses — your
//! own runs, your PB, or an imported KSF record — as a first-person camera
//! (or a chase camera behind the hull).
//!
//! This is *playback*, not re-simulation: the frames carry the pose the
//! recording engine produced, so a KSF world record shows the real run, not
//! our resim of it. `tick()` is never called while watching, and the live
//! session (player, clock, recorder) is untouched — stopping a replay drops
//! you back into the pause menu with your attempt exactly where you left it.
//!
//! The state here is pure so the transport rules (rate, seek, step, end of
//! stream) are unit-tested; the event loop only forwards keys to it.

use std::path::PathBuf;

use surf_audio::{EventDetector, Observation};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::Hull;
use surf_core::trace::trace_box;
use surf_core::World;

use crate::leaderboard;
use crate::pb::RunRecord;
use crate::replay::{self, GhostPlayback, Replay, ReplayFrame};
use crate::timer::format_time;

/// Playback speeds, slowest first. Index 2 is real time.
pub const RATES: [f32; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
const RATE_REALTIME: usize = 2;
/// ←→ seek distance.
pub const SEEK_SECS: f32 = 5.0;
/// Chase camera: how far behind the hull's centre the eye sits, and how far
/// above it. The eye is traced back from the centre so it stops at a wall
/// instead of going through the ramp the runner is on.
const CHASE_BACK: f32 = 150.0;
const CHASE_UP: f32 = 24.0;
const CHASE_PITCH_MAX: f32 = 80.0;

const BTN_JUMP: u8 = 1;
const BTN_DUCK: u8 = 2;

/// A replay the times pages can offer. Built when the page is built, so a
/// row and the file it plays can never disagree.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayPick {
    pub map: String,
    pub path: PathBuf,
    /// What the HUD calls it while it plays: `KSF #1 FinCS2`, `PB`, `run 3`.
    pub label: String,
}

impl ReplayPick {
    /// An imported KSF record, if its `.osxr` is on disk.
    pub fn ksf(map: &str, rec: &leaderboard::KsfRecord) -> Option<Self> {
        let path = replay::ksf_imported_dir(map).join(format!("{}.osxr", rec.file_stem));
        path.is_file().then(|| Self {
            map: map.to_string(),
            path,
            label: format!("KSF #{} {}", rec.rank, rec.name),
        })
    }

    /// The player's PB replay, if one has been saved.
    pub fn pb(map: &str) -> Option<Self> {
        let path = replay::pb_replay_path(map);
        path.is_file().then(|| Self {
            map: map.to_string(),
            path,
            label: "PB".to_string(),
        })
    }

    /// One row of the run history. `ordinal` is its 1-based position in the
    /// list, which is how the page names it.
    pub fn run(map: &str, rec: &RunRecord, ordinal: usize) -> Option<Self> {
        let path = PathBuf::from(rec.replay_path.as_deref()?);
        path.is_file().then(|| Self {
            map: map.to_string(),
            path,
            label: format!("run {ordinal} · {}", format_time(rec.time_secs)),
        })
    }
}

/// A replay being watched.
pub struct ReplayWatch {
    pub pick: ReplayPick,
    /// The frames plus the ramp-contact classification the trail colours by.
    ghost: GhostPlayback,
    /// Whole-run length from the header, for the scrub bar.
    pub total_secs: f32,
    /// Checkpoint splits from the header, drawn as ticks on the scrub bar.
    pub splits: Vec<f32>,
    /// Fraction of the way from the current frame to the next.
    pub alpha: f32,
    accumulator: f32,
    pub paused: bool,
    rate_idx: usize,
    pub third_person: bool,
    /// Chase-camera orbit from the mouse: (yaw, pitch) offsets in degrees.
    pub orbit: (f32, f32),
    /// Its own detector, so watching never disturbs the session's.
    detect: EventDetector,
}

impl ReplayWatch {
    pub fn new(pick: ReplayPick, replay: Replay, world: &World) -> Self {
        let total_secs = if replay.header.time_secs > 0.0 {
            replay.header.time_secs
        } else {
            replay.frames.len() as f32 * replay.header.tick_interval
        };
        let splits = replay.header.splits.clone();
        // KSF imports store buttons one tick behind the pose; align them so
        // the keys readout shows the input that produced the frame on screen.
        let mut replay = replay;
        replay.frames = replay.frames_for_resim();
        let mut ghost = GhostPlayback::from_replay(replay);
        ghost.classify_ramp_contact(world);
        ghost.begin();
        Self {
            pick,
            ghost,
            total_secs,
            splits,
            alpha: 0.0,
            accumulator: 0.0,
            paused: false,
            rate_idx: RATE_REALTIME,
            third_person: false,
            orbit: (0.0, 0.0),
            detect: EventDetector::default(),
        }
    }

    pub fn frame_count(&self) -> usize {
        self.ghost.frames.len()
    }

    pub fn frame_idx(&self) -> usize {
        self.ghost.frame_idx
    }

    pub fn tick_interval(&self) -> f32 {
        self.ghost.tick_interval
    }

    pub fn rate(&self) -> f32 {
        RATES[self.rate_idx]
    }

    /// What the HUD prints beside the label: the rate, or that it is held.
    pub fn transport_text(&self) -> String {
        if self.paused {
            "PAUSED".to_string()
        } else if self.rate_idx == RATE_REALTIME {
            "1x".to_string()
        } else {
            let r = self.rate();
            if r < 1.0 {
                format!("{r}x")
            } else {
                format!("{r:.0}x")
            }
        }
    }

    pub fn at_end(&self) -> bool {
        self.ghost.frames.is_empty() || self.ghost.frame_idx + 1 >= self.ghost.frames.len()
    }

    /// Elapsed run time at the interpolated position. Frame `i` is elapsed
    /// `(i + 1) * tick_interval`, the same convention as the ghost and
    /// `derive_splits`.
    pub fn time_secs(&self) -> f32 {
        if self.ghost.frames.is_empty() {
            return 0.0;
        }
        (self.ghost.frame_idx as f32 + 1.0 + self.alpha) * self.ghost.tick_interval
    }

    pub fn progress(&self) -> f32 {
        if self.total_secs <= 0.0 {
            return 0.0;
        }
        (self.time_secs() / self.total_secs).clamp(0.0, 1.0)
    }

    /// Number of checkpoints already passed at the current time.
    pub fn splits_passed(&self) -> usize {
        // Half a tick of slack: a split recorded at frame time must count on
        // that frame, and `(i + 1) * dt` does not round-trip exactly in f32.
        let t = self.time_secs() + self.tick_interval() * 0.5;
        self.splits.iter().filter(|s| **s <= t).count()
    }

    /// Move playback on by `dt_real` seconds of wall time. Returns the audio
    /// events the ticks crossed produced. Reaching the last frame holds there
    /// paused rather than looping — a replay ends where the run did.
    pub fn advance(&mut self, dt_real: f32) -> Vec<surf_audio::AudioEvent> {
        let mut out = Vec::new();
        if self.paused || self.ghost.frames.is_empty() {
            return out;
        }
        let dt = self.ghost.tick_interval;
        if dt <= 0.0 {
            return out;
        }
        self.accumulator += dt_real.clamp(0.0, 0.25) * self.rate();
        while self.accumulator >= dt && !self.at_end() {
            self.accumulator -= dt;
            self.ghost.advance();
            let f = self.frame();
            let on_ramp = self
                .ghost
                .on_ramp
                .get(self.ghost.frame_idx)
                .copied()
                .unwrap_or(false);
            let emitted = self.detect.observe(Observation {
                dt,
                speed: f.velocity.length_2d(),
                on_ramp,
                grounded: f.grounded,
                wiped: false,
            });
            out.extend(emitted.iter());
        }
        if self.at_end() {
            // Landing on the last frame is the end: hold there, paused, with
            // nothing carried over to leak into a later play.
            self.accumulator = 0.0;
            self.paused = true;
        }
        self.alpha = if self.paused {
            0.0
        } else {
            (self.accumulator / dt).clamp(0.0, 1.0)
        };
        out
    }

    pub fn toggle_pause(&mut self) {
        if self.paused && self.at_end() {
            // Play from a held end restarts, which is what a play button on a
            // finished clip does everywhere else.
            self.seek_secs(0.0);
        }
        self.paused = !self.paused;
    }

    pub fn rate_step(&mut self, dir: i32) {
        let n = RATES.len() as i32;
        self.rate_idx = (self.rate_idx as i32 + dir).clamp(0, n - 1) as usize;
    }

    /// Jump to an absolute time. Clamps to the stream; any transport state
    /// (paused, rate) is kept.
    pub fn seek_secs(&mut self, secs: f32) {
        let n = self.ghost.frames.len();
        if n == 0 {
            return;
        }
        let dt = self.ghost.tick_interval.max(1e-6);
        let idx = ((secs / dt).round() - 1.0).max(0.0) as usize;
        self.ghost.frame_idx = idx.min(n - 1);
        self.ghost.active = true;
        self.accumulator = 0.0;
        self.alpha = 0.0;
        self.resync_detector();
    }

    pub fn seek_by(&mut self, delta_secs: f32) {
        let t = self.time_secs() + delta_secs;
        self.seek_secs(t);
    }

    /// Scrub to a fraction of the run.
    pub fn seek_frac(&mut self, frac: f32) {
        self.seek_secs(self.total_secs * frac.clamp(0.0, 1.0));
    }

    /// One frame forward or back. Pauses: stepping is a paused activity.
    pub fn step(&mut self, dir: i32) {
        self.paused = true;
        let n = self.ghost.frames.len();
        if n == 0 {
            return;
        }
        let idx = (self.ghost.frame_idx as i64 + dir as i64).clamp(0, n as i64 - 1) as usize;
        self.ghost.frame_idx = idx;
        self.accumulator = 0.0;
        self.alpha = 0.0;
        self.resync_detector();
    }

    fn resync_detector(&mut self) {
        let f = self.frame();
        let on_ramp = self
            .ghost
            .on_ramp
            .get(self.ghost.frame_idx)
            .copied()
            .unwrap_or(false);
        self.detect.resync(on_ramp, f.grounded);
    }

    pub fn frame(&self) -> ReplayFrame {
        let i = self
            .ghost
            .frame_idx
            .min(self.ghost.frames.len().saturating_sub(1));
        self.ghost.frames[i]
    }

    fn next_frame(&self) -> ReplayFrame {
        let i = (self.ghost.frame_idx + 1).min(self.ghost.frames.len().saturating_sub(1));
        self.ghost.frames[i]
    }

    /// Interpolated horizontal speed.
    pub fn speed(&self) -> f32 {
        self.ghost
            .sample(self.alpha)
            .map(|(_, v, _)| v.length_2d())
            .unwrap_or(0.0)
    }

    /// Current ramp contact, for the audio parameter push.
    pub fn on_ramp(&self) -> bool {
        self.ghost
            .on_ramp
            .get(self.ghost.frame_idx)
            .copied()
            .unwrap_or(false)
    }

    pub fn ducked(&self) -> bool {
        self.frame().buttons & BTN_DUCK != 0
    }

    /// Interpolated hull origin.
    pub fn origin(&self) -> Vec3 {
        self.ghost
            .sample(self.alpha)
            .map(|(o, _, _)| o)
            .unwrap_or(Vec3::ZERO)
    }

    /// The recorded view, interpolated with yaw wrapping the short way round.
    pub fn view_angles(&self) -> Angle {
        let a = self.frame();
        let b = self.next_frame();
        let t = self.alpha.clamp(0.0, 1.0);
        Angle::new(
            a.pitch + (b.pitch - a.pitch) * t,
            lerp_yaw(a.yaw, b.yaw, t),
            0.0,
        )
    }

    /// Inputs held on the current frame, for the keys readout.
    pub fn keys(&self) -> (bool, bool, bool, bool, bool) {
        let f = self.frame();
        (
            f.forward_move > 0.0,
            f.forward_move < 0.0,
            f.side_move < 0.0,
            f.side_move > 0.0,
            f.buttons & BTN_JUMP != 0,
        )
    }

    /// First-person camera: the recorded eye.
    pub fn eye(&self) -> (Vec3, Angle) {
        let hull = if self.ducked() {
            Hull::css_duck()
        } else {
            Hull::css_stand()
        };
        (
            self.origin() + Vec3::new(0.0, 0.0, hull.eye_height),
            self.view_angles(),
        )
    }

    /// Chase camera: behind and above the hull, looking where the runner
    /// looked plus the mouse orbit, pulled in wherever the world is in the way.
    pub fn chase_eye(&self, world: &World) -> (Vec3, Angle) {
        let base = self.view_angles();
        let angles = Angle::new(
            (base.pitch + self.orbit.1).clamp(-CHASE_PITCH_MAX, CHASE_PITCH_MAX),
            wrap_yaw(base.yaw + self.orbit.0),
            0.0,
        );
        let (forward, _, _) = angles.vectors();
        let centre = self.origin() + Vec3::new(0.0, 0.0, 36.0);
        let want = centre - forward * CHASE_BACK + Vec3::new(0.0, 0.0, CHASE_UP);
        let probe = Vec3::new(4.0, 4.0, 4.0);
        let tr = trace_box(world, centre, want, -probe, probe);
        let eye = if tr.startsolid { centre } else { tr.endpos };
        (eye, angles)
    }

    /// Mouse motion while in the chase view.
    pub fn orbit_by(&mut self, dyaw: f32, dpitch: f32) {
        self.orbit.0 = wrap_yaw(self.orbit.0 + dyaw);
        self.orbit.1 = (self.orbit.1 + dpitch).clamp(-CHASE_PITCH_MAX, CHASE_PITCH_MAX);
    }

    pub fn toggle_camera(&mut self) {
        self.third_person = !self.third_person;
        self.orbit = (0.0, 0.0);
    }

    /// Path samples from the start through the current frame, for the trail.
    pub fn trail(&self) -> Vec<(Vec3, bool)> {
        self.ghost.trail_to_current()
    }
}

fn wrap_yaw(yaw: f32) -> f32 {
    let mut y = yaw % 360.0;
    if y > 180.0 {
        y -= 360.0;
    }
    if y < -180.0 {
        y += 360.0;
    }
    y
}

/// Interpolate two yaws along the shorter arc, so a view that crosses ±180
/// does not whip the long way round between frames.
fn lerp_yaw(a: f32, b: f32, t: f32) -> f32 {
    let d = wrap_yaw(b - a);
    wrap_yaw(a + d * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{
        ReplayHeader, ReplayVars, REPLAY_FORMAT_VERSION, STYLE_MOMENTUM_SURF, TRACK_MAIN,
    };
    use surf_core::graybox;
    use surf_core::movement::MoveVars;

    fn frame(i: usize) -> ReplayFrame {
        ReplayFrame {
            origin: Vec3::new(i as f32 * 10.0, 0.0, 100.0),
            velocity: Vec3::new(i as f32 * 100.0, 0.0, 0.0),
            pitch: 5.0,
            yaw: if i % 2 == 0 { 170.0 } else { -170.0 },
            forward_move: 1.0,
            side_move: if i % 2 == 0 { -1.0 } else { 1.0 },
            buttons: if i == 3 { BTN_DUCK } else { BTN_JUMP },
            grounded: false,
        }
    }

    fn watch(n: usize) -> ReplayWatch {
        let frames: Vec<ReplayFrame> = (0..n).map(frame).collect();
        let replay = Replay {
            header: ReplayHeader {
                format_version: REPLAY_FORMAT_VERSION,
                map: "surf_test".into(),
                track: TRACK_MAIN.into(),
                style: STYLE_MOMENTUM_SURF.into(),
                tick_interval: 0.015,
                time_secs: n as f32 * 0.015,
                frame_count: n as u32,
                recorded_at: 0,
                vars: ReplayVars::from_move_vars(&MoveVars::momentum_surf()),
                splits: vec![0.045, 0.09],
                input_semantics: "cmdProducesPose".into(),
            },
            frames,
        };
        let gb = graybox::surf_ramp_arena();
        let pick = ReplayPick {
            map: "surf_test".into(),
            path: PathBuf::from("/nonexistent.osxr"),
            label: "test".into(),
        };
        ReplayWatch::new(pick, replay, &gb.world)
    }

    #[test]
    fn real_time_advances_one_frame_per_tick_and_doubles_at_2x() {
        let mut w = watch(100);
        w.advance(0.015 * 10.0 + 1e-4);
        assert_eq!(w.frame_idx(), 10);
        w.rate_step(1);
        assert_eq!(w.rate(), 2.0);
        w.advance(0.015 * 10.0 + 1e-4);
        assert_eq!(w.frame_idx(), 30);
    }

    #[test]
    fn alpha_is_the_fraction_into_the_next_tick() {
        let mut w = watch(100);
        w.advance(0.015 * 0.5);
        assert_eq!(w.frame_idx(), 0);
        assert!((w.alpha - 0.5).abs() < 1e-3, "alpha {}", w.alpha);
        // The interpolated origin is halfway between frames 0 and 1.
        assert!((w.origin().x - 5.0).abs() < 1e-3);
        // And the clock reads frame time plus the fraction.
        assert!((w.time_secs() - 0.015 * 1.5).abs() < 1e-5);
    }

    #[test]
    fn the_end_of_the_stream_holds_paused_and_play_restarts() {
        let mut w = watch(5);
        w.advance(10.0);
        assert!(w.at_end());
        assert!(w.paused);
        assert_eq!(w.frame_idx(), 4);
        assert!((w.progress() - 1.0).abs() < 1e-3);
        w.toggle_pause();
        assert!(!w.paused);
        assert_eq!(w.frame_idx(), 0, "play from the end restarts");
    }

    #[test]
    fn seek_clamps_to_the_stream_and_keeps_the_pause_state() {
        let mut w = watch(20);
        w.paused = true;
        w.seek_secs(0.015 * 10.0);
        assert_eq!(w.frame_idx(), 9);
        assert!(w.paused);
        w.seek_by(100.0);
        assert_eq!(w.frame_idx(), 19);
        w.seek_by(-100.0);
        assert_eq!(w.frame_idx(), 0);
        w.seek_frac(0.5);
        assert_eq!(w.frame_idx(), 9);
    }

    #[test]
    fn stepping_pauses_and_moves_exactly_one_frame() {
        let mut w = watch(10);
        w.advance(0.015 * 2.5);
        assert!(!w.paused);
        w.step(1);
        assert!(w.paused);
        assert_eq!(w.frame_idx(), 3);
        assert_eq!(w.alpha, 0.0);
        w.step(-1);
        w.step(-1);
        w.step(-1);
        w.step(-1);
        assert_eq!(w.frame_idx(), 0, "stepping back stops at the first frame");
        // Paused: wall time does nothing.
        w.advance(1.0);
        assert_eq!(w.frame_idx(), 0);
    }

    #[test]
    fn rate_is_clamped_to_the_table() {
        let mut w = watch(10);
        for _ in 0..20 {
            w.rate_step(1);
        }
        assert_eq!(w.rate(), *RATES.last().unwrap());
        for _ in 0..40 {
            w.rate_step(-1);
        }
        assert_eq!(w.rate(), RATES[0]);
        assert_eq!(w.transport_text(), "0.25x");
        w.paused = true;
        assert_eq!(w.transport_text(), "PAUSED");
    }

    /// A view that crosses ±180 between frames must turn 20°, not 340°.
    #[test]
    fn yaw_interpolates_the_short_way_round() {
        let mut w = watch(10);
        w.advance(0.015 * 0.5);
        let yaw = w.view_angles().yaw;
        assert!(
            (yaw.abs() - 180.0).abs() < 1e-3,
            "halfway between 170 and -170 should be ±180, got {yaw}"
        );
        assert_eq!(lerp_yaw(10.0, 20.0, 0.5), 15.0);
    }

    #[test]
    fn eye_height_follows_the_duck_button() {
        let mut w = watch(10);
        assert!((w.eye().0.z - (100.0 + 64.0)).abs() < 1e-3);
        w.seek_secs(0.015 * 4.0); // frame 3 is ducked
        assert_eq!(w.frame_idx(), 3);
        assert!(w.ducked());
        assert!((w.eye().0.z - (100.0 + 47.0)).abs() < 1e-3);
    }

    #[test]
    fn keys_come_from_the_recorded_inputs() {
        let w = watch(10);
        let (f, b, l, r, j) = w.keys();
        assert!(f && !b && l && !r && j);
    }

    #[test]
    fn splits_passed_counts_checkpoints_behind_the_clock() {
        let mut w = watch(20);
        assert_eq!(w.splits_passed(), 0);
        w.seek_secs(0.045);
        assert_eq!(w.splits_passed(), 1);
        w.seek_secs(1.0);
        assert_eq!(w.splits_passed(), 2);
    }

    #[test]
    fn a_run_row_without_a_file_offers_no_replay() {
        let rec = RunRecord {
            time_secs: 40.0,
            is_pb: false,
            finished_at: 0,
            replay_path: Some("/definitely/not/here.osxr".into()),
        };
        assert!(ReplayPick::run("surf_test", &rec, 1).is_none());
        let none = RunRecord {
            replay_path: None,
            ..rec
        };
        assert!(ReplayPick::run("surf_test", &none, 1).is_none());
        assert!(ReplayPick::pb("surf_no_such_map_zzzz").is_none());
    }

    /// The shipped summit ghosts are real data: the record the leaderboard
    /// lists resolves to the file the viewer would open.
    #[test]
    fn summit_ksf_records_resolve_to_their_replay_files() {
        let recs = leaderboard::ksf_records("surf_summit");
        if recs.is_empty() {
            eprintln!("summit manifest absent; skipping");
            return;
        }
        let picks: Vec<ReplayPick> = recs
            .iter()
            .filter_map(|r| ReplayPick::ksf("surf_summit", r))
            .collect();
        assert!(!picks.is_empty(), "no summit record has a replay on disk");
        assert!(picks[0].label.starts_with("KSF #1 "), "{}", picks[0].label);
        let replay = Replay::load(&picks[0].path).expect("load");
        assert_eq!(replay.header.map, "surf_summit");
    }
}
