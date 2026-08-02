//! Momentum-inspired zone overlay (AABB MVP).
//!
//! Native format lives beside the map at `assets/zones/<map>.json`.
//! Axis-aligned boxes only — polygon regions later if a map needs them.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use surf_core::brush::Aabb;
use surf_core::math::Vec3;
use surf_core::movement::Hull;

/// Linear: fail teleports / kill-z do not stop the timer.
/// Staged: segment rules later; R still restarts the track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackType {
    Linear,
    Staged,
}

#[derive(Clone, Debug)]
pub struct ZoneBox {
    pub aabb: Aabb,
}

impl ZoneBox {
    pub fn contains_player(&self, origin: Vec3, hull: Hull) -> bool {
        aabb_overlap(
            origin + hull.mins,
            origin + hull.maxs,
            self.aabb.mins,
            self.aabb.maxs,
        )
    }
}

#[derive(Clone, Debug)]
pub struct TrackZones {
    pub limit_start_ground_speed: f32,
    pub start_on_jump: bool,
    pub start: ZoneBox,
    pub end: ZoneBox,
    pub checkpoints: Vec<ZoneBox>,
}

#[derive(Clone, Debug)]
pub struct MapZones {
    pub map_name: String,
    pub track_type: TrackType,
    pub main: TrackZones,
}

#[derive(Debug)]
pub enum ZoneError {
    Io(std::io::Error),
    Json(serde_json::Error),
    MissingMain,
    BadBox(&'static str),
}

impl std::fmt::Display for ZoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZoneError::Io(e) => write!(f, "zone io: {e}"),
            ZoneError::Json(e) => write!(f, "zone json: {e}"),
            ZoneError::MissingMain => write!(f, "zone file missing tracks.main"),
            ZoneError::BadBox(what) => write!(f, "invalid zone box: {what}"),
        }
    }
}

impl From<std::io::Error> for ZoneError {
    fn from(e: std::io::Error) -> Self {
        ZoneError::Io(e)
    }
}

impl From<serde_json::Error> for ZoneError {
    fn from(e: serde_json::Error) -> Self {
        ZoneError::Json(e)
    }
}

/// Resolve `assets/zones/<stem>.json` next to a `.bsp` path, or under cwd.
pub fn zones_path_for_map(map_path: &Path) -> PathBuf {
    let stem = map_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("map");
    if let Some(parent) = map_path.parent() {
        // assets/maps/foo.bsp → assets/zones/foo.json
        if parent.file_name().and_then(|n| n.to_str()) == Some("maps") {
            if let Some(assets) = parent.parent() {
                return assets.join("zones").join(format!("{stem}.json"));
            }
        }
    }
    PathBuf::from("assets/zones").join(format!("{stem}.json"))
}

pub fn load_zones_file(path: &Path) -> Result<MapZones, ZoneError> {
    let text = fs::read_to_string(path)?;
    parse_zones_json(&text)
}

pub fn parse_zones_json(text: &str) -> Result<MapZones, ZoneError> {
    let raw: RawMapZones = serde_json::from_str(text)?;
    let main = raw.tracks.main.ok_or(ZoneError::MissingMain)?;
    let start = zone_from_raw(&main.start, "start")?;
    let end = zone_from_raw(&main.end, "end")?;
    let mut checkpoints = Vec::with_capacity(main.checkpoints.len());
    for cp in &main.checkpoints {
        checkpoints.push(zone_from_raw(cp, "checkpoint")?);
    }
    let track_type = match raw.track_type.as_deref() {
        Some("staged") => TrackType::Staged,
        _ => TrackType::Linear,
    };
    Ok(MapZones {
        map_name: raw.map_name.unwrap_or_default(),
        track_type,
        main: TrackZones {
            limit_start_ground_speed: main.limit_start_ground_speed.unwrap_or(350.0),
            start_on_jump: main.start_on_jump.unwrap_or(true),
            start,
            end,
            checkpoints,
        },
    })
}

fn zone_from_raw(raw: &RawBox, label: &'static str) -> Result<ZoneBox, ZoneError> {
    if raw.mins.len() != 3 || raw.maxs.len() != 3 {
        return Err(ZoneError::BadBox(label));
    }
    let mins = Vec3::new(raw.mins[0], raw.mins[1], raw.mins[2]);
    let maxs = Vec3::new(raw.maxs[0], raw.maxs[1], raw.maxs[2]);
    Ok(ZoneBox {
        aabb: Aabb::from_mins_maxs(
            Vec3::new(mins.x.min(maxs.x), mins.y.min(maxs.y), mins.z.min(maxs.z)),
            Vec3::new(mins.x.max(maxs.x), mins.y.max(maxs.y), mins.z.max(maxs.z)),
        ),
    })
}

fn aabb_overlap(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
    a_mins.x <= b_maxs.x
        && a_maxs.x >= b_mins.x
        && a_mins.y <= b_maxs.y
        && a_maxs.y >= b_mins.y
        && a_mins.z <= b_maxs.z
        && a_maxs.z >= b_mins.z
}

#[derive(Deserialize)]
struct RawMapZones {
    #[serde(rename = "mapName")]
    map_name: Option<String>,
    #[serde(rename = "trackType")]
    track_type: Option<String>,
    tracks: RawTracks,
}

#[derive(Deserialize)]
struct RawTracks {
    main: Option<RawTrack>,
}

#[derive(Deserialize)]
struct RawTrack {
    #[serde(rename = "limitStartGroundSpeed")]
    limit_start_ground_speed: Option<f32>,
    #[serde(rename = "startOnJump")]
    start_on_jump: Option<bool>,
    start: RawBox,
    end: RawBox,
    #[serde(default)]
    checkpoints: Vec<RawBox>,
}

#[derive(Deserialize)]
struct RawBox {
    mins: Vec<f32>,
    maxs: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use surf_core::movement::Hull;

    const SAMPLE: &str = r#"{
      "formatVersion": 1,
      "mapName": "test_map",
      "trackType": "linear",
      "tracks": {
        "main": {
          "limitStartGroundSpeed": 350.0,
          "startOnJump": true,
          "start": { "mins": [0, 0, 0], "maxs": [100, 100, 80] },
          "end": { "mins": [500, 0, 0], "maxs": [600, 100, 80] },
          "checkpoints": []
        }
      }
    }"#;

    #[test]
    fn parse_minimal_zones() {
        let z = parse_zones_json(SAMPLE).expect("parse");
        assert_eq!(z.map_name, "test_map");
        assert_eq!(z.track_type, TrackType::Linear);
        assert!((z.main.limit_start_ground_speed - 350.0).abs() < 1e-5);
        assert!(z.main.start_on_jump);
        assert!(z.main.start.contains_player(Vec3::new(50.0, 50.0, 0.0), Hull::css_stand()));
        assert!(!z.main.start.contains_player(Vec3::new(200.0, 50.0, 0.0), Hull::css_stand()));
        assert!(z.main.end.contains_player(Vec3::new(550.0, 50.0, 0.0), Hull::css_stand()));
    }

    #[test]
    fn zones_path_from_bsp() {
        let p = zones_path_for_map(Path::new("assets/maps/surf_summit.bsp"));
        assert_eq!(p, PathBuf::from("assets/zones/surf_summit.json"));
    }

    #[test]
    fn load_summit_zones_file() {
        let path = Path::new("assets/zones/surf_summit.json");
        if !path.is_file() {
            // Workspace root may differ when tests run elsewhere.
            return;
        }
        let z = load_zones_file(path).expect("summit zones");
        assert_eq!(z.map_name, "surf_summit");
        // Spawn 1600,0,11552 should be inside start.
        assert!(z
            .main
            .start
            .contains_player(Vec3::new(1600.0, 0.0, 11552.0), Hull::css_stand()));
    }
}
