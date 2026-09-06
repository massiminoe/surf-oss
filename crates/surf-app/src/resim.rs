//! Soft KSF / native replay re-sim against a loaded map (teleports + kill-z).
//!
//! Layer A probes recorded poses for ramp contact / fall-through / burial.
//! Layer B feeds stored inputs through `tick` under a MoveVars preset grid.
//! Primary B signal is short-horizon **windowed** re-sim on on-ramp segments;
//! open-loop full-run remains a diagnostic only.

use serde::{Deserialize, Deserializer, Serialize};
use surf_core::brush::World;
use surf_core::is_on_surf_ramp;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState};
use surf_core::tick;
use surf_core::trace::{point_contents_box, trace_box, TraceHit};
use surf_map::{FieldState, LoadedMap};

use crate::replay::ReplayFrame;
use crate::zones::{MapZones, ZoneBox};

fn deserialize_f32_null_as_zero<'de, D: Deserializer<'de>>(de: D) -> Result<f32, D::Error> {
    Ok(Option::<f32>::deserialize(de)?.unwrap_or(0.0))
}

/// Short-horizon lengths (ticks @ 66.67 Hz ≈ 0.45s / 1s / 2s).
pub const WINDOW_HORIZONS: &[usize] = &[30, 66, 132];

/// Minimum contiguous on-ramp run length to seed a window (~0.6s).
pub const MIN_RAMP_SEGMENT_LEN: usize = 40;

/// Cap evaluated windows per ghost so audits stay cheap.
pub const MAX_WINDOWS: usize = 32;

/// Original v1 KSF harness slice (leave-zone, no push/staged deps).
pub const V1_MAPS: &[&str] = &[
    "surf_summit",
    "surf_boreas",
    "surf_tendies",
    "surf_andromeda",
];

/// Wave 1 soft-audit expand: remaining 0-push linear maps with zones + KSF ghosts.
pub const WAVE1_MAPS: &[&str] = &[
    "surf_void",
    "surf_lux",
    "surf_aquaflow",
    "surf_demise",
    "surf_fornax",
    "surf_lovetunnel",
];

/// Default soft-audit map list — all 18 zoned maps (linear + push + staged).
pub const SOFT_AUDIT_MAPS: &[&str] = &[
    "surf_summit",
    "surf_boreas",
    "surf_tendies",
    "surf_andromeda",
    "surf_void",
    "surf_lux",
    "surf_aquaflow",
    "surf_demise",
    "surf_fornax",
    "surf_lovetunnel",
    "surf_frost",
    "surf_cyberwave",
    "surf_cement",
    "surf_botanica",
    "surf_overgrowth",
];

/// Summit WR ghost filename (KSF rank 1).
pub const SUMMIT_WR_OSXR: &str =
    "replay_css_2946_0_712551_1745965307.osxr";

/// Named MoveVars presets for the Layer B grid.
#[derive(Clone, Copy, Debug)]
pub struct NamedPreset {
    pub name: &'static str,
    pub vars: MoveVars,
}

/// Discrete preset grid: named presets + aa×accel with fixes on.
pub fn preset_grid() -> Vec<NamedPreset> {
    let mut out = vec![
        NamedPreset {
            name: "momentum_surf",
            vars: MoveVars::momentum_surf(),
        },
        NamedPreset {
            name: "ksf_css_66t",
            vars: MoveVars::ksf_css_66t(),
        },
        NamedPreset {
            name: "ksf_css_stock",
            vars: MoveVars::ksf_css_stock(),
        },
    ];
    // Fill aa ∈ {100,150} × accel ∈ {5,10} with fixes on (skip duplicates).
    for aa in [100.0_f32, 150.0] {
        for accel in [5.0_f32, 10.0] {
            let name = match (aa as i32, accel as i32) {
                (150, 5) => continue,  // momentum_surf
                (100, 10) => continue, // ksf_css_66t
                (100, 5) => "aa100_accel5",
                (150, 10) => "aa150_accel10",
                _ => unreachable!(),
            };
            let mut vars = MoveVars::momentum_surf();
            vars.airaccelerate = aa;
            vars.accelerate = accel;
            out.push(NamedPreset { name, vars });
        }
    }
    out
}

pub fn preset_by_name(name: &str) -> Option<NamedPreset> {
    preset_grid().into_iter().find(|p| p.name == name)
}

/// Apply per-map `maxVelocity` from zones onto a preset.
pub fn vars_for_map(base: &MoveVars, zones: &MapZones) -> MoveVars {
    let mut v = *base;
    if let Some(mv) = zones.max_velocity {
        v.maxvelocity = mv;
    }
    v
}

// ---------------------------------------------------------------------------
// Layer A — pose probes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PoseAudit {
    pub frames: usize,
    pub on_ramp: usize,
    pub kill_z_hits: usize,
    pub buried: usize,
}

impl PoseAudit {
    pub fn on_ramp_frac(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            self.on_ramp as f32 / self.frames as f32
        }
    }

    pub fn kill_z_frac(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            self.kill_z_hits as f32 / self.frames as f32
        }
    }

    pub fn buried_frac(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            self.buried as f32 / self.frames as f32
        }
    }
}

/// Probe recorded poses: ramp contact, kill-z, startsolid burial.
pub fn audit_poses(map: &LoadedMap, frames: &[ReplayFrame]) -> PoseAudit {
    let hull = Hull::css_stand();
    let mut on_ramp = 0usize;
    let mut kill_z_hits = 0usize;
    let mut buried = 0usize;
    for f in frames {
        if is_on_surf_ramp(&map.world, f.origin, &hull) {
            on_ramp += 1;
        }
        if f.origin.z < map.kill_z {
            kill_z_hits += 1;
        }
        // Slight lift so seam grazing on ramps doesn't count as buried.
        let probe = f.origin + Vec3::new(0.0, 0.0, 2.0);
        if point_contents_box(&map.world, probe, hull.mins, hull.maxs) {
            buried += 1;
        }
    }
    PoseAudit {
        frames: frames.len(),
        on_ramp,
        kill_z_hits,
        buried,
    }
}

/// Hard Layer A gates (floors/ceilings from baselines after calibration).
#[derive(Clone, Debug)]
pub struct PoseGates {
    pub min_on_ramp_frac: f32,
    pub max_kill_z_frac: f32,
    pub max_buried_frac: f32,
}

impl PoseGates {
    pub fn check(&self, a: &PoseAudit) -> Result<(), String> {
        let mut errs = Vec::new();
        let orf = a.on_ramp_frac();
        if orf < self.min_on_ramp_frac {
            errs.push(format!(
                "on-ramp frac {orf:.3} < floor {:.3}",
                self.min_on_ramp_frac
            ));
        }
        let kz = a.kill_z_frac();
        if kz > self.max_kill_z_frac {
            errs.push(format!(
                "kill-z frac {kz:.3} > ceil {:.3}",
                self.max_kill_z_frac
            ));
        }
        let bf = a.buried_frac();
        if bf > self.max_buried_frac {
            errs.push(format!(
                "buried frac {bf:.3} > ceil {:.3}",
                self.max_buried_frac
            ));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }
}

// ---------------------------------------------------------------------------
// Layer B — input re-sim
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ResimStats {
    pub preset: String,
    pub compared: usize,
    pub ghost_frames: usize,
    pub max_origin_err: f32,
    pub p95_origin_err: f32,
    pub mean_origin_err: f32,
    pub max_vel_err: f32,
    pub died_at: Option<usize>,
    pub teleports: usize,
    pub reached_end: bool,
}

impl ResimStats {
    pub fn survival_frac(&self) -> f32 {
        // frames[0] is seed; compared ticks are frames[1..]
        let target = self.ghost_frames.saturating_sub(1).max(1);
        self.compared as f32 / target as f32
    }

    pub fn catastrophic(&self) -> bool {
        // Dead before half the ghost while the recording continues.
        self.survival_frac() < 0.5
    }
}

fn percentile_sorted(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn player_from_frame(f: &ReplayFrame) -> PlayerState {
    PlayerState {
        origin: f.origin,
        velocity: f.velocity,
        viewangles: Angle::new(f.pitch, f.yaw, 0.0),
        grounded: f.grounded,
        ducked: f.buttons & 2 != 0, // BTN_DUCK
        old_jump: false,
        surface_friction: 1.0,
        ground_normal: Vec3::Z,
        ..PlayerState::default()
    }
}

/// Re-sim `frames[1..]` from `frames[0]` seed; apply teleports; stop on kill-z.
pub fn resim_map_run(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    vars: &MoveVars,
    end_zone: Option<&ZoneBox>,
) -> ResimStats {
    resim_map_run_named(map, frames, vars, "custom", end_zone)
}

pub fn resim_map_run_named(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    vars: &MoveVars,
    preset_name: &str,
    end_zone: Option<&ZoneBox>,
) -> ResimStats {
    let hull = Hull::css_stand();
    if frames.len() < 2 {
        return ResimStats {
            preset: preset_name.to_string(),
            compared: 0,
            ghost_frames: frames.len(),
            max_origin_err: 0.0,
            p95_origin_err: 0.0,
            mean_origin_err: 0.0,
            max_vel_err: 0.0,
            died_at: None,
            teleports: 0,
            reached_end: false,
        };
    }

    let mut player = player_from_frame(&frames[0]);
    let mut fields = FieldState::new();
    let mut errs: Vec<f32> = Vec::with_capacity(frames.len() - 1);
    let mut sum_err = 0.0_f32;
    let mut max_origin_err = 0.0_f32;
    let mut max_vel_err = 0.0_f32;
    let mut teleports = 0usize;
    let mut died_at = None;
    let mut reached_end = false;

    for (i, frame) in frames.iter().enumerate().skip(1) {
        player = tick(&map.world, &player, &frame.to_usercmd(), vars);
        map.apply_fields(&mut player, &mut fields, vars.tick_interval);

        if let Some((dest, angles)) = map.touch_teleport(player.origin) {
            player.origin = dest;
            player.viewangles = angles;
            teleports += 1;
        }

        if player.origin.z < map.kill_z {
            died_at = Some(i);
            break;
        }

        let oerr = (player.origin - frame.origin).length();
        let verr = (player.velocity - frame.velocity).length();
        errs.push(oerr);
        sum_err += oerr;
        max_origin_err = max_origin_err.max(oerr);
        max_vel_err = max_vel_err.max(verr);

        if let Some(end) = end_zone {
            if end.contains_player(player.origin, hull) {
                reached_end = true;
            }
        }
    }

    let compared = errs.len();
    let mean_origin_err = if compared == 0 {
        0.0
    } else {
        sum_err / compared as f32
    };
    let mut sorted = errs;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95_origin_err = percentile_sorted(&sorted, 0.95);

    ResimStats {
        preset: preset_name.to_string(),
        compared,
        ghost_frames: frames.len(),
        max_origin_err,
        p95_origin_err,
        mean_origin_err,
        max_vel_err,
        died_at,
        teleports,
        reached_end,
    }
}

/// Run the full preset grid; return all stats and the best by (survival, p95).
///
/// Open-loop diagnostic — prefer [`resim_windows_best_preset`] for Layer B gates.
pub fn resim_best_preset(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    zones: &MapZones,
    presets: &[NamedPreset],
) -> (Vec<ResimStats>, Option<ResimStats>) {
    let mut all = Vec::with_capacity(presets.len());
    for p in presets {
        let vars = vars_for_map(&p.vars, zones);
        all.push(resim_map_run_named(
            map,
            frames,
            &vars,
            p.name,
            Some(&zones.main.end),
        ));
    }
    let best = all
        .iter()
        .filter(|s| !s.catastrophic())
        .min_by(|a, b| {
            a.p95_origin_err
                .partial_cmp(&b.p95_origin_err)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
        .or_else(|| {
            all.iter()
                .min_by(|a, b| {
                    b.survival_frac()
                        .partial_cmp(&a.survival_frac())
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(
                            a.p95_origin_err
                                .partial_cmp(&b.p95_origin_err)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        )
                })
                .cloned()
        });
    (all, best)
}

// ---------------------------------------------------------------------------
// Layer B — windowed short-horizon re-sim (primary signal)
// ---------------------------------------------------------------------------
// Phase 2 (not built yet): once a worst window is stable (audit prints
// start_tick + maxErr), extract that ramp + WR slice into assets/fixtures/
// for a tight mini-world unit test. Do not add extract tooling until then.



/// Contiguous on-ramp run: `[start, end)` frame indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RampSegment {
    pub start: usize,
    pub end: usize,
}

impl RampSegment {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// One short-horizon window result.
#[derive(Clone, Debug)]
pub struct WindowStats {
    pub preset: String,
    pub start_tick: usize,
    pub horizon: usize,
    pub compared: usize,
    pub max_origin_err: f32,
    pub final_origin_err: f32,
    pub mean_origin_err: f32,
    pub max_vel_err: f32,
    pub died: bool,
    pub teleports: usize,
}

/// Aggregate over many windows at one horizon.
#[derive(Clone, Debug)]
pub struct HorizonAggregate {
    pub horizon: usize,
    pub n_windows: usize,
    /// Median of per-window max origin error.
    pub median_max_origin_err: f32,
    /// p95 of per-window max origin error.
    pub p95_max_origin_err: f32,
    /// Worst windows by max origin error (up to 5), descending.
    pub worst: Vec<WindowStats>,
}

/// Per-preset window sweep across [`WINDOW_HORIZONS`].
#[derive(Clone, Debug)]
pub struct WindowPresetResult {
    pub preset: String,
    pub starts: Vec<usize>,
    pub by_horizon: Vec<HorizonAggregate>,
}

impl WindowPresetResult {
    pub fn at_horizon(&self, h: usize) -> Option<&HorizonAggregate> {
        self.by_horizon.iter().find(|a| a.horizon == h)
    }

    pub fn median_at_66(&self) -> Option<f32> {
        self.at_horizon(66).map(|a| a.median_max_origin_err)
    }
}

/// Find contiguous on-ramp runs of at least `min_len` frames.
pub fn find_on_ramp_segments(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    min_len: usize,
) -> Vec<RampSegment> {
    let hull = Hull::css_stand();
    let mut out = Vec::new();
    let mut seg_start: Option<usize> = None;
    for (i, f) in frames.iter().enumerate() {
        let on = is_on_surf_ramp(&map.world, f.origin, &hull);
        match (on, seg_start) {
            (true, None) => seg_start = Some(i),
            (false, Some(s)) => {
                let end = i;
                if end.saturating_sub(s) >= min_len {
                    out.push(RampSegment { start: s, end });
                }
                seg_start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = seg_start {
        let end = frames.len();
        if end.saturating_sub(s) >= min_len {
            out.push(RampSegment { start: s, end });
        }
    }
    out
}

/// Pick up to `max_windows` segment starts: first, longest, then evenly spaced.
pub fn select_window_starts(segments: &[RampSegment], max_windows: usize) -> Vec<usize> {
    if segments.is_empty() || max_windows == 0 {
        return Vec::new();
    }
    if segments.len() <= max_windows {
        return segments.iter().map(|s| s.start).collect();
    }

    let mut chosen: Vec<usize> = Vec::with_capacity(max_windows);
    let push_unique = |idx: usize, chosen: &mut Vec<usize>| {
        let start = segments[idx].start;
        if !chosen.contains(&start) {
            chosen.push(start);
        }
    };

    // First segment.
    push_unique(0, &mut chosen);

    // Longest segment.
    let longest = segments
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.len())
        .map(|(i, _)| i)
        .unwrap_or(0);
    push_unique(longest, &mut chosen);

    // Evenly spaced across the remaining budget.
    let n = segments.len();
    let budget = max_windows.saturating_sub(chosen.len());
    if budget > 0 && n > 1 {
        for k in 0..budget {
            let idx = ((k + 1) * (n - 1)) / (budget + 1);
            push_unique(idx, &mut chosen);
            if chosen.len() >= max_windows {
                break;
            }
        }
    }

    // Fill any leftover from the front.
    for i in 0..n {
        if chosen.len() >= max_windows {
            break;
        }
        push_unique(i, &mut chosen);
    }

    chosen.sort_unstable();
    chosen.truncate(max_windows);
    chosen
}

/// Sudden sim speed collapse while the ghost keeps flying — the "1px wall" signal.
#[derive(Clone, Debug)]
pub struct SpeedSnag {
    pub tick: usize,
    pub sim_before: f32,
    pub sim_after: f32,
    pub ghost_speed: f32,
}

/// Scan window rows for ticks where sim XY speed drops below `keep_frac` of the
/// previous sim speed while still above `min_before`, and the ghost stays fast.
pub fn find_speed_snags(
    rows: &[WindowTickRow],
    min_before: f32,
    keep_frac: f32,
) -> Vec<SpeedSnag> {
    let mut out = Vec::new();
    let mut prev_sim = rows.first().map(|r| r.sim_speed).unwrap_or(0.0);
    for r in rows.iter().skip(1) {
        if prev_sim >= min_before
            && r.sim_speed < prev_sim * keep_frac
            && r.ghost_speed >= min_before * 0.8
        {
            out.push(SpeedSnag {
                tick: r.tick,
                sim_before: prev_sim,
                sim_after: r.sim_speed,
                ghost_speed: r.ghost_speed,
            });
        }
        prev_sim = r.sim_speed;
    }
    out
}

/// Per-tick row from [`resim_window_trace`].
#[derive(Clone, Debug)]
pub struct WindowTickRow {
    pub tick: usize,
    pub origin_err: f32,
    pub vel_err: f32,
    pub sim_origin: Vec3,
    pub ghost_origin: Vec3,
    pub sim_vel: Vec3,
    pub ghost_vel: Vec3,
    pub sim_speed: f32,
    pub ghost_speed: f32,
    pub sim_grounded: bool,
    pub ghost_grounded: bool,
    pub sim_on_ramp: bool,
    pub ghost_on_ramp: bool,
    pub teleported: bool,
    pub fwd: f32,
    pub side: f32,
    pub yaw: f32,
    /// First sweep hit along pre-tick velocity (diagnostic; may differ slightly from move).
    pub sweep_fraction: f32,
    pub sweep_hit: Option<TraceHit>,
}

/// Re-sim a window and return per-tick comparison rows (plus summary stats).
pub fn resim_window_trace(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    start_tick: usize,
    horizon: usize,
    vars: &MoveVars,
    preset_name: &str,
) -> (WindowStats, Vec<WindowTickRow>) {
    let empty = || {
        (
            WindowStats {
                preset: preset_name.to_string(),
                start_tick,
                horizon,
                compared: 0,
                max_origin_err: 0.0,
                final_origin_err: 0.0,
                mean_origin_err: 0.0,
                max_vel_err: 0.0,
                died: false,
                teleports: 0,
            },
            Vec::new(),
        )
    };
    if start_tick >= frames.len() || horizon == 0 {
        return empty();
    }

    let hull = Hull::css_stand();
    let end = (start_tick + horizon + 1).min(frames.len());
    let mut player = player_from_frame(&frames[start_tick]);
    // Inherit the one-shot gates the recorded run already tripped on its way
    // here — a blank state would re-arm a booster mid-window.
    let path: Vec<Vec3> = frames[..=start_tick].iter().map(|f| f.origin).collect();
    let mut fields = map.field_state_at(&path);
    let mut sum_err = 0.0_f32;
    let mut max_origin_err = 0.0_f32;
    let mut final_origin_err = 0.0_f32;
    let mut max_vel_err = 0.0_f32;
    let mut teleports = 0usize;
    let mut died = false;
    let mut compared = 0usize;
    let mut rows = Vec::with_capacity(horizon);

    for i in (start_tick + 1)..end {
        let frame = &frames[i];
        // Diagnostic sweep along pre-tick velocity (same hull as movement).
        let sweep_end = player.origin + player.velocity * vars.tick_interval;
        let sweep = trace_box(
            &map.world,
            player.origin,
            sweep_end,
            hull.mins,
            hull.maxs,
        );

        player = tick(&map.world, &player, &frame.to_usercmd(), vars);
        map.apply_fields(&mut player, &mut fields, vars.tick_interval);

        let mut teleported = false;
        if let Some((dest, angles)) = map.touch_teleport(player.origin) {
            player.origin = dest;
            player.viewangles = angles;
            teleports += 1;
            teleported = true;
        }

        if player.origin.z < map.kill_z {
            died = true;
            break;
        }

        let oerr = (player.origin - frame.origin).length();
        let verr = (player.velocity - frame.velocity).length();
        compared += 1;
        sum_err += oerr;
        max_origin_err = max_origin_err.max(oerr);
        final_origin_err = oerr;
        max_vel_err = max_vel_err.max(verr);

        rows.push(WindowTickRow {
            tick: i,
            origin_err: oerr,
            vel_err: verr,
            sim_origin: player.origin,
            ghost_origin: frame.origin,
            sim_vel: player.velocity,
            ghost_vel: frame.velocity,
            sim_speed: player.velocity.length_2d(),
            ghost_speed: frame.velocity.length_2d(),
            sim_grounded: player.grounded,
            ghost_grounded: frame.grounded,
            sim_on_ramp: is_on_surf_ramp(&map.world, player.origin, &hull),
            ghost_on_ramp: is_on_surf_ramp(&map.world, frame.origin, &hull),
            teleported,
            fwd: frame.forward_move,
            side: frame.side_move,
            yaw: frame.yaw,
            sweep_fraction: sweep.fraction,
            sweep_hit: sweep.hit,
        });
    }

    let mean_origin_err = if compared == 0 {
        0.0
    } else {
        sum_err / compared as f32
    };

    (
        WindowStats {
            preset: preset_name.to_string(),
            start_tick,
            horizon,
            compared,
            max_origin_err,
            final_origin_err,
            mean_origin_err,
            max_vel_err,
            died,
            teleports,
        },
        rows,
    )
}

/// Re-sim up to `horizon` ticks seeded at `start_tick`.
pub fn resim_window(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    start_tick: usize,
    horizon: usize,
    vars: &MoveVars,
    preset_name: &str,
) -> WindowStats {
    resim_window_trace(map, frames, start_tick, horizon, vars, preset_name).0
}

fn aggregate_horizon(horizon: usize, windows: Vec<WindowStats>) -> HorizonAggregate {
    let n = windows.len();
    let mut max_errs: Vec<f32> = windows.iter().map(|w| w.max_origin_err).collect();
    max_errs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_max_origin_err = percentile_sorted(&max_errs, 0.5);
    let p95_max_origin_err = percentile_sorted(&max_errs, 0.95);

    let mut worst = windows;
    worst.sort_by(|a, b| {
        b.max_origin_err
            .partial_cmp(&a.max_origin_err)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    worst.truncate(5);

    HorizonAggregate {
        horizon,
        n_windows: n,
        median_max_origin_err,
        p95_max_origin_err,
        worst,
    }
}

/// Window sweep for one preset over selected on-ramp starts.
pub fn resim_windows_named(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    vars: &MoveVars,
    preset_name: &str,
    starts: &[usize],
    horizons: &[usize],
) -> WindowPresetResult {
    let mut by_horizon = Vec::with_capacity(horizons.len());
    for &h in horizons {
        let windows: Vec<WindowStats> = starts
            .iter()
            .map(|&s| resim_window(map, frames, s, h, vars, preset_name))
            .collect();
        by_horizon.push(aggregate_horizon(h, windows));
    }
    WindowPresetResult {
        preset: preset_name.to_string(),
        starts: starts.to_vec(),
        by_horizon,
    }
}

/// Discover on-ramp windows and sweep the preset grid; best = lowest median max-err @ H=66.
pub fn resim_windows_best_preset(
    map: &LoadedMap,
    frames: &[ReplayFrame],
    zones: &MapZones,
    presets: &[NamedPreset],
) -> (Vec<RampSegment>, Vec<usize>, Vec<WindowPresetResult>, Option<WindowPresetResult>) {
    let segments = find_on_ramp_segments(map, frames, MIN_RAMP_SEGMENT_LEN);
    let starts = select_window_starts(&segments, MAX_WINDOWS);
    let mut all = Vec::with_capacity(presets.len());
    for p in presets {
        let vars = vars_for_map(&p.vars, zones);
        all.push(resim_windows_named(
            map,
            frames,
            &vars,
            p.name,
            &starts,
            WINDOW_HORIZONS,
        ));
    }
    let best = all
        .iter()
        .filter(|r| r.median_at_66().is_some())
        .min_by(|a, b| {
            a.median_at_66()
                .unwrap_or(f32::MAX)
                .partial_cmp(&b.median_at_66().unwrap_or(f32::MAX))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();
    (segments, starts, all, best)
}

// ---------------------------------------------------------------------------
// Baselines (checked-in JSON)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBaseline {
    pub map: String,
    pub min_on_ramp_frac: f32,
    pub max_kill_z_frac: f32,
    pub max_buried_frac: f32,
    /// Soft ceiling: median of per-window max origin err @ H=66 (best preset).
    pub max_median_origin_err_at_66: f32,
    /// Open-loop full-run p95 (diagnostic; not used for CI soft gate).
    /// `null` / missing deserializes as 0 (e.g. calibrated with `--no-open-loop`).
    #[serde(default, deserialize_with = "deserialize_f32_null_as_zero")]
    pub max_p95_origin_err: f32,
    /// Preferred preset name from calibration (informational).
    #[serde(default)]
    pub best_preset: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResimBaselines {
    pub maps: Vec<MapBaseline>,
}

impl ResimBaselines {
    pub fn for_map(&self, map: &str) -> Option<&MapBaseline> {
        self.maps.iter().find(|m| m.map == map)
    }

    pub fn pose_gates(&self, map: &str) -> Option<PoseGates> {
        self.for_map(map).map(|b| PoseGates {
            min_on_ramp_frac: b.min_on_ramp_frac,
            max_kill_z_frac: b.max_kill_z_frac,
            max_buried_frac: b.max_buried_frac,
        })
    }
}

/// Graybox-only helper kept for unit tests that don't load a BSP.
pub fn resim_world_only(
    world: &World,
    start: &PlayerState,
    frames: &[ReplayFrame],
    vars: &MoveVars,
) -> f32 {
    let mut player = start.clone();
    let mut max_err = 0.0_f32;
    for frame in frames {
        player = tick(world, &player, &frame.to_usercmd(), vars);
        max_err = max_err.max((player.origin - frame.origin).length());
    }
    max_err
}

#[cfg(test)]
mod tests {
    use super::*;
    use surf_core::movement::BugFixes;

    #[test]
    fn preset_grid_covers_matrix() {
        let g = preset_grid();
        let names: Vec<&str> = g.iter().map(|p| p.name).collect();
        assert!(names.contains(&"momentum_surf"));
        assert!(names.contains(&"ksf_css_66t"));
        assert!(names.contains(&"ksf_css_stock"));
        assert!(names.contains(&"aa100_accel5"));
        assert!(names.contains(&"aa150_accel10"));
        assert_eq!(g.len(), 5);
        let ksf = g.iter().find(|p| p.name == "ksf_css_66t").unwrap();
        assert!((ksf.vars.airaccelerate - 100.0).abs() < 1e-5);
        assert!((ksf.vars.accelerate - 10.0).abs() < 1e-5);
        assert!(ksf.vars.fixes.fix_ramp);
        let stock = g.iter().find(|p| p.name == "ksf_css_stock").unwrap();
        assert_eq!(stock.vars.fixes.fix_deadstrafe, BugFixes::stock().fix_deadstrafe);
    }

    #[test]
    fn percentile_p95() {
        let v: Vec<f32> = (0..100).map(|i| i as f32).collect();
        // idx = round((n-1)*0.95) = round(94.05) = 94
        assert!((percentile_sorted(&v, 0.95) - 94.0).abs() < 1e-5);
    }

    #[test]
    fn select_window_starts_picks_first_and_longest() {
        let segs = vec![
            RampSegment { start: 10, end: 60 },   // len 50
            RampSegment { start: 100, end: 200 }, // len 100 (longest)
            RampSegment { start: 300, end: 350 }, // len 50
            RampSegment { start: 400, end: 450 },
            RampSegment { start: 500, end: 550 },
        ];
        let starts = select_window_starts(&segs, 3);
        assert!(starts.contains(&10), "first: {starts:?}");
        assert!(starts.contains(&100), "longest: {starts:?}");
        assert_eq!(starts.len(), 3);
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn select_window_starts_caps() {
        let segs: Vec<_> = (0..40)
            .map(|i| RampSegment {
                start: i * 100,
                end: i * 100 + 50,
            })
            .collect();
        let starts = select_window_starts(&segs, MAX_WINDOWS);
        assert_eq!(starts.len(), MAX_WINDOWS);
    }
}
