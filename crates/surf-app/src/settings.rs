//! Persisted player prefs (sens + HUD + readability + audio).
//!
//! `#[serde(default)]` on the struct is load-bearing twice over: it lets an old
//! settings file gain new keys, and it lets a *removed* key (the retired
//! `slope_tint` / `edge_highlight` readability toggles) be ignored rather than
//! failing the whole file to parse.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Ghost selection id stored in settings.
/// `"off"` | `"auto"` | `"pb"` | path to an `.osxr` (usually under `assets/replays/external/…`).
pub const GHOST_OFF: &str = "off";
/// KSF #1 when imported for the map, otherwise PB.
pub const GHOST_AUTO: &str = "auto";
pub const GHOST_PB: &str = "pb";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mouse_sens: f32,
    pub show_keys: bool,
    /// When true, present with AutoVsync/Fifo. When false, prefer Immediate.
    pub vsync: bool,
    /// Lightmap / albedo exposure multiplier (1.0 = map bake as-authored).
    pub brightness: f32,
    /// Lift dark lightmap luxels toward white (0 = authentic, ~0.5 = readable caves).
    pub shadow_lift: f32,
    /// Active ghost: [`GHOST_OFF`], [`GHOST_AUTO`], [`GHOST_PB`], or an `.osxr` path.
    pub ghost: String,
    /// Neon on/off-ramp trail behind the ghost while racing.
    pub ghost_trail: bool,
    /// Master switch for the procedural audio layer.
    pub audio: bool,
    /// Output level, 0..1.
    pub audio_volume: f32,
    /// Ramp-contact core (the resonant layer sync drives), 0..1.5.
    pub audio_core: f32,
    /// Speed-scaled broadband rush, 0..1.5.
    pub audio_air: f32,
    /// Sub layer — felt more than heard, and the most speaker-dependent. 0..1.5.
    pub audio_sub: f32,
    /// Failure treatment: `"rewind"` or `"dissolve"`. Neither is punishing;
    /// which wears better over a grind session is an open question.
    pub wipe_style: String,
    /// Key table, bind name → winit key name. See [`crate::binds`].
    pub binds: BTreeMap<String, String>,
    /// Degrees per second while a turn bind (Q / E) is held — `cl_yawspeed`.
    pub turn_speed: f32,
}

/// Default sensitivity, in CS:S's own units — our per-count scale is Source's
/// `m_yaw` / `m_pitch` default of 0.022, so this is the number a CS:S config
/// would carry.
pub const DEFAULT_SENS: f32 = 2.57;

impl Default for Settings {
    /// Max's own settings as of 2026-09-06, adopted wholesale so a fresh
    /// install starts where the tuning ended up rather than at the arbitrary
    /// first-guess values. Note `vsync: false` is deliberate and his call —
    /// see the frame-pacing notes in CLAUDE.md before changing it back.
    fn default() -> Self {
        Self {
            mouse_sens: DEFAULT_SENS,
            show_keys: false,
            vsync: false,
            // Neutral: the world shader now reproduces Source's own
            // `albedo * L`, so 1.0 / 0.0 *is* CS:S. (2026-09-06 — Max's 0.97 /
            // 0.5 were tuned against the old 4×-overbright, lift-to-white
            // pipeline and mean something else here.)
            brightness: 1.0,
            shadow_lift: 0.0,
            ghost: GHOST_PB.into(),
            ghost_trail: true,
            audio: true,
            audio_volume: 0.72,
            audio_core: 0.43,
            audio_air: 0.53,
            audio_sub: 0.58,
            wipe_style: "rewind".into(),
            binds: crate::binds::Binds::default().to_map(),
            turn_speed: crate::binds::DEFAULT_TURN_SPEED,
        }
    }
}

impl Settings {
    pub fn load_default() -> Self {
        let path = default_settings_path();
        Self::load_path(&path).unwrap_or_default()
    }

    pub fn load_path(path: &std::path::Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("settings read: {e}"))?;
        let mut s: Self = serde_json::from_str(&text).map_err(|e| format!("settings json: {e}"))?;
        s.mouse_sens = Self::clamp_sens(s.mouse_sens);
        s.brightness = Self::clamp_brightness(s.brightness);
        s.shadow_lift = Self::clamp_shadow_lift(s.shadow_lift);
        s.audio_volume = Self::clamp_audio_volume(s.audio_volume);
        s.audio_core = Self::clamp_audio_level(s.audio_core);
        s.audio_air = Self::clamp_audio_level(s.audio_air);
        s.audio_sub = Self::clamp_audio_level(s.audio_sub);
        s.turn_speed = Self::clamp_turn_speed(s.turn_speed);
        // Normalise: unknown names dropped, missing binds filled in, so the
        // file on disk always shows the whole table.
        s.binds = crate::binds::Binds::from_map(&s.binds).to_map();
        Ok(s)
    }

    pub fn save_default(&self) -> Result<(), String> {
        self.save_path(&default_settings_path())
    }

    pub fn save_path(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("settings dir: {e}"))?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|e| format!("settings serialize: {e}"))?;
        fs::write(path, text).map_err(|e| format!("settings write: {e}"))
    }

    /// CS:S's `sensitivity` range. The floor is well below anything playable
    /// and the ceiling well above it, so a typed value is only refused when it
    /// could not have been meant.
    pub fn clamp_sens(mut sens: f32) -> f32 {
        if !sens.is_finite() {
            sens = DEFAULT_SENS;
        }
        sens.clamp(0.1, 20.0)
    }

    pub fn clamp_brightness(mut v: f32) -> f32 {
        if !v.is_finite() {
            v = 1.0;
        }
        v.clamp(0.5, 3.0)
    }

    pub fn clamp_shadow_lift(mut v: f32) -> f32 {
        if !v.is_finite() {
            v = 0.0;
        }
        v.clamp(0.0, 0.9)
    }

    pub fn clamp_audio_volume(mut v: f32) -> f32 {
        if !v.is_finite() {
            v = Self::default().audio_volume;
        }
        v.clamp(0.0, 1.0)
    }

    pub fn clamp_turn_speed(mut v: f32) -> f32 {
        if !v.is_finite() {
            v = crate::binds::DEFAULT_TURN_SPEED;
        }
        v.clamp(30.0, 720.0)
    }

    /// Per-layer trims go above 1.0 so a layer that reads too quiet on a given
    /// pair of speakers can be pushed rather than only cut.
    pub fn clamp_audio_level(mut v: f32) -> f32 {
        if !v.is_finite() {
            v = 1.0;
        }
        v.clamp(0.0, 1.5)
    }
}

pub fn default_settings_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/mx-surf/settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn roundtrip() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-settings-{stamp}.json"));
        let s = Settings {
            mouse_sens: 7.5,
            show_keys: true,
            vsync: false,
            brightness: 1.4,
            shadow_lift: 0.3,
            ghost: "off".into(),
            ghost_trail: false,
            audio: false,
            audio_volume: 0.35,
            audio_core: 1.2,
            audio_air: 0.4,
            audio_sub: 0.0,
            wipe_style: "dissolve".into(),
            binds: BTreeMap::new(),
            turn_speed: 300.0,
        };
        s.save_path(&path).unwrap();
        let loaded = Settings::load_path(&path).unwrap();
        assert!(!loaded.audio);
        assert!((loaded.audio_volume - 0.35).abs() < 1e-5);
        assert!((loaded.audio_core - 1.2).abs() < 1e-5);
        assert!((loaded.audio_air - 0.4).abs() < 1e-5);
        assert_eq!(loaded.audio_sub, 0.0);
        assert_eq!(loaded.wipe_style, "dissolve");
        assert!((loaded.mouse_sens - 7.5).abs() < 1e-5);
        assert!(loaded.show_keys);
        assert!(!loaded.vsync);
        assert!((loaded.brightness - 1.4).abs() < 1e-5);
        assert!((loaded.shadow_lift - 0.3).abs() < 1e-5);
        assert_eq!(loaded.ghost, "off");
        assert!(!loaded.ghost_trail);
        assert!((loaded.turn_speed - 300.0).abs() < 1e-5);
        // An empty bind map on disk comes back as the full default table.
        assert_eq!(loaded.binds, crate::binds::Binds::default().to_map());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn binds_survive_a_round_trip_and_partial_files_keep_defaults() {
        use crate::binds::{Bind, Binds};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-settings-binds-{stamp}.json"));
        fs::write(
            &path,
            r#"{"binds":{"turn_left":"KeyZ","bogus":"KeyX","jump":"Escape"},"turn_speed":1e40}"#,
        )
        .unwrap();
        let loaded = Settings::load_path(&path).unwrap();
        let b = Binds::from_map(&loaded.binds);
        assert_eq!(b.key(Bind::TurnLeft), winit::keyboard::KeyCode::KeyZ);
        assert_eq!(b.key(Bind::Jump), winit::keyboard::KeyCode::Space);
        assert!(!loaded.binds.contains_key("bogus"));
        assert_eq!(loaded.turn_speed, crate::binds::DEFAULT_TURN_SPEED);
        let _ = fs::remove_file(&path);
    }

    /// The shipped defaults are Max's own settings (2026-09-06), not a first
    /// guess. A change here is a product decision, so it should have to be
    /// made twice.
    #[test]
    fn defaults_are_the_tuned_values() {
        let d = Settings::default();
        assert!((d.mouse_sens - 2.57).abs() < 1e-5);
        assert!(!d.vsync);
        assert!((d.brightness - 1.0).abs() < 1e-5);
        assert!((d.shadow_lift - 0.0).abs() < 1e-5);
        assert_eq!(d.ghost, GHOST_PB);
        assert!(d.ghost_trail);
        assert!(!d.show_keys);
        assert!(d.audio);
        assert!((d.audio_volume - 0.72).abs() < 1e-5);
        assert!((d.audio_core - 0.43).abs() < 1e-5);
        assert!((d.audio_air - 0.53).abs() < 1e-5);
        assert!((d.audio_sub - 0.58).abs() < 1e-5);
        assert_eq!(d.wipe_style, "rewind");
        // Every default has to survive its own clamp, or a fresh install
        // silently starts somewhere else.
        assert_eq!(Settings::clamp_sens(d.mouse_sens), d.mouse_sens);
        assert_eq!(Settings::clamp_brightness(d.brightness), d.brightness);
        assert_eq!(Settings::clamp_shadow_lift(d.shadow_lift), d.shadow_lift);
        assert_eq!(Settings::clamp_audio_volume(d.audio_volume), d.audio_volume);
        assert_eq!(Settings::clamp_audio_level(d.audio_core), d.audio_core);
        assert_eq!(Settings::clamp_turn_speed(d.turn_speed), d.turn_speed);
    }

    #[test]
    fn ghost_defaults_when_absent() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-settings-old-{stamp}.json"));
        fs::write(&path, r#"{"mouse_sens":5.0}"#).unwrap();
        let loaded = Settings::load_path(&path).unwrap();
        let d = Settings::default();
        assert_eq!(loaded.ghost, d.ghost);
        assert_eq!(loaded.ghost_trail, d.ghost_trail);
        // Settings files written before audio existed must still load, with
        // audio on at its default mix.
        assert!(loaded.audio);
        assert!((loaded.audio_volume - d.audio_volume).abs() < 1e-5);
        assert!((loaded.audio_core - d.audio_core).abs() < 1e-5);
        assert_eq!(loaded.wipe_style, d.wipe_style);
        // A file that predates the CS:S sensitivity range keeps its own value.
        assert!((loaded.mouse_sens - 5.0).abs() < 1e-5);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn out_of_range_audio_values_are_clamped_on_load() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-settings-bad-{stamp}.json"));
        fs::write(
            &path,
            r#"{"audio_volume":9.0,"audio_core":-3.0,"audio_air":99.0,"audio_sub":2.5}"#,
        )
        .unwrap();
        let loaded = Settings::load_path(&path).unwrap();
        assert_eq!(loaded.audio_volume, 1.0);
        assert_eq!(loaded.audio_core, 0.0);
        assert_eq!(loaded.audio_air, 1.5);
        assert_eq!(loaded.audio_sub, 1.5);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn non_finite_audio_values_fall_back_to_defaults() {
        // 1e40 overflows f32 to infinity on parse, so this exercises the
        // non-finite guard rather than the clamp.
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mx-surf-settings-inf-{stamp}.json"));
        fs::write(&path, r#"{"audio_volume":1e40,"audio_core":1e40,"mouse_sens":1e40}"#).unwrap();
        let loaded = Settings::load_path(&path).unwrap();
        assert!((loaded.audio_volume - Settings::default().audio_volume).abs() < 1e-5);
        // A corrupt per-layer trim falls back to unity gain, not to the mix
        // default — the point is an audible, neutral level.
        assert_eq!(loaded.audio_core, 1.0);
        assert!((loaded.mouse_sens - DEFAULT_SENS).abs() < 1e-5);
        let _ = fs::remove_file(&path);
    }
}
