//! Everything that belongs to *one loaded map*.
//!
//! `App` used to hold this inline, which meant the map was fixed at startup:
//! there was no way to express "throw that away and load a different one".
//! Grouping it here makes a map change a single assignment, and — the point of
//! the exercise — makes it a **compile error** to forget a field when loading,
//! because [`Session::new`] has to construct all of them.
//!
//! A `Session` always exists. The menus draw over a live world (the graybox
//! arena before anything is picked), so nothing here is optional and no caller
//! has to unwrap.

use surf_audio::EventDetector;
use surf_core::graybox::GrayboxMesh;
use surf_core::graybox::GrayboxWorld;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{MoveVars, PlayerState, UserCmd};
use surf_core::{graybox, tick, World};
use surf_map::{FieldState, LightmapAtlas, LoadedMap, MaterialAtlas, SkyboxAtlas};

use crate::locs::{LocStore, GRAYBOX_MAP};
use crate::pb::PbStore;
use crate::replay::{GhostOption, GhostPlayback, ReplayRecorder};
use crate::timer::RunTimer;
use crate::zones::{self, MapZones, TrackType};

/// The world the player is in: either the M0 graybox arena or a real BSP.
pub enum Level {
    Graybox(GrayboxWorld),
    Map(Box<LoadedMap>),
}

impl Level {
    pub fn world(&self) -> &World {
        match self {
            Level::Graybox(g) => &g.world,
            Level::Map(m) => &m.world,
        }
    }

    pub fn mesh(&self) -> &GrayboxMesh {
        match self {
            Level::Graybox(g) => &g.mesh,
            Level::Map(m) => &m.mesh,
        }
    }

    pub fn materials(&self) -> MaterialAtlas {
        match self {
            Level::Graybox(_) => MaterialAtlas::solid_white(),
            Level::Map(m) => m.materials.clone(),
        }
    }

    pub fn skybox(&self) -> SkyboxAtlas {
        match self {
            Level::Graybox(_) => SkyboxAtlas::none(),
            Level::Map(m) => m.skybox.clone(),
        }
    }

    pub fn lightmaps(&self) -> LightmapAtlas {
        match self {
            Level::Graybox(_) => LightmapAtlas::white_1x1(),
            Level::Map(m) => m.lightmaps.clone(),
        }
    }

    pub fn spawn(&self) -> (Vec3, Angle) {
        match self {
            Level::Graybox(g) => (g.spawn_origin, Angle::new(0.0, g.spawn_yaw, 0.0)),
            Level::Map(m) => (m.spawn_origin, m.spawn_angles),
        }
    }

    pub fn kill_z(&self) -> f32 {
        match self {
            Level::Graybox(_) => -2048.0,
            Level::Map(m) => m.kill_z,
        }
    }

    pub fn touch_teleport(&self, origin: Vec3) -> Option<(Vec3, Angle)> {
        match self {
            Level::Graybox(_) => None,
            Level::Map(m) => m.touch_teleport(origin),
        }
    }

    /// Feed the map's push / gravity / AddOutput effects into the player state
    /// for this tick. Graybox has no entities, so nothing to apply.
    pub fn apply_fields(&self, player: &mut PlayerState, state: &mut FieldState, dt: f32) {
        match self {
            Level::Graybox(_) => {}
            Level::Map(m) => m.apply_fields(player, state, dt),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Level::Graybox(_) => "mx-surf — graybox".into(),
            Level::Map(m) => format!("mx-surf — {}", m.name),
        }
    }

    pub fn map_name(&self) -> Option<&str> {
        match self {
            Level::Graybox(_) => None,
            Level::Map(m) => Some(m.name.as_str()),
        }
    }

    /// Name used to key per-map storage (locs, PBs). Graybox has its own.
    pub fn store_key(&self) -> &str {
        self.map_name().unwrap_or(GRAYBOX_MAP)
    }

    /// Short label for menus — the map name without the `surf_` prefix.
    pub fn short_name(&self) -> String {
        match self.map_name() {
            None => "graybox".into(),
            Some(n) => n.strip_prefix("surf_").unwrap_or(n).to_string(),
        }
    }
}

/// All per-map state. Dropped and rebuilt wholesale on a map change.
pub struct Session {
    pub level: Level,
    pub title_base: String,
    /// Per-map: zone JSON can override `maxvelocity` (summit is uncapped).
    pub vars: MoveVars,

    pub player: PlayerState,
    /// Previous tick's origin — the render interpolation span.
    pub prev_origin: Vec3,
    pub accumulator: f32,
    pub alpha: f32,
    pub sync_display: f32,

    pub zones: Option<MapZones>,
    pub run_timer: RunTimer,
    pub pb_time: Option<f32>,
    /// Live delta vs PB while running/finished; set on finish.
    pub pb_delta: Option<f32>,
    pub finish_recorded: bool,
    pub pb_flash_left: f32,
    pub replay_rec: ReplayRecorder,
    /// PB checkpoint splits (from pb.osxr header or derived).
    pub pb_splits: Vec<f32>,
    pub pb_ghost: Option<GhostPlayback>,
    /// Cached Esc-menu ghost choices for the loaded map.
    pub ghost_options: Vec<GhostOption>,
    pub notice_left: f32,
    pub notice_line: Option<String>,
    /// Last checkpoint reached this run (`"CP3"` / `"S2"`) and its split
    /// against the ghost's (else the PB's) time there. Persists on the HUD
    /// until the next checkpoint, unlike the old three-second flash.
    pub cp_label: Option<String>,
    pub cp_delta: Option<f32>,

    /// Saved practice locations for this map (Mouse2 save / Mouse1 load).
    pub locs: LocStore,
    /// Guard against a stray Mouse1 yanking you out of a real run: loadloc only
    /// works while this is on. Saveloc is always allowed. Off at every map load
    /// — a guard that remembers being disabled is not a guard.
    pub practice_mode: bool,
    /// Touch state for the map's `AddOutput` triggers (boosters, name gates).
    pub field_state: FieldState,

    pub audio_detect: EventDetector,
    /// Last tick's ramp contact, fed to the audio thread each frame.
    pub audio_on_ramp: bool,
}

impl Session {
    /// Build a session for `level`. `airaccelerate` carries the player's live
    /// tweak across a map change — it is a feel preference, not map data,
    /// unlike `maxvelocity` which the zone file owns.
    pub fn new(
        level: Level,
        zones: Option<MapZones>,
        pb_store: Option<&PbStore>,
        airaccelerate: Option<f32>,
    ) -> Self {
        let title_base = level.title();
        let (spawn_origin, spawn_angles) = level.spawn();

        let mut vars = MoveVars::momentum_surf();
        if let Some(aa) = airaccelerate {
            vars.airaccelerate = aa;
        }
        if let Some(mv) = zones.as_ref().and_then(|z| z.max_velocity) {
            vars.maxvelocity = mv;
            if mv <= 0.0 {
                println!("  maxvelocity=uncapped (zone override)");
            } else {
                println!("  maxvelocity={mv:.0} (zone override)");
            }
        }

        // Settle onto the spawn platform before the first frame, exactly as the
        // pre-Session startup path did.
        let mut player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        for _ in 0..10 {
            player = tick(level.world(), &player, &UserCmd::default(), &vars);
        }

        let track_type = zones
            .as_ref()
            .map(|z| z.track_type)
            .unwrap_or(TrackType::Linear);
        let pb_time = match (pb_store, level.map_name()) {
            (Some(store), Some(name)) => store.get(name).ok().flatten(),
            _ => None,
        };
        let locs = LocStore::load_for_map(level.store_key());
        if !locs.is_empty() {
            println!("  locs: {} saved", locs.len());
        }

        Self {
            prev_origin: player.origin,
            player,
            level,
            title_base,
            vars,
            accumulator: 0.0,
            alpha: 1.0,
            sync_display: 0.0,
            zones,
            run_timer: RunTimer::new(track_type),
            pb_time,
            pb_delta: None,
            finish_recorded: false,
            pb_flash_left: 0.0,
            replay_rec: ReplayRecorder::default(),
            pb_splits: Vec::new(),
            pb_ghost: None,
            ghost_options: Vec::new(),
            notice_left: 0.0,
            notice_line: None,
            cp_label: None,
            cp_delta: None,
            locs,
            practice_mode: false,
            field_state: FieldState::new(),
            audio_detect: EventDetector::default(),
            audio_on_ramp: false,
        }
    }

    /// The graybox arena — the world the menus sit in front of before a map is
    /// picked, and the M0 feel-check level.
    pub fn graybox(pb_store: Option<&PbStore>, airaccelerate: Option<f32>) -> Self {
        Self::new(
            Level::Graybox(graybox::surf_ramp_arena()),
            None,
            pb_store,
            airaccelerate,
        )
    }
}

/// Load a map and its zone file off the event loop. Returns the pieces a
/// [`Session`] needs; the caller does the GPU upload on the main thread.
pub fn load_level(path: &std::path::Path) -> Result<(Level, Option<MapZones>), String> {
    let map = LoadedMap::load_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!(
        "  brushes={}  tris={}  teleports={}  spawn=({:.0},{:.0},{:.0})",
        map.world.brushes.len(),
        map.mesh.tris.len(),
        map.teleports.len(),
        map.spawn_origin.x,
        map.spawn_origin.y,
        map.spawn_origin.z,
    );
    let zones = zones::load_for_map(path);
    Ok((Level::Map(Box::new(map)), zones))
}
