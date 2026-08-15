//! KSF timer `.rec` parser and import to native `.osxr`.
//!
//! Format verified against the public KSF viewer layout (see
//! `docs/KSF-REPLAY-HANDOFF.md`). Reimplemented here — no dependency on KSF
//! frontend code. Imported poses are for ghosts / soft metrics, not golden
//! physics masters (KSF CS:S server vs our Momentum-style fixed CS:S).
//!
//! Input layout: KSF stores buttons held *at* each pose; the cmd that produced
//! pose[i] is sample[i-1]'s buttons (angles stay with pose[i]). [`to_osxr`]
//! shifts buttons so native re-sim semantics apply. Legacy `.osxr` without
//! `inputSemantics: cmdProducesPose` are aligned at re-sim time via
//! [`crate::replay::Replay::frames_for_resim`].

use std::path::Path;

use surf_core::math::Vec3;
use surf_core::movement::{Hull, MoveVars};

use crate::replay::{
    derive_splits, Replay, ReplayFrame, ReplayHeader, ReplayVars, REPLAY_FORMAT_VERSION, TRACK_MAIN,
};
use crate::zones::MapZones;

/// Style tag for KSF CS:S 66-tick imports (distinct from native Momentum recordings).
pub const STYLE_KSF_CSS_66T: &str = "ksf_css_66t";

/// Header `inputSemantics`: apply `frames[i]` cmd to pose[i-1] → pose[i] (native + aligned KSF).
pub const INPUT_SEMANTICS_CMD_PRODUCES_POSE: &str = "cmdProducesPose";
/// Legacy KSF imports: buttons on frame[i] are held *at* pose[i] (lag one tick vs cmd).
pub const INPUT_SEMANTICS_KSF_BUTTONS_LAG: &str = "ksfButtonsLag";

const TICK_INTERVAL: f32 = 0.015;
const BOOKMARK_SIZE: usize = 524;
const V2_TICK_SIZE: usize = 40;
const MAX_TICKS: usize = 66_667 * 60 * 30; // ~30 min
const MAX_BOOKMARKS: usize = 10_000;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

// Source IN_ buttons (from KSF viewer).
const IN_JUMP: i32 = 2;
const IN_DUCK: i32 = 4;
const IN_FORWARD: i32 = 8;
const IN_BACK: i32 = 16;
const IN_MOVELEFT: i32 = 512;
const IN_MOVERIGHT: i32 = 1024;

const FL_ONGROUND: i32 = 1;

const BTN_JUMP: u8 = 1;
const BTN_DUCK: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KsfVersion {
    V2,
    V3,
}

#[derive(Clone, Debug)]
pub struct KsfHeader {
    pub version: KsfVersion,
    pub secondary: i32,
    pub tick_count: usize,
    pub bookmark_count: usize,
    pub tick_size: usize,
    pub first_tick_offset: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct KsfFrame {
    pub buttons: i32,
    pub origin: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub flags: i32,
    pub velocity: Vec3,
}

#[derive(Clone, Debug)]
pub struct KsfReplay {
    pub header: KsfHeader,
    pub frames: Vec<KsfFrame>,
}

#[derive(Clone, Debug)]
pub struct KsfCrop {
    pub start_idx: usize,
    pub end_idx: usize,
}

impl KsfCrop {
    pub fn frame_count(&self) -> usize {
        self.end_idx.saturating_sub(self.start_idx).saturating_add(1)
    }

    pub fn duration_secs(&self, tick_interval: f32) -> f32 {
        self.frame_count() as f32 * tick_interval
    }
}

#[derive(Debug)]
pub enum KsfError {
    Io(std::io::Error),
    Format(String),
    Crop(String),
}

impl std::fmt::Display for KsfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KsfError::Io(e) => write!(f, "ksf io: {e}"),
            KsfError::Format(e) => write!(f, "ksf format: {e}"),
            KsfError::Crop(e) => write!(f, "ksf crop: {e}"),
        }
    }
}

impl From<std::io::Error> for KsfError {
    fn from(e: std::io::Error) -> Self {
        KsfError::Io(e)
    }
}

pub fn parse_ksf_bytes(bytes: &[u8]) -> Result<KsfReplay, KsfError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(KsfError::Format(format!(
            "file too large ({} > {MAX_FILE_BYTES})",
            bytes.len()
        )));
    }
    if bytes.len() < 16 {
        return Err(KsfError::Format("truncated header".into()));
    }

    let magic = read_i32(bytes, 0)?;
    let secondary = read_i32(bytes, 4)?;
    let tick_count_i = read_i32(bytes, 8)?;
    let bookmark_count_i = read_i32(bytes, 12)?;

    if tick_count_i < 0 || bookmark_count_i < 0 {
        return Err(KsfError::Format("negative tick/bookmark count".into()));
    }
    let tick_count = tick_count_i as usize;
    let bookmark_count = bookmark_count_i as usize;
    if tick_count > MAX_TICKS {
        return Err(KsfError::Format(format!("tick_count {tick_count} too large")));
    }
    if bookmark_count > MAX_BOOKMARKS {
        return Err(KsfError::Format(format!(
            "bookmark_count {bookmark_count} too large"
        )));
    }

    let (version, tick_size, first_tick_offset) = match magic {
        2 => {
            let first = 16usize
                .checked_add(
                    bookmark_count
                        .checked_mul(BOOKMARK_SIZE)
                        .ok_or_else(|| KsfError::Format("bookmark size overflow".into()))?,
                )
                .ok_or_else(|| KsfError::Format("first_tick_offset overflow".into()))?;
            (KsfVersion::V2, V2_TICK_SIZE, first)
        }
        3 => {
            if bytes.len() < 24 {
                return Err(KsfError::Format("truncated v3 header".into()));
            }
            let frame_cells = read_i32(bytes, 16)?;
            let extension_cells = read_i32(bytes, 20)?;
            if frame_cells < 10 || extension_cells < 0 {
                return Err(KsfError::Format(format!(
                    "bad v3 cells frame={frame_cells} ext={extension_cells}"
                )));
            }
            let tick_size = (frame_cells as usize)
                .checked_mul(4)
                .ok_or_else(|| KsfError::Format("v3 tick_size overflow".into()))?;
            if tick_size < V2_TICK_SIZE {
                return Err(KsfError::Format("v3 tick_size < 40".into()));
            }
            let after_ext = 24usize
                .checked_add(
                    (extension_cells as usize)
                        .checked_mul(4)
                        .ok_or_else(|| KsfError::Format("v3 extension overflow".into()))?,
                )
                .ok_or_else(|| KsfError::Format("v3 header overflow".into()))?;
            let first = after_ext
                .checked_add(
                    bookmark_count
                        .checked_mul(BOOKMARK_SIZE)
                        .ok_or_else(|| KsfError::Format("bookmark size overflow".into()))?,
                )
                .ok_or_else(|| KsfError::Format("first_tick_offset overflow".into()))?;
            (KsfVersion::V3, tick_size, first)
        }
        other => {
            return Err(KsfError::Format(format!(
                "unsupported magic {other} (want 2 or 3)"
            )));
        }
    };

    if first_tick_offset > bytes.len() {
        return Err(KsfError::Format("first_tick_offset past EOF".into()));
    }
    let frames_bytes = tick_count
        .checked_mul(tick_size)
        .ok_or_else(|| KsfError::Format("frames byte length overflow".into()))?;
    let end = first_tick_offset
        .checked_add(frames_bytes)
        .ok_or_else(|| KsfError::Format("frames end overflow".into()))?;
    if end > bytes.len() {
        return Err(KsfError::Format(format!(
            "truncated frames (need {end}, have {})",
            bytes.len()
        )));
    }

    let mut frames = Vec::with_capacity(tick_count);
    for i in 0..tick_count {
        let off = first_tick_offset + i * tick_size;
        frames.push(parse_frame_prefix(&bytes[off..off + V2_TICK_SIZE])?);
    }

    Ok(KsfReplay {
        header: KsfHeader {
            version,
            secondary,
            tick_count,
            bookmark_count,
            tick_size,
            first_tick_offset,
        },
        frames,
    })
}

pub fn load_ksf_file(path: &Path) -> Result<KsfReplay, KsfError> {
    let bytes = std::fs::read(path)?;
    parse_ksf_bytes(&bytes)
}

fn parse_frame_prefix(bytes: &[u8]) -> Result<KsfFrame, KsfError> {
    if bytes.len() < V2_TICK_SIZE {
        return Err(KsfError::Format("truncated frame".into()));
    }
    let buttons = read_i32(bytes, 0)?;
    let origin = Vec3::new(read_f32(bytes, 4)?, read_f32(bytes, 8)?, read_f32(bytes, 12)?);
    let pitch = read_f32(bytes, 16)?;
    let yaw = read_f32(bytes, 20)?;
    let flags = read_i32(bytes, 24)?;
    let velocity = Vec3::new(read_f32(bytes, 28)?, read_f32(bytes, 32)?, read_f32(bytes, 36)?);
    if !origin.x.is_finite()
        || !origin.y.is_finite()
        || !origin.z.is_finite()
        || !velocity.x.is_finite()
        || !velocity.y.is_finite()
        || !velocity.z.is_finite()
        || !pitch.is_finite()
        || !yaw.is_finite()
    {
        return Err(KsfError::Format("non-finite float in frame".into()));
    }
    Ok(KsfFrame {
        buttons,
        origin,
        pitch,
        yaw,
        flags,
        velocity,
    })
}

impl KsfFrame {
    pub fn to_replay_frame(self) -> ReplayFrame {
        let forward = (self.buttons & IN_FORWARD != 0) as i8 - (self.buttons & IN_BACK != 0) as i8;
        let side =
            (self.buttons & IN_MOVERIGHT != 0) as i8 - (self.buttons & IN_MOVELEFT != 0) as i8;
        let mut buttons = 0u8;
        if self.buttons & IN_JUMP != 0 {
            buttons |= BTN_JUMP;
        }
        if self.buttons & IN_DUCK != 0 {
            buttons |= BTN_DUCK;
        }
        ReplayFrame {
            origin: self.origin,
            velocity: self.velocity,
            pitch: self.pitch,
            yaw: self.yaw,
            forward_move: forward as f32,
            side_move: side as f32,
            buttons,
            grounded: self.flags & FL_ONGROUND != 0,
        }
    }
}

/// Find leave-start → enter-end indices using the map's leave-zone timer rule.
///
/// Summit (and default) uses leave-start; `start_on_jump` maps are not handled
/// specially here yet (same leave-zone crop is still a usable ghost window).
pub fn crop_main_run(frames: &[KsfFrame], zones: &MapZones, hull: Hull) -> Result<KsfCrop, KsfError> {
    let start_zone = &zones.main.start;
    let end_zone = &zones.main.end;
    let mut was_in_start = false;
    let mut start_idx: Option<usize> = None;
    let mut end_idx: Option<usize> = None;

    for (i, frame) in frames.iter().enumerate() {
        let in_start = start_zone.contains_player(frame.origin, hull);
        let in_end = end_zone.contains_player(frame.origin, hull);
        match start_idx {
            None => {
                if was_in_start && !in_start {
                    start_idx = Some(i);
                }
            }
            Some(_) => {
                if in_end && !in_start {
                    end_idx = Some(i);
                    break;
                }
            }
        }
        was_in_start = in_start;
    }

    let start_idx = start_idx.ok_or_else(|| {
        KsfError::Crop("never left start zone (no timed-run start found)".into())
    })?;
    let end_idx = end_idx.ok_or_else(|| KsfError::Crop("never entered end zone".into()))?;
    if end_idx < start_idx {
        return Err(KsfError::Crop("end before start".into()));
    }
    Ok(KsfCrop { start_idx, end_idx })
}

/// Convert a cropped KSF run into a native [`Replay`].
pub fn to_osxr(
    ksf: &KsfReplay,
    crop: KsfCrop,
    map: &str,
    zones: &MapZones,
    meta_time_secs: Option<f32>,
) -> Result<Replay, KsfError> {
    if crop.end_idx >= ksf.frames.len() {
        return Err(KsfError::Crop("crop end past frames".into()));
    }
    let slice = &ksf.frames[crop.start_idx..=crop.end_idx];
    let mut frames: Vec<ReplayFrame> =
        slice.iter().copied().map(KsfFrame::to_replay_frame).collect();
    // KSF samples store buttons held *at* the pose; the cmd that produced pose[i]
    // is the buttons from sample[i-1] (angles stay with pose[i]). Shift so native
    // re-sim semantics hold: apply frames[i].to_usercmd() to pose[i-1] → pose[i].
    crate::replay::align_ksf_buttons_to_pose(&mut frames);
    let vars = MoveVars::ksf_css_66t();
    let time_secs = meta_time_secs.unwrap_or_else(|| crop.duration_secs(TICK_INTERVAL));
    let hull = Hull::css_stand();
    let splits = derive_splits(&frames, &zones.main.checkpoints, TICK_INTERVAL, hull);

    Ok(Replay {
        header: ReplayHeader {
            format_version: REPLAY_FORMAT_VERSION,
            map: map.to_string(),
            track: TRACK_MAIN.to_string(),
            style: STYLE_KSF_CSS_66T.to_string(),
            tick_interval: TICK_INTERVAL,
            time_secs,
            frame_count: frames.len() as u32,
            recorded_at: 0,
            vars: ReplayVars::from_move_vars(&vars),
            splits,
            // Buttons already shifted to cmd-produces-pose layout.
            input_semantics: INPUT_SEMANTICS_CMD_PRODUCES_POSE.to_string(),
        },
        frames,
    })
}

/// Parse → crop → `.osxr` in one shot.
pub fn import_ksf_to_replay(
    path: &Path,
    map: &str,
    zones: &MapZones,
    meta_time_secs: Option<f32>,
) -> Result<(Replay, KsfCrop, KsfHeader), KsfError> {
    let ksf = load_ksf_file(path)?;
    let crop = crop_main_run(&ksf.frames, zones, Hull::css_stand())?;
    let replay = to_osxr(&ksf, crop.clone(), map, zones, meta_time_secs)?;
    Ok((replay, crop, ksf.header))
}

fn read_i32(bytes: &[u8], off: usize) -> Result<i32, KsfError> {
    let end = off
        .checked_add(4)
        .ok_or_else(|| KsfError::Format("i32 offset overflow".into()))?;
    let slice = bytes
        .get(off..end)
        .ok_or_else(|| KsfError::Format("i32 past EOF".into()))?;
    Ok(i32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_f32(bytes: &[u8], off: usize) -> Result<f32, KsfError> {
    let end = off
        .checked_add(4)
        .ok_or_else(|| KsfError::Format("f32 offset overflow".into()))?;
    let slice = bytes
        .get(off..end)
        .ok_or_else(|| KsfError::Format("f32 past EOF".into()))?;
    Ok(f32::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::parse_zones_json;
    use std::path::PathBuf;

    fn push_i32(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn push_f32(buf: &mut Vec<u8>, v: f32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_frame(
        buf: &mut Vec<u8>,
        buttons: i32,
        origin: [f32; 3],
        pitch: f32,
        yaw: f32,
        flags: i32,
        vel: [f32; 3],
    ) {
        push_i32(buf, buttons);
        for c in origin {
            push_f32(buf, c);
        }
        push_f32(buf, pitch);
        push_f32(buf, yaw);
        push_i32(buf, flags);
        for c in vel {
            push_f32(buf, c);
        }
    }

    fn minimal_v2(frames: &[[f32; 3]], bookmarks: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        push_i32(&mut buf, 2); // magic
        push_i32(&mut buf, 2); // secondary
        push_i32(&mut buf, frames.len() as i32);
        push_i32(&mut buf, bookmarks as i32);
        buf.resize(buf.len() + bookmarks * BOOKMARK_SIZE, 0);
        for (i, o) in frames.iter().enumerate() {
            let buttons = if i == 0 { IN_FORWARD | IN_MOVERIGHT } else { IN_JUMP };
            let flags = if i == 0 { FL_ONGROUND } else { 0 };
            write_frame(
                &mut buf,
                buttons,
                *o,
                10.0,
                -90.0,
                flags,
                [100.0, 0.0, -10.0],
            );
        }
        buf
    }

    #[test]
    fn parse_minimal_v2() {
        let bytes = minimal_v2(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], 0);
        let ksf = parse_ksf_bytes(&bytes).unwrap();
        assert_eq!(ksf.header.version, KsfVersion::V2);
        assert_eq!(ksf.header.tick_count, 2);
        assert_eq!(ksf.header.tick_size, 40);
        assert!((ksf.frames[0].origin.x - 1.0).abs() < 1e-5);
        assert!((ksf.frames[1].origin.z - 6.0).abs() < 1e-5);
        assert!(ksf.frames[0].flags & FL_ONGROUND != 0);
        let rf = ksf.frames[0].to_replay_frame();
        assert!((rf.forward_move - 1.0).abs() < 1e-5);
        assert!((rf.side_move - 1.0).abs() < 1e-5);
        assert!(rf.grounded);
        let rf1 = ksf.frames[1].to_replay_frame();
        assert!(rf1.buttons & BTN_JUMP != 0);
    }

    #[test]
    fn parse_v3_with_extra_cells() {
        let mut buf = Vec::new();
        push_i32(&mut buf, 3);
        push_i32(&mut buf, 1);
        push_i32(&mut buf, 1); // ticks
        push_i32(&mut buf, 0); // bookmarks
        push_i32(&mut buf, 12); // frame_cells (10 prefix + 2 extra)
        push_i32(&mut buf, 2); // extension_cells
        push_i32(&mut buf, 0);
        push_i32(&mut buf, 0); // extension payload
        write_frame(
            &mut buf,
            IN_DUCK,
            [9.0, 8.0, 7.0],
            1.0,
            2.0,
            0,
            [0.0, 0.0, 0.0],
        );
        push_i32(&mut buf, 111);
        push_i32(&mut buf, 222); // extra cells
        let ksf = parse_ksf_bytes(&buf).unwrap();
        assert_eq!(ksf.header.version, KsfVersion::V3);
        assert_eq!(ksf.header.tick_size, 48);
        assert!((ksf.frames[0].origin.x - 9.0).abs() < 1e-5);
        assert!(ksf.frames[0].to_replay_frame().buttons & BTN_DUCK != 0);
    }

    #[test]
    fn reject_bad_magic_and_truncation() {
        assert!(parse_ksf_bytes(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        let mut bytes = minimal_v2(&[[0.0, 0.0, 0.0]], 0);
        bytes.truncate(20);
        assert!(parse_ksf_bytes(&bytes).is_err());
        let mut neg = Vec::new();
        push_i32(&mut neg, 2);
        push_i32(&mut neg, 2);
        push_i32(&mut neg, -1);
        push_i32(&mut neg, 0);
        assert!(parse_ksf_bytes(&neg).is_err());
    }

    #[test]
    fn summit_wr_import_if_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let rec = root.join(
            "assets/replays/external/ksf/surf_summit/raw/replay_css_2946_0_712551_1745965307.rec",
        );
        if !rec.exists() {
            eprintln!("skip summit WR import test (file not cached): {}", rec.display());
            return;
        }
        let zones_text = std::fs::read_to_string(root.join("assets/zones/surf_summit.json")).unwrap();
        let zones = parse_zones_json(&zones_text).unwrap();
        let meta = 41.474906_f32;
        let (replay, crop, header) =
            import_ksf_to_replay(&rec, "surf_summit", &zones, Some(meta)).unwrap();
        assert_eq!(header.version, KsfVersion::V2);
        assert_eq!(header.tick_count, 3253);
        assert_eq!(crop.start_idx, 155);
        assert_eq!(crop.end_idx, 2919);
        assert_eq!(replay.frames.len(), 2765);
        assert_eq!(replay.header.style, STYLE_KSF_CSS_66T);
        assert!((replay.header.vars.airaccelerate - 100.0).abs() < 1e-5);
        assert!((replay.header.vars.accelerate - 10.0).abs() < 1e-5);
        let cropped = crop.duration_secs(TICK_INTERVAL);
        assert!(
            (cropped - meta).abs() < 0.02,
            "cropped {cropped} vs meta {meta}"
        );
        assert!(replay.frames.iter().all(|f| f.origin.x.is_finite()));
        assert!(
            !replay.header.splits.is_empty(),
            "expected checkpoint splits on summit"
        );
        // Sanity: first cropped frame outside start, last inside end.
        let hull = Hull::css_stand();
        assert!(!zones.main.start.contains_player(replay.frames[0].origin, hull));
        assert!(zones
            .main
            .end
            .contains_player(replay.frames.last().unwrap().origin, hull));
    }
}
