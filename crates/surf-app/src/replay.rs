//! Native mx-surf replay format (`.osxr`).
//!
//! Binary layout (little-endian):
//! ```text
//! magic:          b"OSXR"
//! version:        u16 (= 1)
//! header_len:     u32
//! header:         UTF-8 JSON ([ReplayHeader])
//! frames:         header.frame_count × [ReplayFrame] (44 bytes each)
//! ```
//!
//! Frames store both pose (ghost playback) and inputs (re-sim / CI). Recording
//! starts when the timer enters Running and stops on Finished; cancel (R /
//! voluntary re-enter start) discards the buffer.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use surf_core::brush::World;
use surf_core::is_on_surf_ramp;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};

use crate::settings::{GHOST_AUTO, GHOST_OFF, GHOST_PB};
use crate::zones::ZoneBox;

/// On-disk format version. Bump when frame layout or header schema breaks.
pub const REPLAY_FORMAT_VERSION: u16 = 2;
/// Oldest file version we still load (v1 = no splits field).
const MIN_LOAD_VERSION: u16 = 1;
const MAGIC: &[u8; 4] = b"OSXR";
/// Packed frame size (origin3 + vel3 + pitch + yaw + fwd + side + buttons + grounded + pad2).
pub const FRAME_BYTES: usize = 44;
const MAX_FRAMES: usize = 66_667 * 60 * 30; // ~30 min @ 66.67 Hz

pub const STYLE_MOMENTUM_SURF: &str = "momentum_surf";
pub const TRACK_MAIN: &str = "main";

const BTN_JUMP: u8 = 1;
const BTN_DUCK: u8 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayHeader {
    pub format_version: u16,
    pub map: String,
    pub track: String,
    pub style: String,
    pub tick_interval: f32,
    pub time_secs: f32,
    pub frame_count: u32,
    /// Unix seconds when the file was written.
    pub recorded_at: i64,
    /// Snapshot of movevars that produced the run (for style-matched re-sim).
    pub vars: ReplayVars,
    /// Ordered checkpoint split times (seconds from run start). Empty if none.
    #[serde(default)]
    pub splits: Vec<f32>,
    /// How inputs relate to poses. Empty/legacy: inferred from `style` (KSF → buttons lag).
    /// `"cmdProducesPose"` = apply frames[i] cmd to pose[i-1] → pose[i].
    /// `"ksfButtonsLag"` = buttons on frame[i] belong to the next tick.
    #[serde(default)]
    pub input_semantics: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayVars {
    pub gravity: f32,
    pub friction: f32,
    pub stopspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub maxspeed: f32,
    pub maxvelocity: f32,
    pub jump_impulse: f32,
    pub air_wishspeed_cap: f32,
    pub autobhop: bool,
    pub bhop_clamp: bool,
}

impl ReplayVars {
    pub fn from_move_vars(v: &MoveVars) -> Self {
        Self {
            gravity: v.gravity,
            friction: v.friction,
            stopspeed: v.stopspeed,
            accelerate: v.accelerate,
            airaccelerate: v.airaccelerate,
            maxspeed: v.maxspeed,
            maxvelocity: v.maxvelocity,
            jump_impulse: v.jump_impulse,
            air_wishspeed_cap: v.air_wishspeed_cap,
            autobhop: v.autobhop,
            bhop_clamp: v.bhop_clamp,
        }
    }

    /// Overlay stored knobs onto `base` (keeps tick/step/bounce/BugFixes from base).
    pub fn apply_to(&self, base: &MoveVars) -> MoveVars {
        let mut v = *base;
        v.gravity = self.gravity;
        v.friction = self.friction;
        v.stopspeed = self.stopspeed;
        v.accelerate = self.accelerate;
        v.airaccelerate = self.airaccelerate;
        v.maxspeed = self.maxspeed;
        v.maxvelocity = self.maxvelocity;
        v.jump_impulse = self.jump_impulse;
        v.air_wishspeed_cap = self.air_wishspeed_cap;
        v.autobhop = self.autobhop;
        v.bhop_clamp = self.bhop_clamp;
        v
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReplayFrame {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub forward_move: f32,
    pub side_move: f32,
    pub buttons: u8,
    pub grounded: bool,
}

impl ReplayFrame {
    pub fn from_tick(cmd: &UserCmd, player: &PlayerState) -> Self {
        let mut buttons = 0u8;
        if cmd.jump {
            buttons |= BTN_JUMP;
        }
        if cmd.duck {
            buttons |= BTN_DUCK;
        }
        Self {
            origin: player.origin,
            velocity: player.velocity,
            pitch: cmd.viewangles.pitch,
            yaw: cmd.viewangles.yaw,
            forward_move: cmd.forward_move,
            side_move: cmd.side_move,
            buttons,
            grounded: player.grounded,
        }
    }

    pub fn to_usercmd(self) -> UserCmd {
        UserCmd {
            viewangles: Angle::new(self.pitch, self.yaw, 0.0),
            forward_move: self.forward_move,
            side_move: self.side_move,
            jump: self.buttons & BTN_JUMP != 0,
            duck: self.buttons & BTN_DUCK != 0,
        }
    }

    fn write_bytes<W: Write>(&self, w: &mut W) -> Result<(), String> {
        w.write_all(&self.origin.x.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.origin.y.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.origin.z.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.velocity.x.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.velocity.y.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.velocity.z.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.pitch.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.yaw.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.forward_move.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&self.side_move.to_le_bytes())
            .map_err(|e| e.to_string())?;
        w.write_all(&[self.buttons, u8::from(self.grounded), 0, 0])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn read_bytes(r: &mut Cursor<&[u8]>) -> Result<Self, String> {
        let frame = Self {
            origin: Vec3::new(read_f32(r)?, read_f32(r)?, read_f32(r)?),
            velocity: Vec3::new(read_f32(r)?, read_f32(r)?, read_f32(r)?),
            pitch: read_f32(r)?,
            yaw: read_f32(r)?,
            forward_move: read_f32(r)?,
            side_move: read_f32(r)?,
            buttons: read_u8(r)?,
            grounded: read_u8(r)? != 0,
        };
        let _pad0 = read_u8(r)?;
        let _pad1 = read_u8(r)?;
        Ok(frame)
    }
}

fn read_f32(r: &mut Cursor<&[u8]>) -> Result<f32, String> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)
        .map_err(|e| format!("replay eof: {e}"))?;
    Ok(f32::from_le_bytes(buf))
}

fn read_u8(r: &mut Cursor<&[u8]>) -> Result<u8, String> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)
        .map_err(|e| format!("replay eof: {e}"))?;
    Ok(buf[0])
}

fn read_u16(r: &mut Cursor<&[u8]>) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)
        .map_err(|e| format!("replay eof: {e}"))?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(r: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)
        .map_err(|e| format!("replay eof: {e}"))?;
    Ok(u32::from_le_bytes(buf))
}

#[derive(Clone, Debug)]
pub struct Replay {
    pub header: ReplayHeader,
    pub frames: Vec<ReplayFrame>,
}

/// Shift KSF button/move fields so frame[i] holds the cmd that produced pose[i].
///
/// Angles and pose stay on frame[i]; forward/side/jump/duck come from frame[i-1].
pub fn align_ksf_buttons_to_pose(frames: &mut [ReplayFrame]) {
    if frames.len() < 2 {
        return;
    }
    let prev: Vec<(f32, f32, u8)> = frames
        .iter()
        .map(|f| (f.forward_move, f.side_move, f.buttons))
        .collect();
    for i in 1..frames.len() {
        frames[i].forward_move = prev[i - 1].0;
        frames[i].side_move = prev[i - 1].1;
        frames[i].buttons = prev[i - 1].2;
    }
}

impl Replay {
    /// True when on-disk buttons lag the pose by one tick (legacy KSF imports).
    pub fn buttons_lag_pose(&self) -> bool {
        match self.header.input_semantics.as_str() {
            "cmdProducesPose" => false,
            "ksfButtonsLag" => true,
            "" => {
                // Pre-field imports: KSF style means lagged buttons.
                self.header.style == "ksf_css_66t" || self.header.style.starts_with("ksf_")
            }
            _ => false,
        }
    }

    /// Frames safe for `tick` re-sim under native cmd-produces-pose semantics.
    pub fn frames_for_resim(&self) -> Vec<ReplayFrame> {
        let mut frames = self.frames.clone();
        if self.buttons_lag_pose() {
            align_ksf_buttons_to_pose(&mut frames);
        }
        frames
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("replay dir: {e}"))?;
        }
        let bytes = self.to_bytes()?;
        fs::write(path, bytes).map_err(|e| format!("replay write: {e}"))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("replay read: {e}"))?;
        Self::from_bytes(&bytes)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        if self.frames.len() != self.header.frame_count as usize {
            return Err(format!(
                "replay frame_count {} != frames.len() {}",
                self.header.frame_count,
                self.frames.len()
            ));
        }
        let header_json =
            serde_json::to_vec(&self.header).map_err(|e| format!("replay header json: {e}"))?;
        let mut out =
            Vec::with_capacity(4 + 2 + 4 + header_json.len() + self.frames.len() * FRAME_BYTES);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&REPLAY_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
        out.extend_from_slice(&header_json);
        for frame in &self.frames {
            frame.write_bytes(&mut out)?;
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Cursor::new(bytes);
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)
            .map_err(|e| format!("replay magic: {e}"))?;
        if &magic != MAGIC {
            return Err(format!("bad replay magic: {magic:?}"));
        }
        let version = read_u16(&mut r)?;
        if version < MIN_LOAD_VERSION || version > REPLAY_FORMAT_VERSION {
            return Err(format!(
                "unsupported replay version {version} (support {MIN_LOAD_VERSION}..{REPLAY_FORMAT_VERSION})"
            ));
        }
        let header_len = read_u32(&mut r)? as usize;
        let mut header_buf = vec![0u8; header_len];
        r.read_exact(&mut header_buf)
            .map_err(|e| format!("replay header: {e}"))?;
        let header: ReplayHeader =
            serde_json::from_slice(&header_buf).map_err(|e| format!("replay header json: {e}"))?;
        if header.format_version < MIN_LOAD_VERSION || header.format_version > REPLAY_FORMAT_VERSION
        {
            return Err(format!(
                "header formatVersion {} out of range",
                header.format_version
            ));
        }
        let mut frames = Vec::with_capacity(header.frame_count as usize);
        for _ in 0..header.frame_count {
            frames.push(ReplayFrame::read_bytes(&mut r)?);
        }
        Ok(Self { header, frames })
    }
}

/// In-memory capture while a timed run is active.
#[derive(Clone, Debug, Default)]
pub struct ReplayRecorder {
    frames: Vec<ReplayFrame>,
    active: bool,
}

impl ReplayRecorder {
    pub fn clear(&mut self) {
        self.frames.clear();
        self.active = false;
    }

    pub fn begin(&mut self) {
        self.frames.clear();
        self.active = true;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn push(&mut self, cmd: &UserCmd, player: &PlayerState) {
        if !self.active {
            return;
        }
        if self.frames.len() >= MAX_FRAMES {
            return;
        }
        self.frames.push(ReplayFrame::from_tick(cmd, player));
    }

    /// Finalize into a [`Replay`]. Clears the recorder. Returns `None` if empty.
    pub fn finish(
        &mut self,
        map: &str,
        time_secs: f32,
        vars: &MoveVars,
        splits: &[f32],
    ) -> Option<Replay> {
        if !self.active || self.frames.is_empty() {
            self.clear();
            return None;
        }
        let frames = std::mem::take(&mut self.frames);
        self.active = false;
        Some(Replay {
            header: ReplayHeader {
                format_version: REPLAY_FORMAT_VERSION,
                map: map.to_string(),
                track: TRACK_MAIN.to_string(),
                style: STYLE_MOMENTUM_SURF.to_string(),
                tick_interval: vars.tick_interval,
                time_secs,
                frame_count: frames.len() as u32,
                recorded_at: unix_now(),
                vars: ReplayVars::from_move_vars(vars),
                splits: splits.to_vec(),
                input_semantics: "cmdProducesPose".to_string(),
            },
            frames,
        })
    }
}

/// Derive ordered checkpoint split times by walking recorded poses through zone boxes.
/// Frame `i` corresponds to elapsed `(i + 1) * tick_interval` (first Running tick).
pub fn derive_splits(
    frames: &[ReplayFrame],
    checkpoints: &[ZoneBox],
    tick_interval: f32,
    hull: Hull,
) -> Vec<f32> {
    let mut splits = Vec::new();
    let mut next = 0usize;
    let mut inside = false;
    for (i, frame) in frames.iter().enumerate() {
        if next >= checkpoints.len() {
            break;
        }
        let now = checkpoints[next].contains_player(frame.origin, hull);
        if now && !inside {
            let t = (i as f32 + 1.0) * tick_interval;
            splits.push(t);
            next += 1;
            inside = false; // next CP starts fresh
            continue;
        }
        inside = now;
    }
    splits
}

/// Re-sim `frames` inputs from `start`, return max origin error vs recorded poses.
pub fn resim_max_origin_error(
    world: &surf_core::brush::World,
    start: &PlayerState,
    frames: &[ReplayFrame],
    vars: &MoveVars,
) -> f32 {
    let mut player = start.clone();
    let mut max_err = 0.0_f32;
    for frame in frames {
        player = surf_core::tick(world, &player, &frame.to_usercmd(), vars);
        max_err = max_err.max((player.origin - frame.origin).length());
    }
    max_err
}

/// Playback helper for PB ghost racing (frame index = elapsed running ticks).
#[derive(Clone, Debug, Default)]
pub struct GhostPlayback {
    pub frames: Vec<ReplayFrame>,
    /// Parallel to `frames`: world-probed surf-ramp contact (for trail color).
    pub on_ramp: Vec<bool>,
    pub tick_interval: f32,
    pub splits: Vec<f32>,
    pub time_secs: f32,
    /// Advances while the live timer is Running.
    pub frame_idx: usize,
    pub active: bool,
}

impl GhostPlayback {
    pub fn from_replay(replay: Replay) -> Self {
        Self {
            tick_interval: replay.header.tick_interval,
            splits: replay.header.splits.clone(),
            time_secs: replay.header.time_secs,
            on_ramp: Vec::new(),
            frames: replay.frames,
            frame_idx: 0,
            active: false,
        }
    }

    /// Classify each frame for neon trail coloring (once after load).
    pub fn classify_ramp_contact(&mut self, world: &World) {
        let hull = Hull::css_stand();
        self.on_ramp = self
            .frames
            .iter()
            .map(|f| is_on_surf_ramp(world, f.origin, &hull))
            .collect();
        let on = self.on_ramp.iter().filter(|&&b| b).count();
        println!("  ghost trail: {on}/{} frames on-ramp", self.on_ramp.len());
    }

    pub fn begin(&mut self) {
        self.frame_idx = 0;
        self.active = !self.frames.is_empty();
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.frame_idx = 0;
    }

    /// Jump playback to the frame nearest `secs` (loadloc: the live clock moved,
    /// so the ghost has to move with it or every delta on screen is a lie).
    /// Frame `i` is elapsed `(i + 1) * tick_interval`, matching [`derive_splits`].
    pub fn seek_secs(&mut self, secs: f32) {
        if self.frames.is_empty() || self.tick_interval <= 0.0 {
            self.active = false;
            return;
        }
        let idx = (secs / self.tick_interval).round() - 1.0;
        let idx = idx.max(0.0).min((self.frames.len() - 1) as f32);
        self.frame_idx = idx as usize;
        self.active = true;
    }

    pub fn advance(&mut self) {
        if !self.active || self.frames.is_empty() {
            return;
        }
        if self.frame_idx + 1 < self.frames.len() {
            self.frame_idx += 1;
        }
    }

    /// Returns `(origin, velocity_2d_unused_here, ducked)`.
    pub fn sample(&self, alpha: f32) -> Option<(Vec3, Vec3, bool)> {
        if !self.active || self.frames.is_empty() {
            return None;
        }
        let i = self.frame_idx.min(self.frames.len() - 1);
        let a = &self.frames[i];
        let ducked = a.buttons & BTN_DUCK != 0;
        if i + 1 < self.frames.len() {
            let b = &self.frames[i + 1];
            let t = alpha.clamp(0.0, 1.0);
            let origin = a.origin.lerp(b.origin, t);
            let vel = a.velocity.lerp(b.velocity, t);
            Some((origin, vel, ducked))
        } else {
            Some((a.origin, a.velocity, ducked))
        }
    }

    pub fn current_speed_2d(&self) -> Option<f32> {
        let i = self.frame_idx.min(self.frames.len().saturating_sub(1));
        self.frames.get(i).map(|f| f.velocity.length_2d())
    }

    pub fn current_time(&self) -> f32 {
        if self.frames.is_empty() {
            return 0.0;
        }
        let i = self.frame_idx.min(self.frames.len() - 1);
        (i as f32 + 1.0) * self.tick_interval
    }

    /// Path samples from run start through the current playback frame.
    pub fn trail_to_current(&self) -> Vec<(Vec3, bool)> {
        if !self.active || self.frames.is_empty() {
            return Vec::new();
        }
        let end = self.frame_idx.min(self.frames.len() - 1) + 1;
        self.frames[..end]
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let on = self.on_ramp.get(i).copied().unwrap_or(f.grounded);
                (f.origin, on)
            })
            .collect()
    }
}

/// One selectable ghost in the Esc menu.
#[derive(Clone, Debug)]
pub struct GhostOption {
    /// Settings id: `"off"`, `"pb"`, or path string.
    pub id: String,
    pub label: String,
    pub path: Option<PathBuf>,
}

/// KSF imported ghosts live under `assets/replays/external/ksf/<map>/imported/`.
pub fn ksf_imported_dir(map: &str) -> PathBuf {
    crate::assets::ksf_dir()
        .join(map)
        .join("imported")
}

/// Build Off / Auto / PB / KSF-imported options for `map`. Includes `current_id`
/// if it points at an existing custom `.osxr` not already listed.
pub fn ghost_catalog(map: &str, current_id: &str) -> Vec<GhostOption> {
    let ksf = list_ksf_ghosts(map);
    let auto_label = match ksf.first() {
        Some(first) => format!("Auto ({})", first.label),
        None => {
            if pb_replay_path(map).is_file() {
                "Auto (PB)".into()
            } else {
                "Auto (none)".into()
            }
        }
    };

    let mut out = vec![
        GhostOption {
            id: GHOST_OFF.into(),
            label: "Off".into(),
            path: None,
        },
        GhostOption {
            id: GHOST_AUTO.into(),
            label: auto_label,
            path: resolve_auto_ghost_path(map),
        },
        GhostOption {
            id: GHOST_PB.into(),
            label: if pb_replay_path(map).is_file() {
                "PB".into()
            } else {
                "PB (none)".into()
            },
            path: Some(pb_replay_path(map)),
        },
    ];

    let mut listed_paths: Vec<PathBuf> = Vec::new();
    for g in ksf {
        listed_paths.push(g.path.clone().unwrap());
        out.push(g);
    }

    if current_id != GHOST_OFF && current_id != GHOST_PB && current_id != GHOST_AUTO {
        let cur = PathBuf::from(current_id);
        let already =
            listed_paths.iter().any(|p| p == &cur) || out.iter().any(|o| o.id == current_id);
        if !already && cur.is_file() {
            let label = cur
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("custom")
                .to_string();
            out.push(GhostOption {
                id: current_id.to_string(),
                label: format!("Custom {label}"),
                path: Some(cur),
            });
        }
    }

    out
}

/// Resolve [`GHOST_AUTO`] → KSF #1 path, else PB path (may not exist yet).
pub fn resolve_ghost_path(map: &str, ghost_id: &str) -> Option<PathBuf> {
    match ghost_id {
        GHOST_OFF => None,
        GHOST_AUTO => resolve_auto_ghost_path(map),
        GHOST_PB => Some(pb_replay_path(map)),
        other => Some(PathBuf::from(other)),
    }
}

fn resolve_auto_ghost_path(map: &str) -> Option<PathBuf> {
    if let Some(ksf1) = list_ksf_ghosts(map).into_iter().next() {
        return ksf1.path;
    }
    let pb = pb_replay_path(map);
    if pb.is_file() {
        Some(pb)
    } else {
        // Still return PB path so load can soft-fail; menu shows Auto (none).
        Some(pb)
    }
}

fn list_ksf_ghosts(map: &str) -> Vec<GhostOption> {
    let imported = ksf_imported_dir(map);
    if !imported.is_dir() {
        return Vec::new();
    }
    let meta = load_ksf_manifest_labels(map);
    let mut files: Vec<PathBuf> = fs::read_dir(&imported)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("osxr"))
        })
        .collect();
    files.sort();
    files.sort_by_key(|p| {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        meta.get(&stem).map(|m| m.rank).unwrap_or(u32::MAX)
    });
    let mut out = Vec::new();
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ghost")
            .to_string();
        let id = path_to_ghost_id(&path);
        let label = meta
            .get(&stem)
            .map(|m| {
                let name = truncate_name(&m.name, 18);
                format!("KSF #{} {name} {}", m.rank, format_secs(m.time))
            })
            .unwrap_or_else(|| format!("KSF {stem}"));
        out.push(GhostOption {
            id,
            label,
            path: Some(path),
        });
    }
    out
}

/// Normalize a path for settings storage (prefer relative when under cwd).
pub fn path_to_ghost_id(path: &Path) -> String {
    let path = path.to_path_buf();
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = path.strip_prefix(&cwd) {
            return rel.to_string_lossy().into_owned();
        }
    }
    path.to_string_lossy().into_owned()
}

#[derive(Clone, Debug)]
struct KsfManifestMeta {
    rank: u32,
    name: String,
    time: f32,
}

fn load_ksf_manifest_labels(map: &str) -> std::collections::HashMap<String, KsfManifestMeta> {
    let path = crate::assets::ksf_dir()
        .join(map)
        .join("manifest.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return std::collections::HashMap::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return std::collections::HashMap::new();
    };
    let mut out = std::collections::HashMap::new();
    let Some(records) = v.get("records").and_then(|r| r.as_array()) else {
        return out;
    };
    for rec in records {
        let file = rec.get("file").and_then(|f| f.as_str()).unwrap_or_default();
        let stem = file.trim_end_matches(".rec");
        if stem.is_empty() {
            continue;
        }
        let rank = rec.get("rank").and_then(|r| r.as_u64()).unwrap_or(999) as u32;
        let name = rec
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string();
        let time = rec.get("time").and_then(|t| t.as_f64()).unwrap_or(0.0) as f32;
        out.insert(stem.to_string(), KsfManifestMeta { rank, name, time });
    }
    out
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let mut it = name.chars();
    let head: String = it.by_ref().take(max_chars).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn format_secs(secs: f32) -> String {
    if secs >= 60.0 {
        let m = (secs / 60.0).floor() as u32;
        let s = secs - (m as f32) * 60.0;
        format!("{m}:{s:06.3}")
    } else {
        format!("{secs:.3}")
    }
}

/// `~/Library/Application Support/mx-surf/replays/<map>/<track>/<style>/`
pub fn replay_dir(map: &str) -> PathBuf {
    app_support_dir()
        .join("replays")
        .join(map)
        .join(TRACK_MAIN)
        .join(STYLE_MOMENTUM_SURF)
}

pub fn pb_replay_path(map: &str) -> PathBuf {
    replay_dir(map).join("pb.osxr")
}

/// Unique path for a finished run: `<unix>_<time_ms>.osxr`.
pub fn new_run_replay_path(map: &str, time_secs: f32) -> PathBuf {
    let ms = ((time_secs.max(0.0)) * 1000.0).round() as u32;
    replay_dir(map).join(format!("{}_{ms:08}.osxr", unix_now()))
}

fn app_support_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/mx-surf")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_frame() -> ReplayFrame {
        ReplayFrame {
            origin: Vec3::new(1600.0, 0.0, 11552.0),
            velocity: Vec3::new(350.0, 10.0, -20.0),
            pitch: 12.5,
            yaw: -90.0,
            forward_move: 1.0,
            side_move: -1.0,
            buttons: BTN_JUMP,
            grounded: false,
        }
    }

    #[test]
    fn roundtrip_bytes() {
        let replay = Replay {
            header: ReplayHeader {
                format_version: REPLAY_FORMAT_VERSION,
                map: "surf_summit".into(),
                track: TRACK_MAIN.into(),
                style: STYLE_MOMENTUM_SURF.into(),
                tick_interval: 0.015,
                time_secs: 42.015,
                frame_count: 2,
                recorded_at: 1_700_000_000,
                vars: ReplayVars::from_move_vars(&MoveVars::momentum_surf()),
                splits: vec![10.0, 20.5],
                input_semantics: "cmdProducesPose".into(),
            },
            frames: vec![sample_frame(), {
                let mut f = sample_frame();
                f.origin.x += 16.0;
                f.buttons = 0;
                f.grounded = true;
                f
            }],
        };
        let bytes = replay.to_bytes().unwrap();
        let loaded = Replay::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.header.map, "surf_summit");
        assert_eq!(loaded.header.splits.len(), 2);
        assert_eq!(loaded.frames.len(), 2);
        assert!((loaded.frames[0].origin.x - 1600.0).abs() < 1e-5);
        assert!((loaded.frames[0].yaw - (-90.0)).abs() < 1e-5);
        assert!(loaded.frames[0].buttons & BTN_JUMP != 0);
        assert!(!loaded.frames[0].grounded);
        assert!(loaded.frames[1].grounded);
        let cmd = loaded.frames[0].to_usercmd();
        assert!(cmd.jump);
        assert!((cmd.forward_move - 1.0).abs() < 1e-5);
    }

    #[test]
    fn recorder_finish_and_clear_on_cancel() {
        let vars = MoveVars::momentum_surf();
        let mut rec = ReplayRecorder::default();
        let mut player = PlayerState::default();
        player.origin = Vec3::new(1.0, 2.0, 3.0);
        let cmd = UserCmd {
            forward_move: 1.0,
            jump: true,
            ..UserCmd::default()
        };
        rec.begin();
        rec.push(&cmd, &player);
        player.origin.x += 1.0;
        rec.push(&cmd, &player);
        assert_eq!(rec.frame_count(), 2);
        let replay = rec.finish("surf_test", 1.5, &vars, &[0.5]).unwrap();
        assert_eq!(replay.frames.len(), 2);
        assert!((replay.header.time_secs - 1.5).abs() < 1e-5);
        assert_eq!(replay.header.splits, vec![0.5]);
        assert!(!rec.is_active());
        assert_eq!(rec.frame_count(), 0);

        rec.begin();
        rec.push(&cmd, &player);
        rec.clear();
        assert!(rec.finish("surf_test", 1.0, &vars, &[]).is_none());
    }

    #[test]
    fn save_load_temp_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-replay-{stamp}.osxr"));
        let replay = Replay {
            header: ReplayHeader {
                format_version: REPLAY_FORMAT_VERSION,
                map: "surf_test".into(),
                track: TRACK_MAIN.into(),
                style: STYLE_MOMENTUM_SURF.into(),
                tick_interval: 0.015,
                time_secs: 3.0,
                frame_count: 1,
                recorded_at: 123,
                vars: ReplayVars::from_move_vars(&MoveVars::momentum_surf()),
                splits: Vec::new(),
                input_semantics: "cmdProducesPose".into(),
            },
            frames: vec![sample_frame()],
        };
        replay.save(&path).unwrap();
        let loaded = Replay::load(&path).unwrap();
        assert_eq!(loaded.header.map, "surf_test");
        assert_eq!(loaded.frames.len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn graybox_resim_soft_fixture() {
        use surf_core::graybox;
        use surf_core::tick;

        let gb = graybox::surf_ramp_arena();
        let vars = MoveVars::momentum_surf();
        let mut player = PlayerState {
            origin: gb.spawn_origin,
            viewangles: Angle::new(0.0, gb.spawn_yaw, 0.0),
            grounded: true,
            ..PlayerState::default()
        };
        // Settle onto ground.
        for _ in 0..10 {
            player = tick(&gb.world, &player, &UserCmd::default(), &vars);
        }
        let start = player.clone();
        let mut rec = ReplayRecorder::default();
        rec.begin();
        // Scripted: look along +X, hold forward+jump, slight right strafe.
        for i in 0..120 {
            let yaw = gb.spawn_yaw + (i as f32) * 0.4;
            let cmd = UserCmd {
                viewangles: Angle::new(10.0, yaw, 0.0),
                forward_move: 1.0,
                side_move: if i % 20 < 10 { 1.0 } else { -1.0 },
                jump: true,
                duck: false,
            };
            player = tick(&gb.world, &player, &cmd, &vars);
            rec.push(&cmd, &player);
        }
        let replay = rec
            .finish("graybox", 120.0 * vars.tick_interval, &vars, &[])
            .unwrap();
        let err = resim_max_origin_error(&gb.world, &start, &replay.frames, &vars);
        assert!(
            err < 1e-3,
            "re-sim drift {err} exceeds soft tolerance (determinism broken)"
        );

        // Stable checked-in fixture at workspace assets/fixtures/ (fixed recorded_at).
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fixtures/graybox_strafe.osxr");
        let mut stable = replay.clone();
        stable.header.recorded_at = 1;
        let need_write = match Replay::load(&fixture) {
            Ok(existing) => existing.frames.len() != stable.frames.len(),
            Err(_) => true,
        };
        if need_write {
            if let Some(parent) = fixture.parent() {
                let _ = fs::create_dir_all(parent);
            }
            stable.save(&fixture).expect("write fixture");
        }
        let loaded = Replay::load(&fixture).expect("load fixture");
        let err2 = resim_max_origin_error(&gb.world, &start, &loaded.frames, &vars);
        assert!(
            err2 < 1e-3,
            "checked-in fixture drift {err2} (physics changed; delete assets/fixtures/graybox_strafe.osxr to regenerate)"
        );
    }

    #[test]
    fn ghost_catalog_has_off_auto_pb() {
        let cat = ghost_catalog("surf_no_such_map_zzzz", "auto");
        assert_eq!(cat[0].id, GHOST_OFF);
        assert_eq!(cat[1].id, GHOST_AUTO);
        assert_eq!(cat[2].id, GHOST_PB);
        assert!(cat.len() >= 3);
    }

    #[test]
    fn align_ksf_buttons_shifts_moves() {
        let mut frames = vec![
            ReplayFrame {
                forward_move: 1.0,
                side_move: 0.0,
                buttons: BTN_JUMP,
                ..sample_frame()
            },
            ReplayFrame {
                forward_move: 0.0,
                side_move: -1.0,
                buttons: 0,
                ..sample_frame()
            },
            ReplayFrame {
                forward_move: 0.0,
                side_move: 1.0,
                buttons: BTN_DUCK,
                ..sample_frame()
            },
        ];
        align_ksf_buttons_to_pose(&mut frames);
        assert!((frames[0].forward_move - 1.0).abs() < 1e-5);
        assert!((frames[1].forward_move - 1.0).abs() < 1e-5);
        assert!((frames[1].side_move - 0.0).abs() < 1e-5);
        assert_eq!(frames[1].buttons, BTN_JUMP);
        assert!((frames[2].side_move - (-1.0)).abs() < 1e-5);
        assert_eq!(frames[2].buttons, 0);
    }

    #[test]
    fn ksf_style_infers_buttons_lag() {
        let replay = Replay {
            header: ReplayHeader {
                format_version: REPLAY_FORMAT_VERSION,
                map: "surf_summit".into(),
                track: TRACK_MAIN.into(),
                style: "ksf_css_66t".into(),
                tick_interval: 0.015,
                time_secs: 1.0,
                frame_count: 1,
                recorded_at: 0,
                vars: ReplayVars::from_move_vars(&MoveVars::momentum_surf()),
                splits: Vec::new(),
                input_semantics: String::new(),
            },
            frames: vec![sample_frame()],
        };
        assert!(replay.buttons_lag_pose());
        let mut aligned = replay.clone();
        aligned.header.input_semantics = "cmdProducesPose".into();
        assert!(!aligned.buttons_lag_pose());
    }

    #[test]
    fn classify_summit_ksf_has_on_ramp_frames() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let map = root.join("assets/maps/surf_summit.bsp");
        let ghost = root.join(
            "assets/replays/external/ksf/surf_summit/imported/replay_css_2946_0_712551_1745965307.osxr",
        );
        if !map.is_file() || !ghost.is_file() {
            return;
        }
        let loaded = surf_map::LoadedMap::load_path(&map).expect("summit");
        let replay = Replay::load(&ghost).expect("ksf ghost");
        let mut g = GhostPlayback::from_replay(replay);
        g.classify_ramp_contact(&loaded.world);
        let on = g.on_ramp.iter().filter(|&&b| b).count();
        let n = g.on_ramp.len();
        assert!(
            on > n / 10,
            "expected substantial on-ramp share, got {on}/{n}"
        );
        assert!(on < n, "expected some air frames, got {on}/{n}");
    }
}
