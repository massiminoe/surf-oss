//! Speedrun timer state machine (start leave / end touch / checkpoint splits).

use surf_core::movement::PlayerState;

use crate::zones::{MapZones, TrackType, ZoneBox};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerPhase {
    /// Outside start; not timing.
    Idle,
    /// Inside start zone; armed, not yet timing.
    Armed,
    /// Clock running.
    Running,
    /// Touched end; time frozen.
    Finished,
}

impl TimerPhase {
    /// Stable id for on-disk loc files.
    pub fn as_str(self) -> &'static str {
        match self {
            TimerPhase::Idle => "idle",
            TimerPhase::Armed => "armed",
            TimerPhase::Running => "running",
            TimerPhase::Finished => "finished",
        }
    }

    pub fn from_str_or_idle(s: &str) -> Self {
        match s {
            "armed" => TimerPhase::Armed,
            "running" => TimerPhase::Running,
            "finished" => TimerPhase::Finished,
            _ => TimerPhase::Idle,
        }
    }
}

/// Restorable slice of run state — what a saveloc captures and a loadloc puts
/// back. Edge-detect flags are deliberately absent: they are re-derived from the
/// world on restore, so a snapshot can never resurrect a stale edge.
#[derive(Clone, Debug)]
pub struct TimerSnapshot {
    pub phase: TimerPhase,
    pub time_secs: f32,
    pub current_stage: usize,
    pub splits: Vec<f32>,
    pub next_cp: usize,
}

/// Just-hit checkpoint info for HUD flash.
#[derive(Clone, Copy, Debug)]
pub struct SplitEvent {
    /// 1-based checkpoint index.
    pub index: usize,
    pub time_secs: f32,
    /// `time - pb_split` when a PB split exists for this index.
    pub delta_vs_pb: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct RunTimer {
    pub phase: TimerPhase,
    /// Elapsed seconds while Running / frozen Finished time.
    pub time_secs: f32,
    pub track_type: TrackType,
    /// 1-based current stage (staged maps only; always 1 on linear).
    pub current_stage: usize,
    /// Checkpoint split times for the current / finished run (ordered).
    pub splits: Vec<f32>,
    /// Latest split event (consumed by app for HUD flash).
    pub last_split: Option<SplitEvent>,
    /// Set when the run was interfered with (loadloc). The clock keeps running
    /// and the HUD keeps showing a time, but the result is not a real run: no PB,
    /// no replay. Cleared only by a genuine start (`start_run`) or a reset.
    pub practice: bool,
    was_in_start: bool,
    was_grounded: bool,
    /// Set by fail teleport / kill-z soft-respawn into start: do not cancel.
    soft_enter_start: bool,
    /// Index of next checkpoint to accept (ordered).
    next_cp: usize,
    /// Whether currently overlapping each checkpoint (edge detect).
    cp_inside: Vec<bool>,
}

impl Default for RunTimer {
    fn default() -> Self {
        Self {
            phase: TimerPhase::Idle,
            time_secs: 0.0,
            track_type: TrackType::Linear,
            current_stage: 1,
            splits: Vec::new(),
            last_split: None,
            practice: false,
            was_in_start: false,
            was_grounded: true,
            soft_enter_start: false,
            next_cp: 0,
            cp_inside: Vec::new(),
        }
    }
}

impl RunTimer {
    pub fn new(track_type: TrackType) -> Self {
        Self {
            track_type,
            ..Self::default()
        }
    }

    /// Full restart (R): clear run.
    pub fn reset(&mut self) {
        self.phase = TimerPhase::Idle;
        self.time_secs = 0.0;
        self.current_stage = 1;
        self.splits.clear();
        self.last_split = None;
        self.practice = false;
        self.was_in_start = false;
        self.soft_enter_start = false;
        self.next_cp = 0;
        self.cp_inside.clear();
    }

    /// Capture the run state a loc should remember.
    pub fn snapshot(&self) -> TimerSnapshot {
        TimerSnapshot {
            phase: self.phase,
            time_secs: self.time_secs,
            current_stage: self.current_stage,
            splits: self.splits.clone(),
            next_cp: self.next_cp,
        }
    }

    /// Put a snapshot back (loadloc). Does **not** set [`Self::practice`] —
    /// callers decide, because restoring a start-zone loc is just a respawn.
    ///
    /// `was_in_start` / `soft_enter_start` are set so the first tick after a
    /// restore can never read as voluntary start re-entry and cancel the run;
    /// the real edge is re-established on the tick after that.
    pub fn restore(&mut self, snap: &TimerSnapshot, grounded: bool) {
        self.phase = snap.phase;
        self.time_secs = snap.time_secs;
        self.current_stage = snap.current_stage.max(1);
        self.splits = snap.splits.clone();
        self.next_cp = snap.next_cp;
        self.last_split = None;
        self.cp_inside.clear();
        self.was_in_start = true;
        self.was_grounded = grounded;
        self.soft_enter_start = true;
    }

    /// Mark the current run as practice-only (loadloc).
    pub fn mark_practice(&mut self) {
        self.practice = true;
    }

    /// Re-derive checkpoint overlap from where the player actually is, so a loc
    /// restored *inside* a checkpoint box doesn't immediately re-fire its split.
    pub fn sync_checkpoints_after_restore(&mut self, zones: &MapZones, player: &PlayerState) {
        let cps = &zones.main.checkpoints;
        let hull = player.hull();
        self.cp_inside = cps
            .iter()
            .map(|cp| cp.contains_player(player.origin, hull))
            .collect();
    }

    /// Does a soft respawn that landed the player `in_start` end the attempt?
    ///
    /// Fail teleports and kill-z respawns are one mechanism serving two very
    /// different situations. On a **staged** track, being put back at a stage
    /// start mid-run is normal and the clock must keep going. On a **linear**
    /// track it means you wiped: the run is dead, and keeping the clock alive
    /// only means the player has to press R before every retry.
    ///
    /// The test is deliberately narrow — the respawn has to land *inside the
    /// start zone* — so a mid-course teleport that is part of the intended
    /// route is untouched.
    pub fn wipe_ends_run(&self, in_start: bool) -> bool {
        self.track_type != TrackType::Staged && in_start
    }

    /// Fail teleport / kill-z snapped the player (possibly into a stage start).
    /// Soft-respawn keeps the clock; do not treat this as voluntary start re-entry.
    pub fn notify_soft_respawn(&mut self) {
        if matches!(self.phase, TimerPhase::Running | TimerPhase::Finished) {
            self.soft_enter_start = true;
        }
    }

    /// After a soft snap, sync `current_stage` if the player landed in a stage zone.
    pub fn sync_stage_after_respawn(&mut self, zones: &MapZones, player: &PlayerState) {
        if self.track_type != TrackType::Staged {
            return;
        }
        if let Some(s) = zones.main.stage_containing(player.origin, player.hull()) {
            self.current_stage = s;
        }
    }

    /// Per-tick update after physics. May clamp start-zone ground speed on `player`.
    pub fn tick(&mut self, zones: &MapZones, player: &mut PlayerState, dt: f32, pb_splits: &[f32]) {
        let hull = player.hull();
        let track = &zones.main;
        let in_start = track.start.contains_player(player.origin, hull);
        let in_end = track.end.contains_player(player.origin, hull);
        let soft = self.soft_enter_start;
        self.soft_enter_start = false;

        match self.phase {
            TimerPhase::Idle => {
                if in_start {
                    self.phase = TimerPhase::Armed;
                    self.time_secs = 0.0;
                    self.current_stage = 1;
                    self.splits.clear();
                    self.next_cp = 0;
                    self.practice = false;
                }
            }
            TimerPhase::Armed => {
                if !in_start {
                    self.start_run(track.checkpoints.len());
                    self.time_secs += dt;
                    self.poll_checkpoints(&track.checkpoints, player, pb_splits);
                } else if track.start_on_jump && self.was_grounded && !player.grounded {
                    self.start_run(track.checkpoints.len());
                    self.time_secs += dt;
                    self.poll_checkpoints(&track.checkpoints, player, pb_splits);
                }
            }
            TimerPhase::Running => {
                self.time_secs += dt;
                // Cancel on voluntary re-entry from outside. Soft-respawn into
                // start (fail TP / kill-z → stage/map start) must keep Running.
                if in_start && !self.was_in_start && !soft {
                    self.phase = TimerPhase::Armed;
                    self.time_secs = 0.0;
                    self.current_stage = 1;
                    self.splits.clear();
                    self.next_cp = 0;
                    self.last_split = None;
                    self.practice = false;
                } else if in_end && !in_start {
                    self.phase = TimerPhase::Finished;
                } else {
                    self.poll_checkpoints(&track.checkpoints, player, pb_splits);
                }
            }
            TimerPhase::Finished => {
                // Stay finished until R / voluntary re-enter start.
                if in_start && !soft {
                    self.phase = TimerPhase::Armed;
                    self.time_secs = 0.0;
                    self.current_stage = 1;
                    self.splits.clear();
                    self.next_cp = 0;
                    self.last_split = None;
                    self.practice = false;
                }
            }
        }

        // Prespeed: linear = main start; staged = current stage zone.
        let prespeed_zone = if self.track_type == TrackType::Staged {
            track.stage_zone(self.current_stage)
        } else {
            &track.start
        };
        if prespeed_zone.contains_player(player.origin, hull)
            && matches!(self.phase, TimerPhase::Armed | TimerPhase::Running)
        {
            apply_prespeed(player, track.limit_start_ground_speed);
        }

        self.was_in_start = in_start;
        self.was_grounded = player.grounded;
    }

    fn start_run(&mut self, cp_count: usize) {
        self.phase = TimerPhase::Running;
        self.time_secs = 0.0;
        self.current_stage = 1;
        self.splits.clear();
        self.last_split = None;
        self.practice = false;
        self.next_cp = 0;
        self.cp_inside = vec![false; cp_count];
    }

    fn poll_checkpoints(
        &mut self,
        checkpoints: &[ZoneBox],
        player: &PlayerState,
        pb_splits: &[f32],
    ) {
        if self.next_cp >= checkpoints.len() {
            return;
        }
        if self.cp_inside.len() != checkpoints.len() {
            self.cp_inside = vec![false; checkpoints.len()];
        }
        let hull = player.hull();
        let i = self.next_cp;
        let now = checkpoints[i].contains_player(player.origin, hull);
        if now && !self.cp_inside[i] {
            let t = self.time_secs;
            self.splits.push(t);
            let delta = pb_splits.get(i).map(|pb| t - pb);
            self.last_split = Some(SplitEvent {
                index: i + 1,
                time_secs: t,
                delta_vs_pb: delta,
            });
            self.next_cp += 1;
            // Entering checkpoint i (0-based) means arriving at stage i+2.
            if self.track_type == TrackType::Staged {
                self.current_stage = i + 2;
            }
        }
        self.cp_inside[i] = now;
        // Keep other inside flags updated so re-entry after skip isn't weird.
        for (j, cp) in checkpoints.iter().enumerate() {
            if j != i {
                self.cp_inside[j] = cp.contains_player(player.origin, hull);
            }
        }
    }

    pub fn display_time(&self) -> Option<f32> {
        match self.phase {
            TimerPhase::Idle => None,
            TimerPhase::Armed => Some(0.0),
            TimerPhase::Running | TimerPhase::Finished => Some(self.time_secs),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.phase == TimerPhase::Finished
    }

    pub fn take_split_event(&mut self) -> Option<SplitEvent> {
        self.last_split.take()
    }

    /// `STAGE k/n` while armed/running/finished on staged maps.
    pub fn stage_hud_label(&self, zones: &MapZones) -> Option<String> {
        if self.track_type != TrackType::Staged {
            return None;
        }
        if matches!(self.phase, TimerPhase::Idle) {
            return None;
        }
        let n = zones.main.stage_count().max(1);
        let k = self.current_stage.clamp(1, n);
        Some(format!("STAGE {k}/{n}"))
    }
}

/// Clamp horizontal ground speed inside the start zone (surf convention 350).
pub fn apply_prespeed(player: &mut PlayerState, limit: f32) {
    if !player.grounded || limit <= 0.0 {
        return;
    }
    let speed = player.velocity.length_2d();
    if speed > limit {
        let scale = limit / speed;
        player.velocity.x *= scale;
        player.velocity.y *= scale;
    }
}

/// Format seconds as `M:SS.mmm` (or `SS.mmm` under a minute).
pub fn format_time(secs: f32) -> String {
    let ms_total = (secs.max(0.0) * 1000.0).round() as u32;
    let millis = ms_total % 1000;
    let total_secs = ms_total / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if minutes > 0 {
        format!("{minutes}:{seconds:02}.{millis:03}")
    } else {
        format!("{seconds}.{millis:03}")
    }
}

/// Format PB delta: negative = ahead of PB.
pub fn format_pb_delta(delta: f32) -> String {
    let sign = if delta < 0.0 { '-' } else { '+' };
    let abs = format_time(delta.abs());
    format!("{sign}{abs}")
}

/// HUD line for a checkpoint / stage split flash.
pub fn format_split_line(ev: SplitEvent, staged: bool) -> String {
    let t = format_time(ev.time_secs);
    let label = if staged {
        // Split index 1 = arriving at stage 2.
        format!("S{}", ev.index + 1)
    } else {
        format!("CP{}", ev.index)
    };
    match ev.delta_vs_pb {
        Some(d) => format!("{label} {t}  {}", format_pb_delta(d)),
        None => format!("{label} {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::parse_zones_json;
    use surf_core::math::Vec3;
    use surf_core::movement::PlayerState;

    fn sample_zones() -> MapZones {
        parse_zones_json(
            r#"{
              "formatVersion": 1,
              "mapName": "t",
              "trackType": "linear",
              "tracks": {
                "main": {
                  "limitStartGroundSpeed": 350.0,
                  "startOnJump": true,
                  "start": { "mins": [0, 0, 0], "maxs": [100, 100, 80] },
                  "end": { "mins": [500, 0, 0], "maxs": [600, 100, 80] },
                  "checkpoints": [
                    { "mins": [200, 0, 0], "maxs": [220, 100, 80] },
                    { "mins": [350, 0, 0], "maxs": [370, 100, 80] }
                  ]
                }
              }
            }"#,
        )
        .unwrap()
    }

    fn player_at(x: f32, z: f32, grounded: bool) -> PlayerState {
        PlayerState {
            origin: Vec3::new(x, 50.0, z),
            grounded,
            ..PlayerState::default()
        }
    }

    #[test]
    fn start_leave_end_finish() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let dt = 0.015;

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);
        assert_eq!(timer.display_time(), Some(0.0));

        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!((timer.time_secs - dt).abs() < 1e-4);

        for _ in 0..10 {
            timer.tick(&zones, &mut p, dt, &[]);
        }
        let t = timer.time_secs;
        assert!((t - 11.0 * dt).abs() < 1e-4);

        p = player_at(550.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Finished);
        let frozen = timer.time_secs;
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.time_secs, frozen);
    }

    /// A wipe on a linear map ends the attempt, and the very next tick re-arms
    /// so you can just go again. Before this, the clock survived a fail
    /// teleport and every retry needed a manual R.
    #[test]
    fn a_wipe_back_into_the_start_zone_ends_a_linear_run_and_rearms() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let dt = 0.015;

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        p = player_at(150.0, 0.0, false);
        for _ in 0..40 {
            timer.tick(&zones, &mut p, dt, &[]);
        }
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(timer.time_secs > 0.5);

        // The map yanks us back to the start platform, as a fail teleport does.
        p = player_at(50.0, 0.0, true);
        let in_start = zones.main.start.contains_player(p.origin, p.hull());
        assert!(in_start, "test fixture: respawn must land in the start box");

        // Control — the old behaviour, which is what made R mandatory: treat the
        // wipe as a soft respawn and the clock sails straight past it.
        {
            let mut old = timer.clone();
            let mut q = p.clone();
            old.notify_soft_respawn();
            old.tick(&zones, &mut q, dt, &[]);
            assert_eq!(old.phase, TimerPhase::Running);
            assert!(old.time_secs > 0.5, "old path kept the clock, as expected");
        }

        assert!(
            timer.wipe_ends_run(in_start),
            "a linear wipe must end the run"
        );
        timer.reset();

        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed, "should re-arm immediately");
        assert_eq!(
            timer.display_time(),
            Some(0.0),
            "clock must be back to zero"
        );
        assert!(timer.splits.is_empty());
    }

    /// The same mechanism on a staged map must NOT end the run — being put back
    /// at a stage start mid-run is how staged maps work.
    #[test]
    fn a_staged_respawn_into_the_start_zone_keeps_the_run() {
        let mut timer = RunTimer::new(TrackType::Staged);
        assert!(!timer.wipe_ends_run(true));
        timer.phase = TimerPhase::Running;
        timer.time_secs = 12.0;
        timer.notify_soft_respawn();
        assert_eq!(timer.phase, TimerPhase::Running);
        assert_eq!(timer.time_secs, 12.0);
    }

    /// A teleport that lands somewhere other than the start box is part of the
    /// route, not a wipe.
    #[test]
    fn a_mid_course_teleport_does_not_end_a_linear_run() {
        let timer = RunTimer::new(TrackType::Linear);
        assert!(!timer.wipe_ends_run(false));
    }

    #[test]
    fn checkpoint_splits_ordered_with_pb_delta() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let dt = 0.015;
        let pb = [0.5_f32, 1.0];

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &pb);
        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &pb);

        // Reach CP1.
        p = player_at(210.0, 0.0, false);
        for _ in 0..20 {
            timer.tick(&zones, &mut p, dt, &pb);
        }
        let ev = timer.take_split_event().expect("cp1");
        assert_eq!(ev.index, 1);
        assert_eq!(timer.splits.len(), 1);
        assert!(ev.delta_vs_pb.is_some());

        // Reach CP2.
        p = player_at(360.0, 0.0, false);
        for _ in 0..40 {
            timer.tick(&zones, &mut p, dt, &pb);
        }
        let ev2 = timer.take_split_event().expect("cp2");
        assert_eq!(ev2.index, 2);
        assert_eq!(timer.splits.len(), 2);
    }

    #[test]
    fn start_on_jump_starts_while_inside() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);

        p.grounded = false;
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(timer.time_secs > 0.0);
    }

    #[test]
    fn leave_only_jump_inside_stays_armed() {
        let mut zones = sample_zones();
        zones.main.start_on_jump = false;
        let mut timer = RunTimer::new(TrackType::Linear);
        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);

        p.grounded = false;
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);

        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
    }

    #[test]
    fn prespeed_clamps_ground_speed() {
        let mut p = player_at(50.0, 0.0, true);
        p.velocity = Vec3::new(400.0, 0.0, 0.0);
        apply_prespeed(&mut p, 350.0);
        assert!((p.velocity.length_2d() - 350.0).abs() < 1e-3);
    }

    #[test]
    fn soft_respawn_into_start_keeps_running() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        timer.phase = TimerPhase::Running;
        timer.time_secs = 32.0;
        timer.was_in_start = false;
        timer.cp_inside = vec![false; 2];

        timer.notify_soft_respawn();
        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!((timer.time_secs - 32.015).abs() < 1e-4);
    }

    #[test]
    fn voluntary_reenter_start_cancels() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        timer.phase = TimerPhase::Running;
        timer.time_secs = 32.0;
        timer.was_in_start = false;
        timer.splits = vec![1.0];

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);
        assert_eq!(timer.time_secs, 0.0);
        assert!(timer.splits.is_empty());
    }

    #[test]
    fn summit_fail_to_mapstart_keeps_clock() {
        let path = std::path::Path::new("assets/zones/surf_summit.json");
        if !path.is_file() {
            return;
        }
        let zones = crate::zones::load_zones_file(path).expect("summit zones");
        let mut timer = RunTimer::new(TrackType::Linear);
        timer.phase = TimerPhase::Running;
        timer.time_secs = 33.0;
        timer.was_in_start = false;
        timer.cp_inside = vec![false; zones.main.checkpoints.len()];
        timer.notify_soft_respawn();
        let mut p = PlayerState {
            origin: Vec3::new(1600.0, 0.0, 11552.0),
            grounded: true,
            ..PlayerState::default()
        };
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(timer.time_secs > 33.0);
    }

    #[test]
    fn restored_loc_keeps_timing_but_stays_practice_through_the_finish() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let dt = 0.015;

        // A real run, mid-flight.
        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(!timer.practice);
        let snap = timer.snapshot();

        // Wipe out, then loadloc back to the snapshot.
        timer.reset();
        timer.restore(&snap, false);
        timer.mark_practice();
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!((timer.time_secs - snap.time_secs).abs() < 1e-6);

        // The clock is real…
        timer.tick(&zones, &mut p, dt, &[]);
        assert!(timer.time_secs > snap.time_secs);
        // …the run is not, all the way through the finish.
        p = player_at(550.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Finished);
        assert!(timer.practice);

        // Re-entering start clears the taint, so the next run counts.
        p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);
        assert!(!timer.practice);
    }

    #[test]
    fn loc_saved_in_start_zone_still_produces_an_official_run() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let dt = 0.015;
        let snap = TimerSnapshot {
            phase: TimerPhase::Armed,
            time_secs: 0.0,
            current_stage: 1,
            splits: Vec::new(),
            next_cp: 0,
        };
        timer.restore(&snap, true);
        timer.mark_practice();

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);
        assert!(timer.practice);

        // Leaving start is a genuine start: the taint goes with it.
        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(!timer.practice);
    }

    #[test]
    fn restore_inside_start_does_not_cancel_the_run() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let snap = TimerSnapshot {
            phase: TimerPhase::Running,
            time_secs: 12.0,
            current_stage: 1,
            splits: Vec::new(),
            next_cp: 0,
        };
        timer.restore(&snap, true);
        // Loc sits inside the start box (prespeed practice) — the first tick
        // must not read as voluntary re-entry.
        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(timer.time_secs > 12.0);
    }

    #[test]
    fn restoring_inside_a_checkpoint_does_not_refire_its_split() {
        let zones = sample_zones();
        let mut timer = RunTimer::new(TrackType::Linear);
        let snap = TimerSnapshot {
            phase: TimerPhase::Running,
            time_secs: 8.0,
            current_stage: 1,
            splits: Vec::new(),
            next_cp: 0,
        };
        // Standing inside CP1 at the moment of the load.
        let mut p = player_at(210.0, 0.0, false);
        timer.restore(&snap, false);
        timer.sync_checkpoints_after_restore(&zones, &p);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert!(timer.splits.is_empty());
        assert!(timer.last_split.is_none());

        // Leaving and re-entering does split.
        p = player_at(300.0, 0.0, false);
        timer.tick(&zones, &mut p, 0.015, &[]);
        p = player_at(210.0, 0.0, false);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.splits.len(), 1);
    }

    #[test]
    fn format_time_basic() {
        assert_eq!(format_time(0.0), "0.000");
        assert_eq!(format_time(12.3456), "12.346");
        assert_eq!(format_time(72.5), "1:12.500");
    }

    fn staged_zones() -> MapZones {
        let mut z = sample_zones();
        z.track_type = TrackType::Staged;
        z.main.start_on_jump = false;
        z
    }

    #[test]
    fn staged_advances_stage_on_checkpoint() {
        let zones = staged_zones();
        let mut timer = RunTimer::new(TrackType::Staged);
        let dt = 0.015;

        let mut p = player_at(50.0, 0.0, true);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Armed);
        assert_eq!(timer.current_stage, 1);
        assert_eq!(timer.stage_hud_label(&zones).as_deref(), Some("STAGE 1/3"));

        p = player_at(150.0, 0.0, false);
        timer.tick(&zones, &mut p, dt, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert_eq!(timer.current_stage, 1);

        p = player_at(210.0, 0.0, false);
        for _ in 0..20 {
            timer.tick(&zones, &mut p, dt, &[]);
        }
        assert_eq!(timer.current_stage, 2);
        let ev = timer.take_split_event().expect("stage split");
        assert_eq!(
            format_split_line(ev, true),
            format!("S2 {}", format_time(ev.time_secs))
        );
    }

    #[test]
    fn staged_soft_respawn_syncs_stage_and_prespeed() {
        let zones = staged_zones();
        let mut timer = RunTimer::new(TrackType::Staged);
        timer.phase = TimerPhase::Running;
        timer.time_secs = 10.0;
        timer.current_stage = 2;
        timer.next_cp = 1;
        timer.cp_inside = vec![false; 2];
        timer.was_in_start = false;

        // Kill-z style snap into stage-2 zone center.
        let spawn = zones.main.stage_spawn(2);
        let mut p = PlayerState {
            origin: spawn,
            velocity: Vec3::new(500.0, 0.0, 0.0),
            grounded: true,
            ..PlayerState::default()
        };
        timer.notify_soft_respawn();
        timer.sync_stage_after_respawn(&zones, &p);
        assert_eq!(timer.current_stage, 2);
        timer.tick(&zones, &mut p, 0.015, &[]);
        assert_eq!(timer.phase, TimerPhase::Running);
        assert!(timer.time_secs > 10.0);
        // Prespeed in stage zone.
        assert!((p.velocity.length_2d() - 350.0).abs() < 1e-3);
    }

    #[test]
    fn staged_stage_spawn_is_box_floor_center() {
        let zones = staged_zones();
        let o = zones.main.stage_spawn(1);
        assert!((o.x - 50.0).abs() < 1e-3);
        assert!((o.y - 50.0).abs() < 1e-3);
        assert!((o.z - 0.0).abs() < 1e-3);
        let o2 = zones.main.stage_spawn(2);
        assert!((o2.x - 210.0).abs() < 1e-3);
    }
}
