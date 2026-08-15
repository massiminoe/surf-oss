//! osx-surf app: graybox (M0), real BSP (M1), timer/zones (M2).
//!
//! Usage:
//!   cargo run -p surf-app --release
//!   cargo run -p surf-app --release -- assets/maps/surf_summit.bsp
//!   cargo run -p surf-app --release -- --graybox
//!   cargo run -p surf-app --release -- --ghost path/to/run.osxr
//!   cargo run -p surf-app --release -- --perf-secs 8 --size 2560x1440
//!   cargo run -p surf-app --release -- --no-vsync
//!
//! Controls: WASD move, mouse look, Space jump (autobhop), Ctrl duck, R full reset,
//! T stage reset (staged maps), Esc menu/pause (ghost picker + trail toggle),
//! [ ] sens, - = airaccel.
//! Readability (brightness / edges) and audio (volume, core/air/sub levels,
//! wipe style) in Esc. Perf line logs to stdout once per second;
//! `--perf-secs N` exits after N.
//!
//! macOS note: NSEvent mouse deltas are OS-accelerated. For fair feel, disable
//! pointer acceleration (System Settings → Mouse → Pointer acceleration off),
//! or: `defaults write -g com.apple.mouse.scaling -integer -1`

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use surf_app::pb::PbStore;
use surf_app::replay::{self, derive_splits, GhostPlayback, GhostOption, Replay, ReplayRecorder};
use surf_app::settings::{Settings, GHOST_AUTO, GHOST_OFF, GHOST_PB};
use surf_app::timer::{format_split_line, format_time, RunTimer, TimerPhase};
use surf_app::zones::{self, MapZones, TrackType};
use surf_core::graybox::{self, GrayboxMesh, GrayboxWorld};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};
use surf_audio::{
    AudioEngine, AudioEvent, EventDetector, Levels as AudioLevels, Observation,
    Params as AudioParams, WipeStyle,
};
use surf_core::{air_strafe_sync, is_on_surf_ramp, tick, World};
use surf_map::{LightmapAtlas, LoadedMap, MaterialAtlas, SkyboxAtlas};
use surf_render::{
    Camera, GhostPose, GpuMesh, HudState, HudTimerPhase, MenuHud, MenuRecentEntry, Renderer,
    ShowKeysState, TrailPoint, ViewParams,
};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Source m_yaw / m_pitch defaults.
const MOUSE_YAW_SCALE: f32 = 0.022;
const MOUSE_PITCH_SCALE: f32 = 0.022;

/// Adjustable / action menu rows (not counting read-only recent footer).
/// Must stay <= the renderer's `menu_bufs` pool or extra rows vanish.
const MENU_ITEM_COUNT: usize = 18;
const MENU_SENS: usize = 0;
const MENU_BRIGHTNESS: usize = 1;
const MENU_SHADOW_LIFT: usize = 2;
const MENU_SLOPE_TINT: usize = 3;
const MENU_EDGE_HIGHLIGHT: usize = 4;
const MENU_GHOST: usize = 5;
const MENU_GHOST_TRAIL: usize = 6;
const MENU_SYNC: usize = 7;
const MENU_KEYS: usize = 8;
const MENU_VSYNC: usize = 9;
const MENU_AA: usize = 10;
const MENU_AUDIO: usize = 11;
const MENU_AUDIO_VOLUME: usize = 12;
const MENU_AUDIO_CORE: usize = 13;
const MENU_AUDIO_AIR: usize = 14;
const MENU_AUDIO_SUB: usize = 15;
const MENU_WIPE: usize = 16;
const MENU_QUIT: usize = 17;

/// Rolling frame-time samples for FPS / 1% low display.
const FRAME_STAT_SAMPLES: usize = 180;

struct FrameStats {
    /// Recent frame deltas (seconds), oldest→newest ring.
    dts: Vec<f32>,
    /// Displayed averages (updated ~4×/s).
    fps: f32,
    frame_ms: f32,
    pct1_fps: f32,
    sim_ms: f32,
    render_ms: f32,
    width: u32,
    height: u32,
    /// Accumulators since last display refresh.
    acc_sim_ms: f32,
    acc_render_ms: f32,
    acc_frames: u32,
    last_refresh: Instant,
    last_log: Instant,
    total_frames: u64,
}

impl FrameStats {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            dts: Vec::with_capacity(FRAME_STAT_SAMPLES),
            fps: 0.0,
            frame_ms: 0.0,
            pct1_fps: 0.0,
            sim_ms: 0.0,
            render_ms: 0.0,
            width: 0,
            height: 0,
            acc_sim_ms: 0.0,
            acc_render_ms: 0.0,
            acc_frames: 0,
            last_refresh: now,
            last_log: now,
            total_frames: 0,
        }
    }

    fn push(&mut self, dt: f32, sim_ms: f32, render_ms: f32, width: u32, height: u32) {
        let dt = dt.clamp(1.0e-6, 0.25);
        if self.dts.len() >= FRAME_STAT_SAMPLES {
            self.dts.remove(0);
        }
        self.dts.push(dt);
        self.acc_sim_ms += sim_ms;
        self.acc_render_ms += render_ms;
        self.acc_frames += 1;
        self.total_frames += 1;
        self.width = width;
        self.height = height;

        if self.last_refresh.elapsed().as_secs_f32() >= 0.25 {
            self.refresh_display();
        }
        if self.last_log.elapsed().as_secs_f32() >= 1.0 {
            self.last_log = Instant::now();
            if let Some(line) = self.line() {
                println!("perf: {line}");
            }
        }
    }

    fn refresh_display(&mut self) {
        self.last_refresh = Instant::now();
        if self.dts.is_empty() {
            return;
        }
        let sum: f32 = self.dts.iter().sum();
        let n = self.dts.len() as f32;
        self.fps = n / sum.max(1.0e-6);
        self.frame_ms = (sum / n) * 1000.0;
        self.pct1_fps = pct1_low_fps(&self.dts);
        if self.acc_frames > 0 {
            let f = self.acc_frames as f32;
            self.sim_ms = self.acc_sim_ms / f;
            self.render_ms = self.acc_render_ms / f;
            self.acc_sim_ms = 0.0;
            self.acc_render_ms = 0.0;
            self.acc_frames = 0;
        }
    }

    fn line(&self) -> Option<String> {
        if self.fps <= 0.0 {
            return None;
        }
        Some(format!(
            "{:.0} fps  {:.1} ms  1% {:.0}  sim {:.1}  r {:.1}  {}x{}",
            self.fps,
            self.frame_ms,
            self.pct1_fps,
            self.sim_ms,
            self.render_ms,
            self.width,
            self.height
        ))
    }
}

struct LaunchOpts {
    level: Level,
    zones: Option<MapZones>,
    window_w: u32,
    window_h: u32,
    /// Exit after this many seconds of rendering (agent / CI sampling).
    perf_secs: Option<f32>,
    /// CLI override for settings.vsync (`None` = keep settings.json).
    vsync: Option<bool>,
    /// Optional `.osxr` ghost (overrides PB ghost). Used for KSF imports.
    ghost_path: Option<PathBuf>,
}

/// Average FPS of the slowest 1% of frames in `dts` (seconds).
fn pct1_low_fps(dts: &[f32]) -> f32 {
    if dts.is_empty() {
        return 0.0;
    }
    let mut sorted = dts.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = ((sorted.len() as f32) * 0.01).ceil() as usize;
    let n = n.max(1);
    let worst = &sorted[sorted.len() - n..];
    let mean_dt = worst.iter().sum::<f32>() / n as f32;
    1.0 / mean_dt.max(1.0e-6)
}

const PB_FLASH_SECS: f32 = 2.0;
const SPLIT_FLASH_SECS: f32 = 2.5;

enum Level {
    Graybox(GrayboxWorld),
    Map(LoadedMap),
}

impl Level {
    fn world(&self) -> &World {
        match self {
            Level::Graybox(g) => &g.world,
            Level::Map(m) => &m.world,
        }
    }

    fn mesh(&self) -> &GrayboxMesh {
        match self {
            Level::Graybox(g) => &g.mesh,
            Level::Map(m) => &m.mesh,
        }
    }

    fn materials(&self) -> MaterialAtlas {
        match self {
            Level::Graybox(_) => MaterialAtlas::solid_white(),
            Level::Map(m) => m.materials.clone(),
        }
    }

    fn skybox(&self) -> SkyboxAtlas {
        match self {
            Level::Graybox(_) => SkyboxAtlas::none(),
            Level::Map(m) => m.skybox.clone(),
        }
    }

    fn lightmaps(&self) -> LightmapAtlas {
        match self {
            Level::Graybox(_) => LightmapAtlas::white_1x1(),
            Level::Map(m) => m.lightmaps.clone(),
        }
    }

    fn spawn(&self) -> (Vec3, Angle) {
        match self {
            Level::Graybox(g) => (g.spawn_origin, Angle::new(0.0, g.spawn_yaw, 0.0)),
            Level::Map(m) => (m.spawn_origin, m.spawn_angles),
        }
    }

    fn kill_z(&self) -> f32 {
        match self {
            Level::Graybox(_) => -2048.0,
            Level::Map(m) => m.kill_z,
        }
    }

    fn touch_teleport(&self, origin: Vec3) -> Option<(Vec3, Angle)> {
        match self {
            Level::Graybox(_) => None,
            Level::Map(m) => m.touch_teleport(origin),
        }
    }

    fn touch_push(&self, origin: Vec3) -> Vec3 {
        match self {
            Level::Graybox(_) => Vec3::ZERO,
            Level::Map(m) => m.touch_push(origin),
        }
    }

    fn touch_gravity(&self, origin: Vec3) -> f32 {
        match self {
            Level::Graybox(_) => 1.0,
            Level::Map(m) => m.touch_gravity(origin),
        }
    }

    fn title(&self) -> String {
        match self {
            Level::Graybox(_) => "osx-surf M0 — graybox".into(),
            Level::Map(m) => format!("osx-surf — {}", m.name),
        }
    }

    fn map_name(&self) -> Option<&str> {
        match self {
            Level::Graybox(_) => None,
            Level::Map(m) => Some(m.name.as_str()),
        }
    }
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    renderer: Option<Renderer>,
    level: Level,
    title_base: String,
    player: PlayerState,
    vars: MoveVars,
    keys: HashSet<KeyCode>,
    mouse_captured: bool,
    menu_open: bool,
    menu_selected: usize,
    settings: Settings,
    accumulator: f32,
    last_frame: Instant,
    prev_origin: Vec3,
    alpha: f32,
    hud_timer: f32,
    sync_display: f32,
    zones: Option<MapZones>,
    run_timer: RunTimer,
    pb_store: Option<PbStore>,
    pb_time: Option<f32>,
    /// Live delta vs PB while running/finished; set on finish.
    pb_delta: Option<f32>,
    finish_recorded: bool,
    pb_flash_left: f32,
    recent_count: u32,
    recent_runs: Vec<MenuRecentEntry>,
    replay_rec: ReplayRecorder,
    frame_stats: FrameStats,
    /// PB checkpoint splits (from pb.osxr header or derived).
    pb_splits: Vec<f32>,
    pb_ghost: Option<GhostPlayback>,
    /// Cached Esc-menu ghost choices for the loaded map.
    ghost_options: Vec<GhostOption>,
    split_flash_left: f32,
    split_flash_line: Option<String>,
    window_w: u32,
    window_h: u32,
    perf_secs: Option<f32>,
    /// Set when the first frame renders (for `--perf-secs`).
    perf_started: Option<Instant>,
    /// Surface supports `PresentMode::Immediate` (no VSync).
    immediate_ok: bool,
    /// `None` when no output device was available — the game runs on silently.
    audio: Option<AudioEngine>,
    audio_detect: EventDetector,
    /// Last tick's ramp contact, fed to the audio thread each frame.
    audio_on_ramp: bool,
}

impl App {
    fn new(opts: LaunchOpts) -> Self {
        let LaunchOpts {
            level,
            zones,
            window_w,
            window_h,
            perf_secs,
            vsync,
            ghost_path,
        } = opts;
        let title_base = level.title();
        let (spawn_origin, spawn_angles) = level.spawn();
        let mut player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        let mut vars = MoveVars::momentum_surf();
        if let Some(mv) = zones.as_ref().and_then(|z| z.max_velocity) {
            vars.maxvelocity = mv;
            if mv <= 0.0 {
                println!("  maxvelocity=uncapped (zone override)");
            } else {
                println!("  maxvelocity={mv:.0} (zone override)");
            }
        }
        for _ in 0..10 {
            player = tick(level.world(), &player, &UserCmd::default(), &vars);
        }
        let track_type = zones
            .as_ref()
            .map(|z| z.track_type)
            .unwrap_or(TrackType::Linear);
        let pb_store = match PbStore::open_default() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("PB store unavailable ({e}); times won't be saved");
                None
            }
        };
        let pb_time = match (&pb_store, level.map_name()) {
            (Some(store), Some(name)) => store.get(name).ok().flatten(),
            _ => None,
        };
        let mut settings = Settings::load_default();
        if let Some(v) = vsync {
            settings.vsync = v;
        }
        if let Some(p) = ghost_path {
            settings.ghost = replay::path_to_ghost_id(&p);
        }
        let audio = match AudioEngine::new(
            audio_levels(&settings),
            settings.audio,
            WipeStyle::from_str_or_default(&settings.wipe_style),
        ) {
            Ok(engine) => {
                println!(
                    "  audio: {} Hz, {} ch (CORE.A maglev, wipe={})",
                    engine.sample_rate(),
                    engine.channels(),
                    settings.wipe_style
                );
                Some(engine)
            }
            Err(e) => {
                eprintln!("audio unavailable ({e}); running silent");
                None
            }
        };

        let mut app = Self {
            window: None,
            surface: None,
            renderer: None,
            prev_origin: player.origin,
            player,
            level,
            title_base,
            vars,
            keys: HashSet::new(),
            mouse_captured: false,
            menu_open: false,
            menu_selected: 0,
            settings,
            accumulator: 0.0,
            last_frame: Instant::now(),
            alpha: 1.0,
            hud_timer: 0.0,
            sync_display: 0.0,
            zones,
            run_timer: RunTimer::new(track_type),
            pb_store,
            pb_time,
            pb_delta: None,
            finish_recorded: false,
            pb_flash_left: 0.0,
            recent_count: 0,
            recent_runs: Vec::new(),
            replay_rec: ReplayRecorder::default(),
            frame_stats: FrameStats::new(),
            pb_splits: Vec::new(),
            pb_ghost: None,
            ghost_options: Vec::new(),
            split_flash_left: 0.0,
            split_flash_line: None,
            window_w,
            window_h,
            perf_secs,
            perf_started: None,
            immediate_ok: false,
            audio,
            audio_detect: EventDetector::default(),
            audio_on_ramp: false,
        };
        app.refresh_ghost_options();
        app.load_ghost();
        app.refresh_recent_footer();
        app
    }

    fn refresh_ghost_options(&mut self) {
        let Some(name) = self.level.map_name().map(|s| s.to_string()) else {
            self.ghost_options = vec![GhostOption {
                id: GHOST_OFF.into(),
                label: "Off".into(),
                path: None,
            }];
            return;
        };
        self.ghost_options = replay::ghost_catalog(&name, &self.settings.ghost);
    }

    fn ghost_label(&self) -> String {
        self.ghost_options
            .iter()
            .find(|o| o.id == self.settings.ghost)
            .map(|o| o.label.clone())
            .unwrap_or_else(|| self.settings.ghost.clone())
    }

    fn cycle_ghost(&mut self, dir: i32) {
        self.refresh_ghost_options();
        if self.ghost_options.is_empty() {
            return;
        }
        let cur = self
            .ghost_options
            .iter()
            .position(|o| o.id == self.settings.ghost)
            .unwrap_or(0);
        let n = self.ghost_options.len() as i32;
        let next = (cur as i32 + dir).rem_euclid(n) as usize;
        self.settings.ghost = self.ghost_options[next].id.clone();
        self.load_ghost();
    }

    fn load_ghost(&mut self) {
        let Some(name) = self.level.map_name().map(|s| s.to_string()) else {
            self.pb_ghost = None;
            self.pb_splits.clear();
            return;
        };
        if self.settings.ghost == GHOST_OFF {
            self.pb_ghost = None;
            self.pb_splits.clear();
            println!("  ghost off");
            return;
        }
        let Some(path) = replay::resolve_ghost_path(&name, &self.settings.ghost) else {
            self.pb_ghost = None;
            self.pb_splits.clear();
            return;
        };
        let kind = match self.settings.ghost.as_str() {
            GHOST_AUTO => "Auto",
            GHOST_PB => "PB",
            _ => "External",
        };
        match Replay::load(&path) {
            Ok(mut replay) => {
                if replay.header.splits.is_empty() {
                    if let Some(zones) = self.zones.as_ref() {
                        replay.header.splits = derive_splits(
                            &replay.frames,
                            &zones.main.checkpoints,
                            replay.header.tick_interval,
                            Hull::css_stand(),
                        );
                    }
                }
                self.pb_splits = replay.header.splits.clone();
                println!(
                    "  {} ghost loaded ({} frames, {} splits, style={}) from {}",
                    kind,
                    replay.header.frame_count,
                    self.pb_splits.len(),
                    replay.header.style,
                    path.display()
                );
                let mut ghost = GhostPlayback::from_replay(replay);
                ghost.classify_ramp_contact(self.level.world());
                self.pb_ghost = Some(ghost);
            }
            Err(e) => {
                if self.settings.ghost != GHOST_PB && self.settings.ghost != GHOST_AUTO {
                    eprintln!("  ghost load failed ({}): {e}", path.display());
                } else if self.settings.ghost == GHOST_AUTO {
                    eprintln!("  auto ghost load failed ({}): {e}", path.display());
                }
                self.pb_ghost = None;
                self.pb_splits.clear();
            }
        }
    }

    fn refresh_recent_footer(&mut self) {
        self.recent_runs.clear();
        self.recent_count = 0;
        let Some(name) = self.level.map_name() else {
            return;
        };
        let Some(store) = self.pb_store.as_ref() else {
            return;
        };
        let Ok(recent) = store.list_recent(name, 5) else {
            return;
        };
        let Ok(count) = store.count_completions(name) else {
            return;
        };
        self.recent_count = count as u32;
        for r in recent {
            self.recent_runs.push(MenuRecentEntry {
                time: format_time(r.time_secs),
                is_pb: r.is_pb,
            });
        }
    }

    fn persist_settings(&self) {
        if let Err(e) = self.settings.save_default() {
            eprintln!("settings save failed: {e}");
        }
    }

    /// Push the audio-related settings across to the audio thread. Cheap enough
    /// to call on every menu adjustment, which is what makes tuning by ear work.
    fn apply_audio_settings(&self) {
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        audio.set_enabled(self.settings.audio);
        audio.set_levels(audio_levels(&self.settings));
        audio.set_wipe_style(WipeStyle::from_str_or_default(&self.settings.wipe_style));
    }

    fn open_menu(&mut self) {
        self.menu_open = true;
        self.menu_selected = 0;
        self.set_capture(false);
        self.refresh_ghost_options();
        self.refresh_recent_footer();
        // The sim is paused, so nothing would update the parameters — zero them
        // so the voice releases instead of holding a note under the menu.
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
        }
    }

    fn close_menu(&mut self, recapture: bool) {
        self.menu_open = false;
        self.persist_settings();
        if recapture {
            self.set_capture(true);
        }
    }

    fn menu_adjust(&mut self, dir: i32) {
        match self.menu_selected {
            MENU_SENS => {
                let step = if dir > 0 { 1.25 } else { 1.0 / 1.25 };
                self.settings.mouse_sens =
                    Settings::clamp_sens(self.settings.mouse_sens * step);
            }
            MENU_BRIGHTNESS => {
                let step = if dir > 0 { 0.1 } else { -0.1 };
                self.settings.brightness =
                    Settings::clamp_brightness(self.settings.brightness + step);
            }
            MENU_SHADOW_LIFT => {
                let step = if dir > 0 { 0.1 } else { -0.1 };
                self.settings.shadow_lift =
                    Settings::clamp_shadow_lift(self.settings.shadow_lift + step);
            }
            MENU_SLOPE_TINT => {
                self.settings.slope_tint = !self.settings.slope_tint;
            }
            MENU_EDGE_HIGHLIGHT => {
                self.settings.edge_highlight = !self.settings.edge_highlight;
            }
            MENU_GHOST => {
                self.cycle_ghost(dir);
            }
            MENU_GHOST_TRAIL => {
                self.settings.ghost_trail = !self.settings.ghost_trail;
            }
            MENU_SYNC => {
                self.settings.show_sync_bar = !self.settings.show_sync_bar;
            }
            MENU_KEYS => {
                self.settings.show_keys = !self.settings.show_keys;
            }
            MENU_VSYNC => {
                let want_vsync = !self.settings.vsync;
                if !want_vsync && !self.immediate_ok {
                    println!("VSync: Immediate not available on this surface; staying on");
                } else {
                    self.settings.vsync = want_vsync;
                    self.apply_present_mode();
                }
            }
            MENU_AA => {
                if dir > 0 {
                    self.vars.airaccelerate = (self.vars.airaccelerate * 1.5).min(1000.0);
                } else {
                    self.vars.airaccelerate = (self.vars.airaccelerate / 1.5).max(1.0);
                }
            }
            MENU_AUDIO => {
                self.settings.audio = !self.settings.audio;
            }
            MENU_AUDIO_VOLUME => {
                let step = if dir > 0 { 0.05 } else { -0.05 };
                self.settings.audio_volume =
                    Settings::clamp_audio_volume(self.settings.audio_volume + step);
            }
            MENU_AUDIO_CORE => {
                let step = if dir > 0 { 0.1 } else { -0.1 };
                self.settings.audio_core =
                    Settings::clamp_audio_level(self.settings.audio_core + step);
            }
            MENU_AUDIO_AIR => {
                let step = if dir > 0 { 0.1 } else { -0.1 };
                self.settings.audio_air =
                    Settings::clamp_audio_level(self.settings.audio_air + step);
            }
            MENU_AUDIO_SUB => {
                let step = if dir > 0 { 0.1 } else { -0.1 };
                self.settings.audio_sub =
                    Settings::clamp_audio_level(self.settings.audio_sub + step);
            }
            MENU_WIPE => {
                let next = WipeStyle::from_str_or_default(&self.settings.wipe_style).toggled();
                self.settings.wipe_style = next.as_str().into();
                // Play it on selection: this is a choice you make by ear.
                if let Some(audio) = self.audio.as_ref() {
                    audio.set_wipe_style(next);
                    audio.push(AudioEvent::Wipe);
                }
            }
            MENU_QUIT => {}
            _ => {}
        }
        self.apply_audio_settings();
    }

    fn present_mode_for_settings(&self) -> wgpu::PresentMode {
        if self.settings.vsync || !self.immediate_ok {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::Immediate
        }
    }

    fn apply_present_mode(&mut self) {
        let mode = self.present_mode_for_settings();
        if let (Some(renderer), Some(surface)) =
            (self.renderer.as_mut(), self.surface.as_ref())
        {
            if renderer.config.present_mode == mode {
                return;
            }
            renderer.config.present_mode = mode;
            surface.configure(&renderer.device, &renderer.config);
            println!(
                "present_mode={mode:?}  vsync={}",
                self.settings.vsync
            );
        }
    }

    fn build_menu_hud(&self) -> MenuHud {
        let on = |b: bool| if b { "On" } else { "Off" };
        let vsync_label = if self.settings.vsync {
            "On".into()
        } else if self.immediate_ok {
            "Off".into()
        } else {
            "On*".into()
        };
        let items = vec![
            (
                "Mouse sens".into(),
                format!("{:.1}", self.settings.mouse_sens),
            ),
            (
                "Brightness".into(),
                format!("{:.1}", self.settings.brightness),
            ),
            (
                "Shadow lift".into(),
                format!("{:.1}", self.settings.shadow_lift),
            ),
            (
                "Slope tint".into(),
                on(self.settings.slope_tint).into(),
            ),
            (
                "Edge highlight".into(),
                on(self.settings.edge_highlight).into(),
            ),
            ("Ghost".into(), self.ghost_label()),
            (
                "Ghost trail".into(),
                on(self.settings.ghost_trail).into(),
            ),
            (
                "Show sync bar".into(),
                on(self.settings.show_sync_bar).into(),
            ),
            ("Show keys".into(), on(self.settings.show_keys).into()),
            ("VSync".into(), vsync_label),
            (
                "Airaccelerate".into(),
                format!("{:.0}", self.vars.airaccelerate),
            ),
            (
                "Audio".into(),
                if self.audio.is_some() {
                    on(self.settings.audio).into()
                } else {
                    "No device".into()
                },
            ),
            (
                "Audio volume".into(),
                format!("{:.2}", self.settings.audio_volume),
            ),
            ("Core level".into(), format!("{:.1}", self.settings.audio_core)),
            ("Air level".into(), format!("{:.1}", self.settings.audio_air)),
            ("Sub level".into(), format!("{:.1}", self.settings.audio_sub)),
            (
                "Wipe sound".into(),
                match WipeStyle::from_str_or_default(&self.settings.wipe_style) {
                    WipeStyle::Rewind => "Rewind".into(),
                    WipeStyle::Dissolve => "Dissolve".into(),
                },
            ),
            ("Quit".into(), String::new()),
        ];
        MenuHud {
            selected: self.menu_selected,
            items,
            hint: "↑↓ select   ←→ / A D adjust   Esc resume".into(),
            recent_count: self.recent_count,
            recent: self.recent_runs.clone(),
        }
    }

    fn reset(&mut self) {
        let (spawn_origin, spawn_angles) = self.level.spawn();
        self.player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        for _ in 0..10 {
            self.player = tick(
                self.level.world(),
                &self.player,
                &UserCmd::default(),
                &self.vars,
            );
        }
        self.prev_origin = self.player.origin;
        self.accumulator = 0.0;
        self.sync_display = 0.0;
        self.run_timer.reset();
        self.pb_delta = None;
        self.finish_recorded = false;
        self.pb_flash_left = 0.0;
        self.split_flash_left = 0.0;
        self.split_flash_line = None;
        self.replay_rec.clear();
        if let Some(g) = self.pb_ghost.as_mut() {
            g.stop();
        }
        // Re-baseline the edge detector so respawning can't fire a phantom
        // landing, and mark the restart with the same quiet tick the start zone
        // uses. A manual reset is not a failure and doesn't get a wipe.
        self.audio_on_ramp = false;
        self.audio_detect
            .resync(false, self.player.grounded);
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
            audio.push(AudioEvent::Rearm);
        }
    }

    /// Snap to the current stage start without clearing the run clock / splits.
    /// Linear maps fall back to a full [`Self::reset`].
    fn reset_stage(&mut self) {
        if self.zones.is_none() || self.run_timer.track_type != TrackType::Staged {
            self.reset();
            return;
        }
        let stage = self.run_timer.current_stage;
        let angles = self.player.viewangles;
        let (spawn_origin, spawn_angles) = {
            let zones = self.zones.as_ref().unwrap();
            stage_respawn_pose(zones, &self.level, stage, angles)
        };
        self.player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        for _ in 0..10 {
            self.player = tick(
                self.level.world(),
                &self.player,
                &UserCmd::default(),
                &self.vars,
            );
        }
        self.prev_origin = self.player.origin;
        self.accumulator = 0.0;
        // Keep phase / time / splits / current_stage; soft-enter so landing
        // inside a stage start doesn't cancel or double-split.
        self.run_timer.notify_soft_respawn();
        if let Some(zones) = self.zones.as_ref() {
            self.run_timer
                .sync_stage_after_respawn(zones, &self.player);
        }
        self.audio_on_ramp = false;
        self.audio_detect
            .resync(false, self.player.grounded);
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
            audio.push(AudioEvent::Rearm);
        }
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&self.title_base)
                        .with_inner_size(PhysicalSize::new(self.window_w, self.window_h)),
                )
                .expect("window"),
        );

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL | wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("osx-surf"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .expect("device");

        let size = window.inner_size();
        let info = adapter.get_info();
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        self.immediate_ok = caps
            .present_modes
            .iter()
            .any(|m| *m == wgpu::PresentMode::Immediate);
        if !self.settings.vsync && !self.immediate_ok {
            println!("VSync: Immediate unavailable; forcing vsync on");
            self.settings.vsync = true;
        }
        let present_mode = self.present_mode_for_settings();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        println!(
            "GPU: {}  backend={:?}  driver={} {}",
            info.name, info.backend, info.driver, info.driver_info
        );
        println!(
            "  present_mode={present_mode:?}  vsync={}  available={:?}",
            self.settings.vsync, caps.present_modes
        );
        println!(
            "  surface={}x{}  format={format:?}  alpha={:?}",
            config.width, config.height, config.alpha_mode
        );

        let mesh = GpuMesh::from_graybox(&device, self.level.mesh());
        let atlas = self.level.materials();
        let lightmaps = self.level.lightmaps();
        let sky = self.level.skybox();
        let renderer = Renderer::new(device, queue, config, mesh, &atlas, &lightmaps, &sky);
        println!(
            "  mesh tris={}  lightmap={}x{}",
            renderer.mesh.index_count / 3,
            lightmaps.width,
            lightmaps.height
        );

        self.window = Some(window);
        self.surface = Some(surface);
        self.renderer = Some(renderer);
        self.last_frame = Instant::now();
    }

    fn set_capture(&mut self, capture: bool) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if capture {
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.mouse_captured = capture;
    }

    fn build_cmd(&self) -> UserCmd {
        let mut forward = 0.0;
        let mut side = 0.0;
        if self.keys.contains(&KeyCode::KeyW) {
            forward += 1.0;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            forward -= 1.0;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            side += 1.0;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            side -= 1.0;
        }
        UserCmd {
            viewangles: self.player.viewangles,
            forward_move: forward,
            side_move: side,
            jump: self.keys.contains(&KeyCode::Space),
            duck: self.keys.contains(&KeyCode::ControlLeft)
                || self.keys.contains(&KeyCode::ControlRight),
        }
    }

    fn simulate(&mut self, dt_real: f32) {
        if self.menu_open {
            return;
        }
        let tick_dt = self.vars.tick_interval;
        self.accumulator += dt_real;
        if self.accumulator > 0.25 {
            self.accumulator = 0.25;
        }
        while self.accumulator >= tick_dt {
            self.prev_origin = self.player.origin;
            let prev_vel = self.player.velocity;
            let cmd = self.build_cmd();
            let wishing = cmd.forward_move.abs() + cmd.side_move.abs() > 0.0;
            // Field triggers feed basevelocity / gravity_scale before physics.
            self.player.basevelocity = self.level.touch_push(self.player.origin);
            self.player.gravity_scale = self.level.touch_gravity(self.player.origin);
            self.player = tick(self.level.world(), &self.player, &cmd, &self.vars);

            let mut soft_respawned = false;
            if let Some((dest, angles)) = self.level.touch_teleport(self.player.origin) {
                self.player.origin = dest;
                self.player.viewangles = angles;
                soft_respawned = true;
            }
            if self.player.origin.z < self.level.kill_z() {
                let (spawn_origin, spawn_angles) = if self.run_timer.track_type == TrackType::Staged
                {
                    if let Some(zones) = self.zones.as_ref() {
                        stage_respawn_pose(
                            zones,
                            &self.level,
                            self.run_timer.current_stage,
                            self.player.viewangles,
                        )
                    } else {
                        self.level.spawn()
                    }
                } else {
                    self.level.spawn()
                };
                self.player.origin = spawn_origin;
                self.player.viewangles = spawn_angles;
                self.player.velocity = Vec3::ZERO;
                self.player.basevelocity = Vec3::ZERO;
                self.player.gravity_scale = 1.0;
                self.player.grounded = true;
                soft_respawned = true;
            }

            if soft_respawned {
                self.run_timer.notify_soft_respawn();
                if let Some(zones) = self.zones.as_ref() {
                    self.run_timer
                        .sync_stage_after_respawn(zones, &self.player);
                }
            }

            // Audio observation. Surfing is airborne by definition, so ramp
            // contact can't come from `grounded` — `is_on_surf_ramp` is the same
            // probe the ghost trail already uses, so the two agree.
            if self.audio.is_some() && self.settings.audio {
                let hull = self.player.hull();
                let on_ramp =
                    is_on_surf_ramp(self.level.world(), self.player.origin, &hull);
                self.audio_on_ramp = on_ramp;
                let obs = Observation {
                    dt: tick_dt,
                    speed: self.player.velocity.length_2d(),
                    on_ramp,
                    grounded: self.player.grounded,
                    wiped: soft_respawned,
                };
                let emitted = self.audio_detect.observe(obs);
                if let Some(audio) = self.audio.as_ref() {
                    for ev in emitted.iter() {
                        audio.push(ev);
                    }
                }
            }
            if let Some(zones) = self.zones.as_ref() {
                let phase_before = self.run_timer.phase;
                let was_finished = self.run_timer.is_finished();
                let pb_splits = self.pb_splits.clone();
                self.run_timer
                    .tick(zones, &mut self.player, tick_dt, &pb_splits);
                if let Some(ev) = self.run_timer.take_split_event() {
                    let staged = self.run_timer.track_type == TrackType::Staged;
                    let line = format_split_line(ev, staged);
                    println!("{line}");
                    self.split_flash_line = Some(line);
                    self.split_flash_left = SPLIT_FLASH_SECS;
                }
                self.record_replay_tick(phase_before, &cmd);
                if self.run_timer.is_finished() && !was_finished && !self.finish_recorded {
                    self.on_finish();
                }
                if self.run_timer.phase == TimerPhase::Running {
                    if let Some(pb) = self.pb_time {
                        self.pb_delta = Some(self.run_timer.time_secs - pb);
                    }
                }
            }

            let sample = air_strafe_sync(
                prev_vel,
                self.player.velocity,
                wishing,
                !self.player.grounded,
            );
            if sample > self.sync_display {
                self.sync_display = self.sync_display * 0.5 + sample * 0.5;
            } else {
                self.sync_display = self.sync_display * 0.85 + sample * 0.15;
            }
            self.accumulator -= tick_dt;
        }
        self.alpha = self.accumulator / tick_dt;

        // One parameter push per frame. Everything is smoothed per sample on
        // the audio thread, so this rate only has to beat the smoothing.
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams {
                speed: self.player.velocity.length_2d(),
                sync: self.sync_display,
                contact: if self.audio_on_ramp { 1.0 } else { 0.0 },
            });
        }
    }

    fn hud_timer_phase(&self) -> HudTimerPhase {
        match self.run_timer.phase {
            TimerPhase::Idle => HudTimerPhase::Idle,
            TimerPhase::Armed => HudTimerPhase::Armed,
            TimerPhase::Running => HudTimerPhase::Running,
            TimerPhase::Finished => HudTimerPhase::Finished,
        }
    }

    fn render_frame(&mut self) {
        if self.surface.is_none() || self.renderer.is_none() || self.window.is_none() {
            return;
        }

        let origin = self.prev_origin.lerp(self.player.origin, self.alpha);
        let eye = origin + Vec3::new(0.0, 0.0, self.player.hull().eye_height);
        let speed = self.player.velocity.length_2d();
        let show_keys = if self.settings.show_keys && !self.menu_open {
            Some(ShowKeysState {
                forward: self.keys.contains(&KeyCode::KeyW),
                back: self.keys.contains(&KeyCode::KeyS),
                left: self.keys.contains(&KeyCode::KeyA),
                right: self.keys.contains(&KeyCode::KeyD),
                jump: self.keys.contains(&KeyCode::Space),
            })
        } else {
            None
        };
        let menu = if self.menu_open {
            Some(self.build_menu_hud())
        } else {
            None
        };
        let timer_phase = self.hud_timer_phase();
        let perf_line = self.frame_stats.line();
        let racing = self.run_timer.phase == TimerPhase::Running
            && self
                .pb_ghost
                .as_ref()
                .map(|g| g.active)
                .unwrap_or(false);
        let (ghost_time_delta, ghost_speed_delta, ghost_pose, trail_pts) = if racing {
            let g = self.pb_ghost.as_ref().unwrap();
            let td = Some(self.run_timer.time_secs - g.current_time());
            let sd = g.current_speed_2d().map(|gs| speed - gs);
            let pose = g.sample(self.alpha).map(|(o, _, ducked)| GhostPose {
                origin: o,
                ducked,
            });
            let trail = if self.settings.ghost_trail {
                g.trail_to_current()
                    .into_iter()
                    .map(|(origin, on_ramp)| TrailPoint { origin, on_ramp })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (td, sd, pose, trail)
        } else {
            (None, None, None, Vec::new())
        };
        let split_line = if self.split_flash_left > 0.0 {
            self.split_flash_line.clone()
        } else {
            None
        };
        let stage_line = self
            .zones
            .as_ref()
            .and_then(|z| self.run_timer.stage_hud_label(z));
        let hud = HudState {
            speed,
            sync: self.sync_display,
            grounded: self.player.grounded,
            speed_scale: 3500.0,
            time_secs: self.run_timer.display_time(),
            pb_time_secs: self.pb_time,
            pb_delta_secs: self.pb_delta,
            timer_phase,
            show_sync_bar: self.settings.show_sync_bar,
            show_keys,
            pb_flash: self.pb_flash_left > 0.0,
            split_line,
            stage_line,
            ghost_time_delta,
            ghost_speed_delta,
            menu,
            perf_line,
        };
        let viewangles = self.player.viewangles;
        let title_base = self.title_base.clone();
        let menu_open = self.menu_open;
        let grounded = self.player.grounded;
        let time_label = self.run_timer.display_time().map(format_time);
        let aa = self.vars.airaccelerate;
        let sens = self.settings.mouse_sens;

        let surface = self.surface.as_ref().unwrap();
        let renderer = self.renderer.as_mut().unwrap();
        let window = self.window.as_ref().unwrap();

        let aspect = renderer.config.width as f32 / renderer.config.height.max(1) as f32;
        let camera = Camera::new(eye, viewangles, aspect);
        let view = ViewParams {
            exposure: Settings::clamp_brightness(self.settings.brightness),
            shadow_lift: Settings::clamp_shadow_lift(self.settings.shadow_lift),
            slope_tint: if self.settings.slope_tint { 1.0 } else { 0.0 },
            edge_highlight: if self.settings.edge_highlight {
                1.0
            } else {
                0.0
            },
        };

        let trail_ref = if trail_pts.len() >= 2 {
            Some(trail_pts.as_slice())
        } else {
            None
        };
        match renderer.render(surface, &camera, hud, ghost_pose, trail_ref, view) {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = window.inner_size();
                renderer.resize(size.width, size.height);
                surface.configure(&renderer.device, &renderer.config);
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                eprintln!("wgpu OOM");
            }
            Err(e) => eprintln!("render error: {e:?}"),
        }

        self.hud_timer += 1.0;
        if self.hud_timer > 10.0 {
            self.hud_timer = 0.0;
            let g = if grounded { "G" } else { "A" };
            let t = time_label.unwrap_or_else(|| "-".into());
            let pause = if menu_open { " PAUSED" } else { "" };
            window.set_title(&format!(
                "{title_base}{pause}  |  {speed:7.1} u/s  t {t}  [{g}]  aa {aa:.0}  sens {sens:.1}",
            ));
        }
    }

    /// Capture / discard frames based on timer phase transitions.
    fn record_replay_tick(&mut self, phase_before: TimerPhase, cmd: &UserCmd) {
        let phase = self.run_timer.phase;
        match phase {
            TimerPhase::Running => {
                if phase_before != TimerPhase::Running {
                    self.replay_rec.begin();
                    if let Some(g) = self.pb_ghost.as_mut() {
                        g.begin();
                    }
                } else if let Some(g) = self.pb_ghost.as_mut() {
                    g.advance();
                }
                self.replay_rec.push(cmd, &self.player);
            }
            TimerPhase::Finished => {
                if phase_before == TimerPhase::Running {
                    self.replay_rec.push(cmd, &self.player);
                }
            }
            TimerPhase::Armed | TimerPhase::Idle => {
                if matches!(phase_before, TimerPhase::Running | TimerPhase::Finished) {
                    if self.replay_rec.is_active() {
                        self.replay_rec.clear();
                    }
                    if let Some(g) = self.pb_ghost.as_mut() {
                        g.stop();
                    }
                }
            }
        }
    }

    fn on_finish(&mut self) {
        self.finish_recorded = true;
        let time = self.run_timer.time_secs;
        let splits = self.run_timer.splits.clone();
        if let Some(pb) = self.pb_time {
            self.pb_delta = Some(time - pb);
        } else {
            self.pb_delta = Some(0.0);
        }
        let Some(name) = self.level.map_name().map(|s| s.to_string()) else {
            self.replay_rec.clear();
            return;
        };

        let saved_path = self
            .replay_rec
            .finish(&name, time, &self.vars, &splits)
            .and_then(|replay| {
                let path = replay::new_run_replay_path(&name, time);
                match replay.save(&path) {
                    Ok(()) => {
                        println!(
                            "Replay saved {} ({} splits)",
                            path.display(),
                            splits.len()
                        );
                        Some(path)
                    }
                    Err(e) => {
                        eprintln!("Replay save failed: {e}");
                        None
                    }
                }
            });

        if let Some(store) = self.pb_store.as_ref() {
            let path_str = saved_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            match store.record_finish(&name, time, path_str.as_deref()) {
                Ok(Some(new_pb)) => {
                    self.pb_time = Some(new_pb);
                    self.pb_delta = Some(0.0);
                    self.pb_flash_left = PB_FLASH_SECS;
                    if let Some(src) = saved_path.as_ref() {
                        let pb_path = replay::pb_replay_path(&name);
                        if let Err(e) = std::fs::copy(src, &pb_path) {
                            eprintln!("PB replay copy failed: {e}");
                        } else {
                            println!("PB replay {}", pb_path.display());
                        }
                    }
                    self.pb_splits = splits;
                    if self.settings.ghost == GHOST_PB {
                        self.load_ghost();
                    }
                    self.refresh_ghost_options();
                    println!("PB! {} — {}", name, format_time(new_pb));
                }
                Ok(None) => {
                    println!(
                        "Finished {} in {} (PB {})",
                        name,
                        format_time(time),
                        self.pb_time
                            .map(format_time)
                            .unwrap_or_else(|| "-".into())
                    );
                }
                Err(e) => eprintln!("PB write failed: {e}"),
            }
        }
        self.refresh_recent_footer();
    }

    fn handle_menu_key(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) {
        match code {
            KeyCode::Escape => {
                // Close menu without recapture; Esc again (uncaptured) quits.
                self.close_menu(false);
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if self.menu_selected == MENU_QUIT {
                    self.persist_settings();
                    event_loop.exit();
                } else {
                    self.close_menu(true);
                }
            }
            KeyCode::ArrowUp => {
                if self.menu_selected == 0 {
                    self.menu_selected = MENU_ITEM_COUNT - 1;
                } else {
                    self.menu_selected -= 1;
                }
            }
            KeyCode::ArrowDown => {
                self.menu_selected = (self.menu_selected + 1) % MENU_ITEM_COUNT;
            }
            KeyCode::ArrowLeft | KeyCode::KeyA => self.menu_adjust(-1),
            KeyCode::ArrowRight | KeyCode::KeyD => self.menu_adjust(1),
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.init_gpu(event_loop);
        }
        event_loop.set_control_flow(ControlFlow::Poll);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let (Some(renderer), Some(surface)) =
                    (self.renderer.as_mut(), self.surface.as_ref())
                {
                    renderer.resize(size.width, size.height);
                    surface.configure(&renderer.device, &renderer.config);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } => match state {
                ElementState::Pressed => {
                    if self.menu_open {
                        if !repeat {
                            self.handle_menu_key(code, event_loop);
                        } else if matches!(
                            code,
                            KeyCode::ArrowLeft
                                | KeyCode::ArrowRight
                                | KeyCode::KeyA
                                | KeyCode::KeyD
                        ) {
                            self.handle_menu_key(code, event_loop);
                        }
                        return;
                    }
                    self.keys.insert(code);
                    match code {
                        KeyCode::Escape => {
                            if self.mouse_captured {
                                self.open_menu();
                            } else {
                                // Uncaptured + Esc → quit (second Esc after leaving menu).
                                event_loop.exit();
                            }
                        }
                        KeyCode::KeyR => self.reset(),
                        KeyCode::KeyT => self.reset_stage(),
                        KeyCode::BracketLeft => {
                            self.settings.mouse_sens =
                                Settings::clamp_sens(self.settings.mouse_sens / 1.25);
                            self.persist_settings();
                        }
                        KeyCode::BracketRight => {
                            self.settings.mouse_sens =
                                Settings::clamp_sens(self.settings.mouse_sens * 1.25);
                            self.persist_settings();
                        }
                        KeyCode::Minus => {
                            self.vars.airaccelerate =
                                (self.vars.airaccelerate / 1.5).max(1.0);
                        }
                        KeyCode::Equal => {
                            self.vars.airaccelerate =
                                (self.vars.airaccelerate * 1.5).min(1000.0);
                        }
                        _ => {}
                    }
                }
                ElementState::Released => {
                    self.keys.remove(&code);
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if self.menu_open {
                    self.close_menu(true);
                } else if !self.mouse_captured {
                    self.set_capture(true);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;
                if self.pb_flash_left > 0.0 {
                    self.pb_flash_left = (self.pb_flash_left - dt).max(0.0);
                }
                if self.split_flash_left > 0.0 {
                    self.split_flash_left = (self.split_flash_left - dt).max(0.0);
                }
                let t0 = Instant::now();
                self.simulate(dt);
                let sim_ms = t0.elapsed().as_secs_f32() * 1000.0;
                let t1 = Instant::now();
                self.render_frame();
                let render_ms = t1.elapsed().as_secs_f32() * 1000.0;
                let (w, h) = self
                    .renderer
                    .as_ref()
                    .map(|r| (r.config.width, r.config.height))
                    .unwrap_or((0, 0));
                self.frame_stats.push(dt, sim_ms, render_ms, w, h);
                if self.perf_started.is_none() {
                    self.perf_started = Some(Instant::now());
                }
                if let Some(limit) = self.perf_secs {
                    if let Some(start) = self.perf_started {
                        if start.elapsed().as_secs_f32() >= limit {
                            self.perf_secs = None;
                            self.frame_stats.refresh_display();
                            if let Some(line) = self.frame_stats.line() {
                                println!(
                                    "perf-final: {line}  frames={}",
                                    self.frame_stats.total_frames
                                );
                            } else {
                                println!(
                                    "perf-final: (no samples)  frames={}",
                                    self.frame_stats.total_frames
                                );
                            }
                            event_loop.exit();
                            return;
                        }
                    }
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if !self.mouse_captured || self.menu_open {
                return;
            }
            let yaw_scale = self.settings.mouse_sens * MOUSE_YAW_SCALE;
            let pitch_scale = self.settings.mouse_sens * MOUSE_PITCH_SCALE;
            self.player.viewangles.yaw -= dx as f32 * yaw_scale;
            self.player.viewangles.pitch += dy as f32 * pitch_scale;
            self.player.viewangles.pitch = self.player.viewangles.pitch.clamp(-89.0, 89.0);
            if self.player.viewangles.yaw > 180.0 {
                self.player.viewangles.yaw -= 360.0;
            }
            if self.player.viewangles.yaw < -180.0 {
                self.player.viewangles.yaw += 360.0;
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }
}

fn audio_levels(settings: &Settings) -> AudioLevels {
    AudioLevels {
        master: settings.audio_volume,
        core: settings.audio_core,
        air: settings.audio_air,
        sub: settings.audio_sub,
    }
}

/// Prefer a map teleport destination that sits inside the stage zone (real
/// platform), falling back to the zone-box floor center.
fn stage_respawn_pose(
    zones: &MapZones,
    level: &Level,
    stage: usize,
    fallback_angles: Angle,
) -> (Vec3, Angle) {
    let hull = Hull::css_stand();
    let zone = zones.main.stage_zone(stage);
    if let Level::Map(m) = level {
        for tp in &m.teleports {
            if zone.contains_player(tp.dest_origin, hull) {
                return (tp.dest_origin, tp.dest_angles);
            }
        }
    }
    (zones.main.stage_spawn(stage), fallback_angles)
}

fn load_zones_for(map_path: &std::path::Path) -> Option<MapZones> {
    let zpath = zones::zones_path_for_map(map_path);
    match zones::load_zones_file(&zpath) {
        Ok(z) => {
            println!(
                "  zones={}  start_cap={:.0}  start_on_jump={}  cps={}",
                zpath.display(),
                z.main.limit_start_ground_speed,
                z.main.start_on_jump,
                z.main.checkpoints.len()
            );
            Some(z)
        }
        Err(e) => {
            eprintln!("  zones unavailable ({}): {e}", zpath.display());
            None
        }
    }
}

fn parse_args() -> LaunchOpts {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut window_w = 1280u32;
    let mut window_h = 800u32;
    let mut perf_secs: Option<f32> = None;
    let mut vsync: Option<bool> = None;
    let mut graybox = false;
    let mut map_path: Option<PathBuf> = None;
    let mut ghost_path: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--graybox" => {
                graybox = true;
                i += 1;
            }
            "--vsync" => {
                vsync = Some(true);
                i += 1;
            }
            "--no-vsync" => {
                vsync = Some(false);
                i += 1;
            }
            "--ghost" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--ghost needs a .osxr path");
                    std::process::exit(2);
                };
                ghost_path = Some(PathBuf::from(v));
                i += 2;
            }
            "--perf-secs" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--perf-secs needs a number");
                    std::process::exit(2);
                };
                perf_secs = Some(v.parse::<f32>().unwrap_or_else(|_| {
                    eprintln!("invalid --perf-secs {v}");
                    std::process::exit(2);
                }));
                i += 2;
            }
            "--size" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--size needs WxH (e.g. 2560x1440)");
                    std::process::exit(2);
                };
                let (w, h) = parse_size(v).unwrap_or_else(|| {
                    eprintln!("invalid --size {v} (want WxH)");
                    std::process::exit(2);
                });
                window_w = w;
                window_h = h;
                i += 2;
            }
            s if s.starts_with('-') => {
                eprintln!("unknown flag {s}");
                std::process::exit(2);
            }
            s => {
                map_path = Some(PathBuf::from(s));
                i += 1;
            }
        }
    }

    let (level, zones) = if graybox {
        (Level::Graybox(graybox::surf_ramp_arena()), None)
    } else if let Some(path) = map_path {
        println!("Loading map {} …", path.display());
        let map = LoadedMap::load_path(&path).unwrap_or_else(|e| {
            eprintln!("failed to load {}: {e}", path.display());
            std::process::exit(1);
        });
        print_map_stats(&map);
        let zones = load_zones_for(&path);
        (Level::Map(map), zones)
    } else {
        let summit = PathBuf::from("assets/maps/surf_summit.bsp");
        if summit.is_file() {
            println!("Loading default map {} …", summit.display());
            match LoadedMap::load_path(&summit) {
                Ok(map) => {
                    print_map_stats(&map);
                    let zones = load_zones_for(&summit);
                    (Level::Map(map), zones)
                }
                Err(e) => {
                    eprintln!("summit load failed ({e}); falling back to graybox");
                    (Level::Graybox(graybox::surf_ramp_arena()), None)
                }
            }
        } else {
            (Level::Graybox(graybox::surf_ramp_arena()), None)
        }
    };

    LaunchOpts {
        level,
        zones,
        ghost_path,
        window_w,
        window_h,
        perf_secs,
        vsync,
    }
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn print_map_stats(map: &LoadedMap) {
    println!(
        "  brushes={}  tris={}  teleports={}  spawn=({:.0},{:.0},{:.0})",
        map.world.brushes.len(),
        map.mesh.tris.len(),
        map.teleports.len(),
        map.spawn_origin.x,
        map.spawn_origin.y,
        map.spawn_origin.z,
    );
}

fn main() {
    let opts = parse_args();
    let title = opts.level.title();
    println!("{title}");
    println!(
        "Click to capture. WASD, Space, R reset, T stage, Esc menu, [ ] sens, - = airaccel."
    );
    println!(
        "Movevars: aa={} accel={} friction={} tick={:.0}Hz autobhop={} (Momentum surf defaults).",
        MoveVars::momentum_surf().airaccelerate,
        MoveVars::momentum_surf().accelerate,
        MoveVars::momentum_surf().friction,
        1.0 / MoveVars::momentum_surf().tick_interval,
        MoveVars::momentum_surf().autobhop,
    );
    if opts.zones.is_some() {
        println!("Timer: leave start zone to begin; touch end to finish. R clears run.");
    }
    println!(
        "macOS: disable pointer acceleration for fair mouse feel \
         (System Settings → Mouse → Pointer acceleration)."
    );
    println!(
        "Window {}x{}{}",
        opts.window_w,
        opts.window_h,
        opts.perf_secs
            .map(|s| format!("  perf-sample {s}s then exit"))
            .unwrap_or_default()
    );

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(opts);
    event_loop.run_app(&mut app).expect("run");
}
