//! Soft KSF / native replay re-sim against a loaded map (teleports + kill-z).
//!
//! Layer A probes recorded poses for ramp contact / fall-through / burial.
//! Layer B feeds stored inputs through `tick` under a MoveVars preset grid.

use serde::{Deserialize, Serialize};
use surf_core::brush::World;
use surf_core::is_on_surf_ramp;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState};
use surf_core::tick;
use surf_core::trace::point_contents_box;
use surf_map::LoadedMap;

use crate::replay::ReplayFrame;
use crate::zones::{MapZones, ZoneBox};

/// Default linear maps for the v1 KSF harness (leave-zone, no push/staged deps).
pub const V1_MAPS: &[&str] = &[
    "surf_summit",
    "surf_boreas",
    "surf_tendies",
    "surf_hourglass",
    "surf_andromeda",
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
    let mut errs: Vec<f32> = Vec::with_capacity(frames.len() - 1);
    let mut sum_err = 0.0_f32;
    let mut max_origin_err = 0.0_f32;
    let mut max_vel_err = 0.0_f32;
    let mut teleports = 0usize;
    let mut died_at = None;
    let mut reached_end = false;

    for (i, frame) in frames.iter().enumerate().skip(1) {
        player = tick(&map.world, &player, &frame.to_usercmd(), vars);

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
// Baselines (checked-in JSON)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBaseline {
    pub map: String,
    pub min_on_ramp_frac: f32,
    pub max_kill_z_frac: f32,
    pub max_buried_frac: f32,
    /// Soft p95 origin-error ceiling for the best preset (units).
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
}
