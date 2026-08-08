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
    /// Checkpoint split times for the current / finished run (ordered).
    pub splits: Vec<f32>,
    /// Latest split event (consumed by app for HUD flash).
    pub last_split: Option<SplitEvent>,
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
            splits: Vec::new(),
            last_split: None,
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
        self.splits.clear();
        self.last_split = None;
        self.was_in_start = false;
        self.soft_enter_start = false;
        self.next_cp = 0;
        self.cp_inside.clear();
    }

    /// Fail teleport / kill-z snapped the player (possibly into start).
    /// Linear maps keep the clock; do not treat this as voluntary start re-entry.
    pub fn notify_soft_respawn(&mut self) {
        if matches!(self.phase, TimerPhase::Running | TimerPhase::Finished) {
            self.soft_enter_start = true;
        }
    }

    /// Per-tick update after physics. May clamp start-zone ground speed on `player`.
    pub fn tick(
        &mut self,
        zones: &MapZones,
        player: &mut PlayerState,
        dt: f32,
        pb_splits: &[f32],
    ) {
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
                    self.splits.clear();
                    self.next_cp = 0;
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
                // start (fail TP / kill-z → td_mapstart) must keep Running.
                if in_start && !self.was_in_start && !soft {
                    self.phase = TimerPhase::Armed;
                    self.time_secs = 0.0;
                    self.splits.clear();
                    self.next_cp = 0;
                    self.last_split = None;
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
                    self.splits.clear();
                    self.next_cp = 0;
                    self.last_split = None;
                }
            }
        }

        // Prespeed while standing in start (armed or after fail soft-enter).
        if in_start && matches!(self.phase, TimerPhase::Armed | TimerPhase::Running) {
            apply_prespeed(player, track.limit_start_ground_speed);
        }

        self.was_in_start = in_start;
        self.was_grounded = player.grounded;
    }

    fn start_run(&mut self, cp_count: usize) {
        self.phase = TimerPhase::Running;
        self.time_secs = 0.0;
        self.splits.clear();
        self.last_split = None;
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

/// HUD line for a checkpoint split flash.
pub fn format_split_line(ev: SplitEvent) -> String {
    let t = format_time(ev.time_secs);
    match ev.delta_vs_pb {
        Some(d) => format!("CP{} {t}  {}", ev.index, format_pb_delta(d)),
        None => format!("CP{} {t}", ev.index),
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
    fn format_time_basic() {
        assert_eq!(format_time(0.0), "0.000");
        assert_eq!(format_time(12.3456), "12.346");
        assert_eq!(format_time(72.5), "1:12.500");
    }
}
