//! mx-surf app: graybox (M0), real BSP (M1), timer/zones (M2), menus.
//!
//! Usage:
//!   cargo run -p surf-app --release
//!   cargo run -p surf-app --release -- assets/maps/surf_summit.bsp
//!   cargo run -p surf-app --release -- --graybox
//!   cargo run -p surf-app --release -- --ghost path/to/run.osxr
//!   cargo run -p surf-app --release -- --perf-secs 8 --size 2560x1440
//!   cargo run -p surf-app --release -- --no-vsync
//!
//! Controls: WASD move, mouse look, Space jump (autobhop), Ctrl duck, R full
//! reset, T stage reset (staged maps), Esc pause menu, [ ] sens, - = airaccel.
//!
//! Practice locs: Mouse2 saveloc (pose + velocity + clock), Mouse1 loadloc
//! (selected loc, newest by default). A loaded run keeps timing but is marked
//! PRACTICE — no PB, no replay. Loc binds are inert while a menu is open or the
//! mouse is uncaptured. P toggles **practice mode** (off at launch): loadloc
//! only works while it is on, so a stray Mouse1 can't yank you out of a real
//! run. Saveloc always works.
//!
//! Menus: one page model everywhere (`surf_render::MenuPage`). The shell is
//! Main menu → Play / Leaderboard / Settings, and Esc in a map opens a pause
//! page with **sections** (Settings · Locs · Times) plus a permanent action bar
//! — Resume, Restart, Maps, Main menu, Quit. Every page is fully mouse-driven:
//! click a row or tab, drag a slider, wheel to adjust the hovered setting or
//! step the list, right-click to go back (or to delete a loc). Tab moves
//! keyboard focus between the list and the action bar; ↑↓ / ←→ / Enter drive
//! whichever has it.
//!
//! Perf line logs to stdout once per second; `--perf-secs N` exits after N.
//!
//! macOS note: NSEvent mouse deltas are OS-accelerated. For fair feel, disable
//! pointer acceleration (System Settings → Mouse → Pointer acceleration off),
//! or: `defaults write -g com.apple.mouse.scaling -integer -1`

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use surf_app::binds::{key_label, turn_delta, Bind, Binds};
use surf_app::leaderboard::{self, MapStanding};
use surf_app::locs::{self, Loc, GRAYBOX_MAP};
use surf_app::menu::{
    slider_range, ButtonAction, Focus, Nav, PageId, PauseTab, RowAction, Setting, SettingsEntry,
    LOC_ROW_PRACTICE, SETTINGS_PAGE,
};
use surf_app::pb::PbStore;
use surf_app::replay::{self, derive_splits, GhostOption, GhostPlayback, Replay};
use surf_app::session::{load_level, Level, Session};
use surf_app::settings::{Settings, GHOST_AUTO, GHOST_OFF, GHOST_PB};
use surf_app::timer::{format_split_line, format_time, TimerPhase};
use surf_app::zones::{MapZones, TrackType};
use surf_audio::{
    AudioEngine, AudioEvent, Levels as AudioLevels, Observation, Params as AudioParams, WipeStyle,
};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{Hull, MoveVars, PlayerState, UserCmd};
use surf_core::{air_strafe_sync, is_on_surf_ramp, tick};
use surf_render::{
    Camera, GhostPose, GpuMesh, HudState, HudTimerPhase, MenuPage, MenuPanel, MenuRow, PageLayout,
    Renderer, RowKind, RowTone, ShowKeysState, TrailPoint, ViewParams,
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
    /// Map to open straight into. `None` = start on the menu.
    map_path: Option<PathBuf>,
    /// `--graybox`: skip the menu into the M0 arena.
    graybox: bool,
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

/// One row of the map picker.
struct MapEntry {
    label: String,
    /// Map name as stored (`surf_summit`); the graybox arena uses its own key.
    name: String,
    /// `None` = the generated graybox arena.
    path: Option<PathBuf>,
    pb: Option<f32>,
    /// Imported KSF world record, when we have ghosts for the map.
    wr: Option<f32>,
}

/// What the app is showing. A [`Session`] always exists underneath — the menus
/// draw over a live world — so nothing here carries map state.
enum Mode {
    /// Title screen: Play / Settings / Quit.
    MainMenu,
    /// Map list.
    MapPicker,
    /// A map is loading on a worker thread. Loads run 1.3-7.5s on this corpus,
    /// which is far too long to block the event loop.
    Loading {
        rx: std::sync::mpsc::Receiver<Result<(Level, Option<MapZones>), String>>,
        name: String,
        started: Instant,
    },
    /// Leaderboard: the map list, or one map's records.
    Leaderboard { map: Option<String> },
    /// Settings, reached from the main menu (same list as the pause section).
    Settings,
    /// In the world. `menu_open` layers the Esc pause page over this.
    Playing,
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    renderer: Option<Renderer>,
    /// The loaded map and everything belonging to it.
    session: Session,
    mode: Mode,
    /// Map picker rows. PBs are cached here, not queried per frame.
    map_list: Vec<MapEntry>,
    /// Per-map PB / WR summary for the leaderboard, refreshed when it opens.
    standings: Vec<MapStanding>,
    /// Cursor position on each page.
    nav: Nav,
    /// Error from the last failed load, shown on the picker.
    load_error: Option<String>,
    keys: HashSet<KeyCode>,
    mouse_captured: bool,
    menu_open: bool,
    /// Section of the pause menu that is showing.
    pause_tab: PauseTab,
    /// Whether the keyboard drives the row list or the action bar.
    focus: Focus,
    /// Index in the action bar while it has focus.
    button_selected: usize,
    /// Slider row being dragged with the mouse held down, if any.
    dragging: Option<usize>,
    /// A world has been entered at least once, so "Resume" is meaningful.
    entered_world: bool,
    settings: Settings,
    /// Decoded form of `settings.binds`; the map is rewritten from this.
    binds: Binds,
    /// A KEYBINDS row is waiting for its next key press.
    rebinding: Option<Bind>,
    last_frame: Instant,
    hud_timer: f32,
    pb_store: Option<PbStore>,
    frame_stats: FrameStats,
    window_w: u32,
    window_h: u32,
    perf_secs: Option<f32>,
    /// Set when the first frame renders (for `--perf-secs`).
    perf_started: Option<Instant>,
    /// Surface supports `PresentMode::Immediate` (no VSync).
    immediate_ok: bool,
    /// `None` when no output device was available — the game runs on silently.
    audio: Option<AudioEngine>,
    /// Cursor position in physical pixels while a menu is up.
    cursor_px: (f32, f32),
    /// Wall clock since launch — drives the menu backdrop and the busy bar.
    start_time: Instant,
}

impl App {
    fn new(opts: LaunchOpts) -> Self {
        let LaunchOpts {
            map_path,
            graybox,
            window_w,
            window_h,
            perf_secs,
            vsync,
            ghost_path,
        } = opts;

        let pb_store = match PbStore::open_default() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("PB store unavailable ({e}); times won't be saved");
                None
            }
        };
        let mut settings = Settings::load_default();
        let binds = Binds::from_map(&settings.binds);
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

        // A session always exists so the menus have a world to draw over and no
        // caller has to unwrap. The graybox arena is generated, not loaded, so
        // this costs nothing at startup.
        let session = Session::graybox(pb_store.as_ref(), None);

        let mut app = Self {
            window: None,
            surface: None,
            renderer: None,
            session,
            mode: Mode::MainMenu,
            map_list: discover_maps(),
            standings: Vec::new(),
            nav: Nav::default(),
            load_error: None,
            keys: HashSet::new(),
            mouse_captured: false,
            menu_open: false,
            pause_tab: PauseTab::Settings,
            focus: Focus::Rows,
            button_selected: 0,
            dragging: None,
            entered_world: false,
            settings,
            binds,
            rebinding: None,
            last_frame: Instant::now(),
            hud_timer: 0.0,
            pb_store,
            frame_stats: FrameStats::new(),
            window_w,
            window_h,
            perf_secs,
            perf_started: None,
            immediate_ok: false,
            audio,
            cursor_px: (0.0, 0.0),
            start_time: Instant::now(),
        };

        // `mx-surf <map>` and `--graybox` skip the shell, as they always have.
        if graybox {
            app.mode = Mode::Playing;
            app.entered_world = true;
            app.on_session_loaded();
        } else if let Some(path) = map_path {
            app.begin_load(path);
        }
        app
    }

    /// Kick a map load onto a worker thread. Loads run 1.3-7.5s on this corpus,
    /// so doing it inline would freeze the window (and beachball on macOS).
    fn begin_load(&mut self, path: PathBuf) {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("map")
            .to_string();
        if !path.is_file() {
            self.load_error = Some(fetch_hint(&path));
            eprintln!("{}", fetch_hint(&path));
            self.mode = Mode::MapPicker;
            return;
        }
        println!("Loading map {} …", path.display());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load_level(&path));
        });
        self.load_error = None;
        self.mode = Mode::Loading {
            rx,
            name,
            started: Instant::now(),
        };
    }

    /// Poll the loader. Returns once the session has been swapped in (or the
    /// load failed and we bounced back to the picker).
    fn poll_load(&mut self) {
        let Mode::Loading { rx, .. } = &self.mode else {
            return;
        };
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("loader thread died".to_string())
            }
        };
        let elapsed = match &self.mode {
            Mode::Loading { started, .. } => started.elapsed().as_secs_f32(),
            _ => 0.0,
        };
        match msg {
            Ok((level, zones)) => {
                println!("  loaded in {elapsed:.1}s");
                self.enter_session(Session::new(
                    level,
                    zones,
                    self.pb_store.as_ref(),
                    Some(self.session.vars.airaccelerate),
                ));
            }
            Err(e) => {
                eprintln!("load failed: {e}");
                self.load_error = Some(e);
                self.mode = Mode::MapPicker;
            }
        }
    }

    /// Swap in a freshly built session and rebuild everything downstream of it:
    /// GPU buffers, ghost catalogue, PB footer, window title, audio baseline.
    fn enter_session(&mut self, session: Session) {
        self.session = session;
        self.mode = Mode::Playing;
        self.menu_open = false;
        self.entered_world = true;
        self.pause_tab = PauseTab::Settings;
        self.nav.set(PageId::Locs, LOC_ROW_PRACTICE);
        self.nav.set(PageId::Times, 0);
        self.on_session_loaded();
        // Loading a map is a deliberate "play this now", so take the mouse
        // rather than making the player click the world first.
        self.set_capture(true);
    }

    /// Shared tail of "a new session is live". Also runs at startup so the two
    /// paths can't drift.
    fn on_session_loaded(&mut self) {
        if let Some(r) = self.renderer.as_mut() {
            r.load_level(
                self.session.level.mesh(),
                &self.session.level.materials(),
                &self.session.level.lightmaps(),
                &self.session.level.skybox(),
            );
            println!("  mesh tris={}", r.mesh.index_count / 3);
        }
        self.refresh_ghost_options();
        self.load_ghost();
        self.apply_audio_settings();
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
            audio.push(AudioEvent::Rearm);
        }
        if let Some(w) = self.window.as_ref() {
            w.set_title(&self.session.title_base);
        }
        if self.session.zones.is_some() {
            println!("Timer: leave start zone to begin; touch end to finish. R clears run.");
        }
    }

    /// Identity of the page currently on screen.
    fn page_id(&self) -> PageId {
        match &self.mode {
            Mode::MainMenu => PageId::Main,
            Mode::MapPicker => PageId::Picker,
            Mode::Settings => PageId::Settings,
            Mode::Leaderboard { map: None } => PageId::Board,
            Mode::Leaderboard { map: Some(_) } => PageId::Records,
            Mode::Loading { .. } => PageId::Loading,
            Mode::Playing => match self.pause_tab {
                PauseTab::Settings => PageId::Settings,
                PauseTab::Locs => PageId::Locs,
                PauseTab::Times => PageId::Times,
            },
        }
    }

    fn selected_row(&self) -> usize {
        self.nav.get(self.page_id())
    }

    fn set_selected_row(&mut self, row: usize) {
        let id = self.page_id();
        self.nav.set(id, row);
        // Highlighting a loc row *is* selecting it, so a later Mouse1 in game
        // loads the one you were last looking at.
        if id == PageId::Locs {
            if let Some(i) = locs::loc_index_for_row(self.session.locs.len(), row) {
                self.session.locs.set_selected(i);
            }
        }
    }

    /// Is a menu (shell page or pause overlay) on screen?
    fn page_open(&self) -> bool {
        !matches!(self.mode, Mode::Playing) || self.menu_open
    }

    // -- page content ------------------------------------------------------

    /// Rows of the active page, each paired with what activating it does. One
    /// builder so the drawn list and the action table cannot drift apart.
    fn page_entries(&self) -> Vec<(MenuRow, RowAction)> {
        match &self.mode {
            Mode::MainMenu => {
                let mut out = Vec::new();
                if self.entered_world {
                    out.push((
                        MenuRow::item("Resume", self.session.level.short_name())
                            .with_tone(RowTone::Accent),
                        RowAction::MainResume,
                    ));
                }
                out.push((MenuRow::item("Play", ""), RowAction::MainPlay));
                out.push((MenuRow::item("Leaderboard", ""), RowAction::MainLeaderboard));
                out.push((MenuRow::item("Settings", ""), RowAction::MainSettings));
                out.push((MenuRow::item("Quit", ""), RowAction::MainQuit));
                out
            }
            Mode::MapPicker => self
                .map_list
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let row = MenuRow::item(
                        m.label.clone(),
                        m.pb.map(format_time).unwrap_or_else(|| "—".into()),
                    )
                    .with_note(
                        m.wr.map(|t| format!("wr {}", format_time(t)))
                            .unwrap_or_default(),
                    )
                    .with_tone(if m.pb.is_some() {
                        RowTone::Normal
                    } else {
                        RowTone::Dim
                    });
                    (row, RowAction::PickMap(i))
                })
                .collect(),
            Mode::Settings => self.settings_entries(),
            Mode::Leaderboard { map: None } => self
                .standings
                .iter()
                .map(|st| {
                    let row = MenuRow::item(
                        st.label.clone(),
                        st.pb.map(format_time).unwrap_or_else(|| "—".into()),
                    )
                    .with_note(match st.wr.as_ref() {
                        Some(wr) => format!("wr {}", format_time(wr.time)),
                        None => String::new(),
                    })
                    .with_tone(match st.delta() {
                        // Beating an imported KSF record is a real event; say so.
                        Some(d) if d <= 0.0 => RowTone::Good,
                        Some(_) => RowTone::Normal,
                        None if st.pb.is_some() => RowTone::Normal,
                        None => RowTone::Dim,
                    });
                    (row, RowAction::OpenBoard(st.map.clone()))
                })
                .collect(),
            Mode::Leaderboard { map: Some(map) } => self.record_entries(map),
            Mode::Loading { .. } => Vec::new(),
            Mode::Playing => match self.pause_tab {
                PauseTab::Settings => self.settings_entries(),
                PauseTab::Locs => self.locs_entries(),
                PauseTab::Times => {
                    let map = self.session.level.store_key().to_string();
                    self.record_entries(&map)
                }
            },
        }
    }

    fn settings_entries(&self) -> Vec<(MenuRow, RowAction)> {
        SETTINGS_PAGE
            .iter()
            .map(|e| match e {
                SettingsEntry::Header(h) => (MenuRow::header(*h), RowAction::None),
                SettingsEntry::Set(s) => (self.setting_row(*s), RowAction::Adjust(*s)),
                SettingsEntry::Bind(b) => (self.bind_row(*b), RowAction::Rebind(*b)),
            })
            .collect()
    }

    fn bind_row(&self, b: Bind) -> MenuRow {
        if self.rebinding == Some(b) {
            return MenuRow::item(b.label(), "press a key").with_tone(RowTone::Warn);
        }
        MenuRow::item(b.label(), key_label(self.binds.key(b)).unwrap_or("?"))
    }

    fn setting_row(&self, s: Setting) -> MenuRow {
        let on = |b: bool| if b { "On" } else { "Off" };
        let label = self.setting_label(s);
        if let Some(range) = slider_range(s) {
            let v = self.setting_value(s);
            let text = match s {
                Setting::Airaccel => format!("{v:.0}"),
                Setting::Sens => format!("{v:.1}"),
                Setting::TurnSpeed => format!("{v:.0}°/s"),
                _ => format!("{v:.2}"),
            };
            return MenuRow::slider(label, text, range.frac_of(v));
        }
        match s {
            Setting::Vsync => {
                // "On*" = Immediate isn't offered by this surface, so the
                // toggle is inert rather than broken.
                let v = if self.settings.vsync {
                    "On"
                } else if self.immediate_ok {
                    "Off"
                } else {
                    "On*"
                };
                MenuRow::item(label, v)
            }
            Setting::ShowSync => MenuRow::item(label, on(self.settings.show_sync_bar)),
            Setting::ShowKeys => MenuRow::item(label, on(self.settings.show_keys)),
            Setting::Ghost => MenuRow::item(label, self.ghost_label()),
            Setting::GhostTrail => MenuRow::item(label, on(self.settings.ghost_trail)),
            Setting::Audio => MenuRow::item(
                label,
                if self.audio.is_some() {
                    on(self.settings.audio)
                } else {
                    "No device"
                },
            ),
            Setting::Wipe => MenuRow::item(
                label,
                match WipeStyle::from_str_or_default(&self.settings.wipe_style) {
                    WipeStyle::Rewind => "Rewind",
                    WipeStyle::Dissolve => "Dissolve",
                },
            ),
            _ => MenuRow::item(label, String::new()),
        }
    }

    fn setting_label(&self, s: Setting) -> &'static str {
        match s {
            Setting::Sens => "Sensitivity",
            Setting::Brightness => "Brightness",
            Setting::ShadowLift => "Shadow lift",
            Setting::Vsync => "VSync",
            Setting::ShowSync => "Show sync %",
            Setting::ShowKeys => "Show keys",
            Setting::Ghost => "Ghost",
            Setting::GhostTrail => "Ghost trail",
            Setting::Airaccel => "Airaccelerate",
            Setting::Audio => "Audio",
            Setting::AudioVolume => "Volume",
            Setting::AudioCore => "Core level",
            Setting::AudioAir => "Air level",
            Setting::AudioSub => "Sub level",
            Setting::Wipe => "Wipe sound",
            Setting::TurnSpeed => "Turn speed",
        }
    }

    fn locs_entries(&self) -> Vec<(MenuRow, RowAction)> {
        let n = self.session.locs.len();
        let mut out: Vec<(MenuRow, RowAction)> = Vec::with_capacity(n + 3);
        out.push((
            MenuRow::item(
                "Practice mode",
                if self.session.practice_mode {
                    "On"
                } else {
                    "Off"
                },
            )
            .with_tone(if self.session.practice_mode {
                RowTone::Warn
            } else {
                RowTone::Dim
            }),
            RowAction::LocPractice,
        ));
        for i in 0..n {
            let Some(loc) = self.session.locs.get(i) else {
                continue;
            };
            let active = self.session.locs.selected() == Some(i);
            out.push((
                MenuRow::item(
                    format!("{} #{}", if active { "▸" } else { " " }, i + 1),
                    format_time(loc.time_secs),
                )
                .with_note(format!("{:.0} u/s", loc.speed_2d())),
                RowAction::Loc(i),
            ));
        }
        out.push((
            MenuRow::item("Load selected", if n == 0 { "—" } else { "enter" }),
            RowAction::LocLoad,
        ));
        out.push((
            MenuRow::item("Clear all", if n == 0 { "—" } else { "x" }),
            RowAction::LocClear,
        ));
        out
    }

    /// Times for one map: the imported KSF records, then the player's own.
    fn record_entries(&self, map: &str) -> Vec<(MenuRow, RowAction)> {
        let mut out: Vec<(MenuRow, RowAction)> = Vec::new();
        let records = leaderboard::ksf_records(map);
        let pb = self
            .pb_store
            .as_ref()
            .and_then(|s| s.get(map).ok().flatten());

        out.push((MenuRow::header("WORLD RECORDS · KSF"), RowAction::None));
        if records.is_empty() {
            out.push((
                MenuRow::text("no imported records", "").with_tone(RowTone::Dim),
                RowAction::None,
            ));
        }
        for r in records.iter().take(15) {
            let beat = pb.is_some_and(|p| p <= r.time);
            out.push((
                MenuRow::text(format!("#{:<2} {}", r.rank, r.name), format_time(r.time))
                    .with_tone(if beat { RowTone::Good } else { RowTone::Normal }),
                RowAction::None,
            ));
        }

        out.push((MenuRow::header("YOUR TIMES"), RowAction::None));
        match pb {
            Some(t) => {
                let note = records
                    .first()
                    .map(|wr| format!("{:+.3} vs wr", t - wr.time))
                    .unwrap_or_default();
                out.push((
                    MenuRow::text("PB", format_time(t))
                        .with_note(note)
                        .with_tone(RowTone::Accent),
                    RowAction::None,
                ));
            }
            None => out.push((
                MenuRow::text("no finish yet", "").with_tone(RowTone::Dim),
                RowAction::None,
            )),
        }
        let recent = self
            .pb_store
            .as_ref()
            .and_then(|s| s.list_recent(map, 8).ok())
            .unwrap_or_default();
        for (i, r) in recent.iter().enumerate() {
            out.push((
                MenuRow::text(format!("run {}", i + 1), format_time(r.time_secs))
                    .with_note(if r.is_pb { "pb".into() } else { String::new() })
                    .with_tone(if r.is_pb { RowTone::Good } else { RowTone::Dim }),
                RowAction::None,
            ));
        }
        out
    }

    fn buttons(&self) -> Vec<ButtonAction> {
        match &self.mode {
            Mode::MainMenu | Mode::Loading { .. } => Vec::new(),
            Mode::MapPicker | Mode::Settings | Mode::Leaderboard { .. } => vec![ButtonAction::Back],
            Mode::Playing => vec![
                ButtonAction::Resume,
                ButtonAction::Restart,
                ButtonAction::Maps,
                ButtonAction::MainMenu,
                ButtonAction::Quit,
            ],
        }
    }

    /// Title and subtitle. There is deliberately no key-hint line (Max,
    /// 2026-09-03): how to drive a list is implicit in the design.
    fn page_title(&self) -> (String, String) {
        match &self.mode {
            Mode::MainMenu => (
                "MX-SURF".into(),
                "source-faithful surf · single player".into(),
            ),
            Mode::MapPicker => (
                "SELECT MAP".into(),
                format!("{} available", self.map_list.len()),
            ),
            Mode::Settings => (
                "SETTINGS".into(),
                if self.rebinding.is_some() {
                    "press a key · esc cancels".into()
                } else {
                    "saved on close".into()
                },
            ),
            Mode::Leaderboard { map: None } => (
                "LEADERBOARD".into(),
                "your PB against the imported KSF record".into(),
            ),
            Mode::Leaderboard { map: Some(m) } => (
                m.strip_prefix("surf_").unwrap_or(m).to_uppercase(),
                "ksf records · your runs".into(),
            ),
            Mode::Loading { name, started, .. } => (
                "LOADING".into(),
                format!("{name} · {:.1}s", started.elapsed().as_secs_f32()),
            ),
            Mode::Playing => (
                self.session.level.short_name().to_uppercase(),
                if self.rebinding.is_some() {
                    "press a key · esc cancels".into()
                } else {
                    match self.session.pb_time {
                        Some(t) => format!("paused · pb {}", format_time(t)),
                        None => "paused · no pb yet".into(),
                    }
                },
            ),
        }
    }

    /// Build the page the renderer draws. `None` while actually playing.
    fn build_page(&self) -> Option<MenuPage> {
        if !self.page_open() {
            return None;
        }
        let entries = self.page_entries();
        let rows: Vec<MenuRow> = entries.iter().map(|(r, _)| r.clone()).collect();
        let selected = self.selected_row().min(rows.len().saturating_sub(1));
        let (title, subtitle) = self.page_title();
        let playing = matches!(self.mode, Mode::Playing);
        let wide = matches!(self.mode, Mode::Leaderboard { .. })
            || (playing && self.pause_tab == PauseTab::Times);
        let buttons: Vec<String> = self
            .buttons()
            .iter()
            .map(|b| b.label().to_string())
            .collect();

        // The layout decides how many rows fit, so scroll has to be derived
        // from it — otherwise the window and the highlight disagree.
        let mut page = MenuPage {
            title,
            subtitle,
            tabs: if playing {
                PauseTab::ALL
                    .iter()
                    .map(|t| {
                        match t {
                            PauseTab::Settings => "SETTINGS",
                            PauseTab::Locs => "LOCS",
                            PauseTab::Times => "TIMES",
                        }
                        .to_string()
                    })
                    .collect()
            } else {
                Vec::new()
            },
            tab: self.pause_tab.index(),
            tab_hovered: None,
            panel: MenuPanel {
                rows,
                selected,
                hovered: None,
                scroll: 0,
                focused: self.focus == Focus::Rows,
            },
            buttons,
            button_selected: (self.focus == Focus::Buttons).then_some(self.button_selected),
            button_hovered: None,
            message: self.load_error.clone(),
            wide,
            backdrop: !playing,
            busy: matches!(self.mode, Mode::Loading { .. }),
            large: matches!(self.mode, Mode::MainMenu),
        };
        let layout = surf_render::layout_for(self.screen().0, self.screen().1, &page);
        page.panel.scroll = locs::scroll_window_start(
            page.panel.rows.len(),
            layout.panel.rows,
            page.panel.selected,
        );
        let (mx, my) = self.cursor_px;
        page.panel.hovered = layout
            .panel
            .row_at(mx, my)
            .map(|d| d + page.panel.scroll)
            .filter(|r| *r < page.panel.rows.len());
        page.tab_hovered = layout.tabs.iter().position(|r| r.contains(mx, my));
        page.button_hovered = layout.buttons.iter().position(|r| r.contains(mx, my));
        Some(page)
    }

    /// Physical pixel size of the drawing surface — the space menu geometry and
    /// `CursorMoved` both live in, so no scaling is needed between them.
    fn screen(&self) -> (f32, f32) {
        self.renderer
            .as_ref()
            .map(|r| (r.config.width as f32, r.config.height as f32))
            .unwrap_or((self.window_w as f32, self.window_h as f32))
    }

    fn page_layout(&self) -> Option<(MenuPage, PageLayout)> {
        let page = self.build_page()?;
        let (w, h) = self.screen();
        let layout = surf_render::layout_for(w, h, &page);
        Some((page, layout))
    }

    // -- page navigation ---------------------------------------------------

    /// Move the row cursor, skipping headers and read-only lines. Wraps.
    fn move_selection(&mut self, d: i32) {
        let entries = self.page_entries();
        let n = entries.len();
        if n == 0 {
            return;
        }
        let selectable = |i: usize| entries[i].0.kind.selectable();
        let mut row = self.selected_row().min(n - 1) as i32;
        for _ in 0..n {
            row = (row + d).rem_euclid(n as i32);
            if selectable(row as usize) {
                self.set_selected_row(row as usize);
                return;
            }
        }
        // Nothing selectable (a records page): leave the cursor alone.
    }

    /// Put the cursor on the first selectable row of a page we just opened.
    fn snap_selection_into_range(&mut self) {
        let entries = self.page_entries();
        if entries.is_empty() {
            self.set_selected_row(0);
            return;
        }
        let cur = self.selected_row();
        if cur < entries.len() && entries[cur].0.kind.selectable() {
            return;
        }
        let first = entries.iter().position(|(r, _)| r.kind.selectable());
        self.set_selected_row(first.unwrap_or(0));
    }

    fn action_at(&self, row: usize) -> Option<RowAction> {
        self.page_entries().get(row).map(|(_, a)| a.clone())
    }

    fn activate_row(&mut self, row: usize, event_loop: &ActiveEventLoop) {
        let Some(action) = self.action_at(row) else {
            return;
        };
        // Clicking anywhere else abandons a pending key capture.
        self.rebinding = None;
        match action {
            RowAction::None => {}
            // Toggles and cyclers flip on Enter/click; sliders need ←→, the
            // wheel, or a drag, so activating one does nothing rather than
            // jumping the value under the cursor.
            RowAction::Adjust(s) => {
                if slider_range(s).is_none() {
                    self.adjust_setting(s, 1);
                }
            }
            RowAction::Rebind(b) => self.rebinding = Some(b),
            RowAction::MainResume => self.resume_world(),
            RowAction::MainPlay => self.open_picker(),
            RowAction::MainLeaderboard => self.open_leaderboard(),
            RowAction::MainSettings => {
                self.mode = Mode::Settings;
                self.focus = Focus::Rows;
                self.snap_selection_into_range();
            }
            RowAction::MainQuit => {
                self.persist_settings();
                event_loop.exit();
            }
            RowAction::PickMap(i) => {
                let Some(entry) = self.map_list.get(i) else {
                    return;
                };
                match entry.path.clone() {
                    Some(p) => self.begin_load(p),
                    None => {
                        let s = Session::graybox(
                            self.pb_store.as_ref(),
                            Some(self.session.vars.airaccelerate),
                        );
                        self.enter_session(s);
                    }
                }
            }
            RowAction::OpenBoard(map) => {
                self.mode = Mode::Leaderboard { map: Some(map) };
                self.nav.set(PageId::Records, 0);
            }
            RowAction::LocPractice => {
                let on = !self.session.practice_mode;
                self.set_practice_mode(on);
            }
            RowAction::Loc(i) => {
                // Picking a loc here is deliberate, so it arms practice mode for
                // you rather than refusing — the guard exists for stray clicks.
                self.set_practice_mode(true);
                self.close_menu(true);
                self.load_loc(i);
            }
            RowAction::LocLoad => {
                if let Some(i) = self.session.locs.selected() {
                    self.set_practice_mode(true);
                    self.close_menu(true);
                    self.load_loc(i);
                }
            }
            RowAction::LocClear => {
                let n = self.session.locs.len();
                if n > 0 {
                    self.session.locs.clear();
                    self.persist_locs();
                    self.nav.set(PageId::Locs, LOC_ROW_PRACTICE);
                    println!("cleared {n} locs");
                }
            }
        }
    }

    /// ←→ / wheel on a row.
    fn adjust_row(&mut self, row: usize, dir: i32) {
        match self.action_at(row) {
            Some(RowAction::Adjust(s)) => self.adjust_setting(s, dir),
            Some(RowAction::LocPractice) => {
                let on = !self.session.practice_mode;
                self.set_practice_mode(on);
            }
            _ => {}
        }
    }

    fn activate_button(&mut self, i: usize, event_loop: &ActiveEventLoop) {
        let Some(b) = self.buttons().get(i).copied() else {
            return;
        };
        match b {
            ButtonAction::Resume => self.close_menu(true),
            ButtonAction::Restart => {
                self.close_menu(true);
                self.reset();
            }
            ButtonAction::Maps => self.open_picker(),
            ButtonAction::MainMenu => self.leave_to_main_menu(),
            ButtonAction::Quit => {
                self.persist_settings();
                event_loop.exit();
            }
            ButtonAction::Back => self.page_back(event_loop),
        }
    }

    /// Is backing out of this page a harmless click target? On the title
    /// screen there is nowhere to go back to, and a load must not be
    /// interrupted by a stray click off the panel or a right-click.
    fn back_is_click_safe(&self) -> bool {
        !matches!(self.mode, Mode::MainMenu | Mode::Loading { .. })
    }

    /// Esc / right-click / Back.
    ///
    /// Esc never quits (Max, 2026-09-02): backing out of nested menus with a
    /// run of Esc presses used to fall through the title screen and close the
    /// game. Quit is a deliberate row / button only.
    fn page_back(&mut self, _event_loop: &ActiveEventLoop) {
        if self.rebinding.take().is_some() {
            // Esc during a key capture only cancels the capture.
            return;
        }
        match &self.mode {
            Mode::MainMenu => {
                self.persist_settings();
            }
            Mode::MapPicker | Mode::Settings | Mode::Leaderboard { map: None } => {
                self.persist_settings();
                self.load_error = None;
                self.mode = Mode::MainMenu;
                self.focus = Focus::Rows;
                self.snap_selection_into_range();
            }
            Mode::Leaderboard { map: Some(_) } => {
                self.mode = Mode::Leaderboard { map: None };
                self.focus = Focus::Rows;
                self.snap_selection_into_range();
            }
            Mode::Loading { .. } => {}
            Mode::Playing => self.close_menu(false),
        }
    }

    fn open_picker(&mut self) {
        self.refresh_map_pbs();
        self.load_error = None;
        self.menu_open = false;
        self.set_capture(false);
        self.mode = Mode::MapPicker;
        self.focus = Focus::Rows;
        self.snap_selection_into_range();
    }

    fn open_leaderboard(&mut self) {
        self.refresh_map_pbs();
        let names: Vec<String> = self.map_list.iter().map(|m| m.name.clone()).collect();
        self.standings = leaderboard::standings(&names, self.pb_store.as_ref());
        self.mode = Mode::Leaderboard { map: None };
        self.focus = Focus::Rows;
        self.snap_selection_into_range();
    }

    /// Back into the world we already have loaded, from the main menu.
    fn resume_world(&mut self) {
        self.load_error = None;
        self.mode = Mode::Playing;
        self.menu_open = false;
        self.set_capture(true);
    }

    /// Leave the world for the title screen. The session stays loaded, so
    /// "Resume" on the main menu goes straight back without a reload.
    fn leave_to_main_menu(&mut self) {
        self.persist_settings();
        self.menu_open = false;
        self.set_capture(false);
        self.mode = Mode::MainMenu;
        self.focus = Focus::Rows;
        self.snap_selection_into_range();
    }

    /// Refresh cached PBs / records — cheap, but only worth doing on entering
    /// a page that shows them.
    fn refresh_map_pbs(&mut self) {
        for m in self.map_list.iter_mut() {
            if let Some(store) = self.pb_store.as_ref() {
                m.pb = store.get(&m.name).ok().flatten();
            }
            if m.wr.is_none() {
                m.wr = leaderboard::world_record(&m.name).map(|r| r.time);
            }
        }
    }

    fn refresh_ghost_options(&mut self) {
        let Some(name) = self.session.level.map_name().map(|s| s.to_string()) else {
            self.session.ghost_options = vec![GhostOption {
                id: GHOST_OFF.into(),
                label: "Off".into(),
                path: None,
            }];
            return;
        };
        self.session.ghost_options = replay::ghost_catalog(&name, &self.settings.ghost);
    }

    fn ghost_label(&self) -> String {
        self.session
            .ghost_options
            .iter()
            .find(|o| o.id == self.settings.ghost)
            .map(|o| o.label.clone())
            .unwrap_or_else(|| self.settings.ghost.clone())
    }

    fn cycle_ghost(&mut self, dir: i32) {
        self.refresh_ghost_options();
        if self.session.ghost_options.is_empty() {
            return;
        }
        let cur = self
            .session
            .ghost_options
            .iter()
            .position(|o| o.id == self.settings.ghost)
            .unwrap_or(0);
        let n = self.session.ghost_options.len() as i32;
        let next = (cur as i32 + dir).rem_euclid(n) as usize;
        self.settings.ghost = self.session.ghost_options[next].id.clone();
        self.load_ghost();
    }

    fn load_ghost(&mut self) {
        let Some(name) = self.session.level.map_name().map(|s| s.to_string()) else {
            self.session.pb_ghost = None;
            self.session.pb_splits.clear();
            return;
        };
        if self.settings.ghost == GHOST_OFF {
            self.session.pb_ghost = None;
            self.session.pb_splits.clear();
            println!("  ghost off");
            return;
        }
        let Some(path) = replay::resolve_ghost_path(&name, &self.settings.ghost) else {
            self.session.pb_ghost = None;
            self.session.pb_splits.clear();
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
                    if let Some(zones) = self.session.zones.as_ref() {
                        replay.header.splits = derive_splits(
                            &replay.frames,
                            &zones.main.checkpoints,
                            replay.header.tick_interval,
                            Hull::css_stand(),
                        );
                    }
                }
                self.session.pb_splits = replay.header.splits.clone();
                println!(
                    "  {} ghost loaded ({} frames, {} splits, style={}) from {}",
                    kind,
                    replay.header.frame_count,
                    self.session.pb_splits.len(),
                    replay.header.style,
                    path.display()
                );
                let mut ghost = GhostPlayback::from_replay(replay);
                ghost.classify_ramp_contact(self.session.level.world());
                self.session.pb_ghost = Some(ghost);
            }
            Err(e) => {
                if self.settings.ghost != GHOST_PB && self.settings.ghost != GHOST_AUTO {
                    eprintln!("  ghost load failed ({}): {e}", path.display());
                } else if self.settings.ghost == GHOST_AUTO {
                    eprintln!("  auto ghost load failed ({}): {e}", path.display());
                }
                self.session.pb_ghost = None;
                self.session.pb_splits.clear();
            }
        }
    }

    /// Freeze pose + velocity + run clock into a new numbered loc.
    fn save_loc(&mut self) {
        let loc = Loc::capture(&self.session.player, &self.session.run_timer.snapshot());
        let speed = loc.speed_2d();
        let time = loc.time_secs;
        match self.session.locs.push(loc) {
            Ok(index) => {
                self.persist_locs();
                println!(
                    "saveloc #{}  {:.0} u/s  t {}",
                    index + 1,
                    speed,
                    format_time(time)
                );
            }
            Err(e) => eprintln!("saveloc: {e}"),
        }
    }

    /// Restore the selected loc. The clock keeps its restored value and keeps
    /// running, but the attempt is practice from here: no PB, no replay.
    fn load_loc(&mut self, index: usize) {
        let Some(loc) = self.session.locs.get(index).cloned() else {
            println!("loadloc: no loc saved");
            return;
        };
        self.session.locs.set_selected(index);

        loc.apply(&mut self.session.player);
        // A loc jump is a teleport: the recorded touch set belongs to wherever
        // we were. Clearing it also re-arms the map's one-shot boosts, which is
        // the point of practising with a loc saved before one.
        self.session.field_state.reset();
        let snap = loc.timer_snapshot();
        self.session
            .run_timer
            .restore(&snap, self.session.player.grounded);
        self.session.run_timer.mark_practice();
        if let Some(zones) = self.session.zones.as_ref() {
            self.session
                .run_timer
                .sync_checkpoints_after_restore(zones, &self.session.player);
        }

        // Collapse the interpolation span — otherwise the camera smears from
        // wherever we were to wherever the loc is.
        self.session.prev_origin = self.session.player.origin;
        self.session.accumulator = 0.0;
        self.session.sync_display = 0.0;

        self.session.finish_recorded = false;
        self.session.pb_delta = self
            .session
            .pb_time
            .filter(|_| self.session.run_timer.phase == TimerPhase::Running)
            .map(|pb| self.session.run_timer.time_secs - pb);
        self.session.pb_flash_left = 0.0;
        self.session.split_flash_left = 0.0;
        self.session.split_flash_line = None;
        // A practice attempt is never a saved run; drop whatever was captured.
        self.session.replay_rec.clear();

        // Move the ghost with the clock, or every delta on screen would be wrong.
        let running = self.session.run_timer.phase == TimerPhase::Running;
        let t = self.session.run_timer.time_secs;
        if let Some(g) = self.session.pb_ghost.as_mut() {
            if running {
                g.seek_secs(t);
            } else {
                g.stop();
            }
        }

        // Same treatment as a reset: re-baseline the detector so the jump can't
        // fire a phantom landing, and re-arm rather than wipe — a loadloc is not
        // a failure.
        self.session.audio_on_ramp = false;
        self.session
            .audio_detect
            .resync(false, self.session.player.grounded);
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
            audio.push(AudioEvent::Rearm);
        }

        println!(
            "loadloc #{}  {:.0} u/s  t {}  (practice)",
            index + 1,
            self.session.player.velocity.length_2d(),
            format_time(self.session.run_timer.time_secs)
        );
    }

    /// Mouse1 / bind path — refuses unless practice mode is armed.
    fn load_selected_loc(&mut self) {
        if !self.session.practice_mode {
            self.flash_notice("PRACTICE MODE OFF · LOADLOC LOCKED");
            println!("loadloc blocked: practice mode off (P to toggle)");
            return;
        }
        match self.session.locs.selected() {
            Some(i) => self.load_loc(i),
            None => println!("loadloc: no loc saved"),
        }
    }

    fn set_practice_mode(&mut self, on: bool) {
        if self.session.practice_mode == on {
            return;
        }
        self.session.practice_mode = on;
        if on {
            self.flash_notice("PRACTICE MODE ON");
            println!("practice mode ON — Mouse1 loadloc armed");
        } else {
            self.flash_notice("PRACTICE MODE OFF");
            println!("practice mode OFF — loadloc locked");
        }
    }

    /// Brief centre-screen message, reusing the checkpoint-split flash slot.
    fn flash_notice(&mut self, msg: &str) {
        self.session.split_flash_line = Some(msg.to_string());
        self.session.split_flash_left = SPLIT_FLASH_SECS;
    }

    fn persist_locs(&self) {
        if let Err(e) = self.session.locs.save() {
            eprintln!("locs save failed: {e}");
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
        self.focus = Focus::Rows;
        self.button_selected = 0;
        // Land the locs list on the active loc so it reads as "this is the one
        // Mouse1 will load".
        self.nav.set(
            PageId::Locs,
            match self.session.locs.selected() {
                Some(i) => i + 1,
                None => LOC_ROW_PRACTICE,
            },
        );
        self.set_capture(false);
        self.refresh_ghost_options();
        self.snap_selection_into_range();
        // The sim is paused, so nothing would update the parameters — zero them
        // so the voice releases instead of holding a note under the menu.
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
        }
    }

    fn close_menu(&mut self, recapture: bool) {
        self.rebinding = None;
        self.menu_open = false;
        self.dragging = None;
        self.persist_settings();
        if recapture {
            self.set_capture(true);
        }
    }

    fn setting_value(&self, s: Setting) -> f32 {
        match s {
            Setting::Sens => self.settings.mouse_sens,
            Setting::Brightness => self.settings.brightness,
            Setting::ShadowLift => self.settings.shadow_lift,
            Setting::Airaccel => self.session.vars.airaccelerate,
            Setting::AudioVolume => self.settings.audio_volume,
            Setting::AudioCore => self.settings.audio_core,
            Setting::AudioAir => self.settings.audio_air,
            Setting::AudioSub => self.settings.audio_sub,
            Setting::TurnSpeed => self.settings.turn_speed,
            _ => 0.0,
        }
    }

    /// Write a continuous setting, clamped by whoever owns the clamp.
    fn set_setting_value(&mut self, s: Setting, v: f32) {
        match s {
            Setting::Sens => self.settings.mouse_sens = Settings::clamp_sens(v),
            Setting::Brightness => self.settings.brightness = Settings::clamp_brightness(v),
            Setting::ShadowLift => self.settings.shadow_lift = Settings::clamp_shadow_lift(v),
            Setting::Airaccel => self.session.vars.airaccelerate = v.clamp(1.0, 1000.0),
            Setting::AudioVolume => {
                self.settings.audio_volume = Settings::clamp_audio_volume(v);
            }
            Setting::AudioCore => self.settings.audio_core = Settings::clamp_audio_level(v),
            Setting::AudioAir => self.settings.audio_air = Settings::clamp_audio_level(v),
            Setting::AudioSub => self.settings.audio_sub = Settings::clamp_audio_level(v),
            Setting::TurnSpeed => self.settings.turn_speed = Settings::clamp_turn_speed(v),
            _ => return,
        }
        self.apply_audio_settings();
    }

    /// Drag / click on a slider track: set the value from a 0..1 position.
    fn set_setting_frac(&mut self, s: Setting, frac: f32) {
        let Some(range) = slider_range(s) else {
            return;
        };
        self.set_setting_value(s, range.value_of(frac));
    }

    /// One notch of a setting: a slider step, or a toggle / cycle.
    fn adjust_setting(&mut self, s: Setting, dir: i32) {
        if let Some(range) = slider_range(s) {
            let v = range.nudge(self.setting_value(s), dir);
            self.set_setting_value(s, v);
            return;
        }
        match s {
            Setting::Vsync => {
                let want_vsync = !self.settings.vsync;
                if !want_vsync && !self.immediate_ok {
                    println!("VSync: Immediate not available on this surface; staying on");
                } else {
                    self.settings.vsync = want_vsync;
                    self.apply_present_mode();
                }
            }
            Setting::ShowSync => self.settings.show_sync_bar = !self.settings.show_sync_bar,
            Setting::ShowKeys => self.settings.show_keys = !self.settings.show_keys,
            Setting::Ghost => self.cycle_ghost(dir),
            Setting::GhostTrail => self.settings.ghost_trail = !self.settings.ghost_trail,
            Setting::Audio => self.settings.audio = !self.settings.audio,
            Setting::Wipe => {
                let next = WipeStyle::from_str_or_default(&self.settings.wipe_style).toggled();
                self.settings.wipe_style = next.as_str().into();
                // Play it on selection: this is a choice you make by ear.
                if let Some(audio) = self.audio.as_ref() {
                    audio.set_wipe_style(next);
                    audio.push(AudioEvent::Wipe);
                }
            }
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
        if let (Some(renderer), Some(surface)) = (self.renderer.as_mut(), self.surface.as_ref()) {
            if renderer.config.present_mode == mode {
                return;
            }
            renderer.config.present_mode = mode;
            surface.configure(&renderer.device, &renderer.config);
            println!("present_mode={mode:?}  vsync={}", self.settings.vsync);
        }
    }

    fn reset(&mut self) {
        let (spawn_origin, spawn_angles) = self.session.level.spawn();
        self.session.player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        for _ in 0..10 {
            self.session.player = tick(
                self.session.level.world(),
                &self.session.player,
                &UserCmd::default(),
                &self.session.vars,
            );
        }
        self.session.prev_origin = self.session.player.origin;
        self.session.accumulator = 0.0;
        self.session.sync_display = 0.0;
        self.clear_run_state();
    }

    /// Throw away the current attempt: clock, splits, ghost, recorded frames.
    /// Everything a fresh run needs zeroed, without moving the player — a wipe
    /// has already been teleported by the map, and only wants this half.
    fn clear_run_state(&mut self) {
        self.session.run_timer.reset();
        self.session.field_state.reset();
        self.session.pb_delta = None;
        self.session.finish_recorded = false;
        self.session.pb_flash_left = 0.0;
        self.session.split_flash_left = 0.0;
        self.session.split_flash_line = None;
        self.session.replay_rec.clear();
        if let Some(g) = self.session.pb_ghost.as_mut() {
            g.stop();
        }
        // Re-baseline the edge detector so respawning can't fire a phantom
        // landing, and mark the restart with the same quiet tick the start zone
        // uses. A manual reset is not a failure and doesn't get a wipe.
        self.session.audio_on_ramp = false;
        self.session
            .audio_detect
            .resync(false, self.session.player.grounded);
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams::default());
            audio.push(AudioEvent::Rearm);
        }
    }

    /// Did the soft respawn we just took end the attempt? See
    /// [`RunTimer::wipe_ends_run`] — the rule lives there so it can be tested
    /// without the event loop.
    fn wipe_ends_the_run(&self) -> bool {
        let Some(zones) = self.session.zones.as_ref() else {
            return false;
        };
        let in_start = zones
            .main
            .start
            .contains_player(self.session.player.origin, self.session.player.hull());
        self.session.run_timer.wipe_ends_run(in_start)
    }

    /// Snap to the current stage start without clearing the run clock / splits.
    /// Linear maps fall back to a full [`Self::reset`].
    fn reset_stage(&mut self) {
        if self.session.zones.is_none() || self.session.run_timer.track_type != TrackType::Staged {
            self.reset();
            return;
        }
        let stage = self.session.run_timer.current_stage;
        let angles = self.session.player.viewangles;
        let (spawn_origin, spawn_angles) = {
            let zones = self.session.zones.as_ref().unwrap();
            stage_respawn_pose(zones, &self.session.level, stage, angles)
        };
        self.session.player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        for _ in 0..10 {
            self.session.player = tick(
                self.session.level.world(),
                &self.session.player,
                &UserCmd::default(),
                &self.session.vars,
            );
        }
        self.session.prev_origin = self.session.player.origin;
        self.session.accumulator = 0.0;
        // Keep phase / time / splits / current_stage; soft-enter so landing
        // inside a stage start doesn't cancel or double-split.
        self.session.run_timer.notify_soft_respawn();
        if let Some(zones) = self.session.zones.as_ref() {
            self.session
                .run_timer
                .sync_stage_after_respawn(zones, &self.session.player);
        }
        self.session.audio_on_ramp = false;
        self.session
            .audio_detect
            .resync(false, self.session.player.grounded);
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
                        .with_title(&self.session.title_base)
                        .with_inner_size(PhysicalSize::new(self.window_w, self.window_h)),
                )
                .expect("window"),
        );

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL | wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("adapter");

        // `Limits::default()` caps a single buffer at 256 MB. Real maps blow
        // straight past that once static props are in the mesh — boreas' is
        // ~396 MB — and the allocation fails rather than degrading. Take what
        // this adapter actually supports; on Apple Silicon that is far higher.
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("mx-surf"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
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

        let atlas = self.session.level.materials();
        let mesh = GpuMesh::from_graybox(&device, self.session.level.mesh(), &atlas.additive_layers);
        let lightmaps = self.session.level.lightmaps();
        let sky = self.session.level.skybox();
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
        if self.held(Bind::Forward) {
            forward += 1.0;
        }
        if self.held(Bind::Back) {
            forward -= 1.0;
        }
        if self.held(Bind::Right) {
            side += 1.0;
        }
        if self.held(Bind::Left) {
            side -= 1.0;
        }
        UserCmd {
            viewangles: self.session.player.viewangles,
            forward_move: forward,
            side_move: side,
            jump: self.held(Bind::Jump),
            duck: self.held(Bind::Duck),
        }
    }

    fn held(&self, b: Bind) -> bool {
        self.keys.contains(&self.binds.key(b))
    }

    /// Keyboard turning (`+left` / `+right`), one tick's worth, applied before
    /// the command is built so the recorded viewangles carry it and a replay
    /// resims identically.
    fn apply_turn_binds(&mut self, dt: f32) {
        let d = turn_delta(
            self.held(Bind::TurnLeft),
            self.held(Bind::TurnRight),
            self.settings.turn_speed,
            dt,
        );
        if d == 0.0 {
            return;
        }
        let yaw = &mut self.session.player.viewangles.yaw;
        *yaw += d;
        if *yaw > 180.0 {
            *yaw -= 360.0;
        }
        if *yaw < -180.0 {
            *yaw += 360.0;
        }
    }

    fn simulate(&mut self, dt_real: f32) {
        if self.menu_open || !matches!(self.mode, Mode::Playing) {
            return;
        }
        let tick_dt = self.session.vars.tick_interval;
        self.session.accumulator += dt_real;
        if self.session.accumulator > 0.25 {
            self.session.accumulator = 0.25;
        }
        while self.session.accumulator >= tick_dt {
            self.session.prev_origin = self.session.player.origin;
            let prev_vel = self.session.player.velocity;
            self.apply_turn_binds(tick_dt);
            let cmd = self.build_cmd();
            let wishing = cmd.forward_move.abs() + cmd.side_move.abs() > 0.0;
            self.session.player = tick(
                self.session.level.world(),
                &self.session.player,
                &cmd,
                &self.session.vars,
            );
            // Trigger touches are processed at the end of a move, so a booster
            // pays out on the tick you leave it and the basevelocity a volume
            // asserts is carried by the next one.
            self.session.level.apply_fields(
                &mut self.session.player,
                &mut self.session.field_state,
                tick_dt,
            );

            let mut soft_respawned = false;
            if let Some((dest, angles)) = self
                .session
                .level
                .touch_teleport(self.session.player.origin)
            {
                self.session.player.origin = dest;
                self.session.player.viewangles = angles;
                // Whatever volume we were standing in is not where we are now.
                self.session.player.basevelocity = Vec3::ZERO;
                soft_respawned = true;
            }
            if self.session.player.origin.z < self.session.level.kill_z() {
                let (spawn_origin, spawn_angles) =
                    if self.session.run_timer.track_type == TrackType::Staged {
                        if let Some(zones) = self.session.zones.as_ref() {
                            stage_respawn_pose(
                                zones,
                                &self.session.level,
                                self.session.run_timer.current_stage,
                                self.session.player.viewangles,
                            )
                        } else {
                            self.session.level.spawn()
                        }
                    } else {
                        self.session.level.spawn()
                    };
                self.session.player.origin = spawn_origin;
                self.session.player.viewangles = spawn_angles;
                self.session.player.velocity = Vec3::ZERO;
                self.session.player.basevelocity = Vec3::ZERO;
                self.session.player.gravity_scale = 1.0;
                self.session.player.grounded = true;
                // A fresh life re-arms every one-shot the map gates by name.
                self.session.field_state.reset();
                soft_respawned = true;
            }

            if soft_respawned {
                if self.wipe_ends_the_run() {
                    // Back in the start box on a linear map: the attempt is
                    // over. Clearing here (before the timer ticks) lets the
                    // same tick re-arm, so you can simply go again.
                    self.clear_run_state();
                } else {
                    self.session.run_timer.notify_soft_respawn();
                    if let Some(zones) = self.session.zones.as_ref() {
                        self.session
                            .run_timer
                            .sync_stage_after_respawn(zones, &self.session.player);
                    }
                }
            }

            // Audio observation. Surfing is airborne by definition, so ramp
            // contact can't come from `grounded` — `is_on_surf_ramp` is the same
            // probe the ghost trail already uses, so the two agree.
            if self.audio.is_some() && self.settings.audio {
                let hull = self.session.player.hull();
                let on_ramp = is_on_surf_ramp(
                    self.session.level.world(),
                    self.session.player.origin,
                    &hull,
                );
                self.session.audio_on_ramp = on_ramp;
                let obs = Observation {
                    dt: tick_dt,
                    speed: self.session.player.velocity.length_2d(),
                    on_ramp,
                    grounded: self.session.player.grounded,
                    wiped: soft_respawned,
                };
                let emitted = self.session.audio_detect.observe(obs);
                if let Some(audio) = self.audio.as_ref() {
                    for ev in emitted.iter() {
                        audio.push(ev);
                    }
                }
            }
            if let Some(zones) = self.session.zones.as_ref() {
                let phase_before = self.session.run_timer.phase;
                let was_finished = self.session.run_timer.is_finished();
                let pb_splits = self.session.pb_splits.clone();
                self.session
                    .run_timer
                    .tick(zones, &mut self.session.player, tick_dt, &pb_splits);
                if let Some(ev) = self.session.run_timer.take_split_event() {
                    let staged = self.session.run_timer.track_type == TrackType::Staged;
                    let line = format_split_line(ev, staged);
                    println!("{line}");
                    self.session.split_flash_line = Some(line);
                    self.session.split_flash_left = SPLIT_FLASH_SECS;
                }
                self.record_replay_tick(phase_before, &cmd);
                if self.session.run_timer.is_finished()
                    && !was_finished
                    && !self.session.finish_recorded
                {
                    self.on_finish();
                }
                if self.session.run_timer.phase == TimerPhase::Running {
                    if let Some(pb) = self.session.pb_time {
                        self.session.pb_delta = Some(self.session.run_timer.time_secs - pb);
                    }
                }
            }

            let sample = air_strafe_sync(
                prev_vel,
                self.session.player.velocity,
                wishing,
                !self.session.player.grounded,
            );
            if sample > self.session.sync_display {
                self.session.sync_display = self.session.sync_display * 0.5 + sample * 0.5;
            } else {
                self.session.sync_display = self.session.sync_display * 0.85 + sample * 0.15;
            }
            self.session.accumulator -= tick_dt;
        }
        self.session.alpha = self.session.accumulator / tick_dt;

        // One parameter push per frame. Everything is smoothed per sample on
        // the audio thread, so this rate only has to beat the smoothing.
        if let Some(audio) = self.audio.as_ref() {
            audio.set_params(AudioParams {
                speed: self.session.player.velocity.length_2d(),
                sync: self.session.sync_display,
                contact: if self.session.audio_on_ramp { 1.0 } else { 0.0 },
            });
        }
    }

    fn hud_timer_phase(&self) -> HudTimerPhase {
        match self.session.run_timer.phase {
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

        let origin = self
            .session
            .prev_origin
            .lerp(self.session.player.origin, self.session.alpha);
        let eye = origin + Vec3::new(0.0, 0.0, self.session.player.hull().eye_height);
        let speed = self.session.player.velocity.length_2d();
        let show_keys = if self.settings.show_keys && !self.page_open() {
            Some(ShowKeysState {
                forward: self.held(Bind::Forward),
                back: self.held(Bind::Back),
                left: self.held(Bind::Left),
                right: self.held(Bind::Right),
                jump: self.held(Bind::Jump),
            })
        } else {
            None
        };
        let page = self.build_page();
        let timer_phase = self.hud_timer_phase();
        let perf_line = self.frame_stats.line();
        let racing = self.session.run_timer.phase == TimerPhase::Running
            && self
                .session
                .pb_ghost
                .as_ref()
                .map(|g| g.active)
                .unwrap_or(false);
        let (ghost_time_delta, ghost_speed_delta, ghost_pose, trail_pts) = if racing {
            let g = self.session.pb_ghost.as_ref().unwrap();
            let td = Some(self.session.run_timer.time_secs - g.current_time());
            let sd = g.current_speed_2d().map(|gs| speed - gs);
            let pose = g
                .sample(self.session.alpha)
                .map(|(o, _, ducked)| GhostPose { origin: o, ducked });
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
        let split_line = if self.session.split_flash_left > 0.0 {
            self.session.split_flash_line.clone()
        } else {
            None
        };
        let stage_line = self
            .session
            .zones
            .as_ref()
            .and_then(|z| self.session.run_timer.stage_hud_label(z));
        let hud = HudState {
            speed,
            sync: self.session.sync_display,
            grounded: self.session.player.grounded,
            speed_scale: 3500.0,
            time_secs: self.session.run_timer.display_time(),
            pb_time_secs: self.session.pb_time,
            pb_delta_secs: self.session.pb_delta,
            timer_phase,
            show_sync_bar: self.settings.show_sync_bar,
            show_keys,
            pb_flash: self.session.pb_flash_left > 0.0,
            split_line,
            stage_line,
            practice: self.session.run_timer.practice,
            practice_mode: self.session.practice_mode,
            ghost_time_delta,
            ghost_speed_delta,
            page,
            perf_line,
            time: self.start_time.elapsed().as_secs_f32(),
        };
        let viewangles = self.session.player.viewangles;
        let title_base = self.session.title_base.clone();
        let menu_open = self.menu_open;
        let grounded = self.session.player.grounded;
        let time_label = self.session.run_timer.display_time().map(format_time);
        let aa = self.session.vars.airaccelerate;
        let sens = self.settings.mouse_sens;

        let surface = self.surface.as_ref().unwrap();
        let renderer = self.renderer.as_mut().unwrap();
        let window = self.window.as_ref().unwrap();

        let aspect = renderer.config.width as f32 / renderer.config.height.max(1) as f32;
        let camera = Camera::new(eye, viewangles, aspect);
        let view = ViewParams::new(
            Settings::clamp_brightness(self.settings.brightness),
            Settings::clamp_shadow_lift(self.settings.shadow_lift),
        );

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
        let phase = self.session.run_timer.phase;
        // A practice attempt is never saved — recording it would bake a position
        // discontinuity into the frame stream and break re-simulation. The ghost
        // still runs, because practising against it is the whole point.
        if self.session.run_timer.practice {
            if self.session.replay_rec.is_active() {
                self.session.replay_rec.clear();
            }
            if phase == TimerPhase::Running {
                if let Some(g) = self.session.pb_ghost.as_mut() {
                    g.advance();
                }
            }
            return;
        }
        match phase {
            TimerPhase::Running => {
                if phase_before != TimerPhase::Running {
                    self.session.replay_rec.begin();
                    if let Some(g) = self.session.pb_ghost.as_mut() {
                        g.begin();
                    }
                } else if let Some(g) = self.session.pb_ghost.as_mut() {
                    g.advance();
                }
                self.session.replay_rec.push(cmd, &self.session.player);
            }
            TimerPhase::Finished => {
                if phase_before == TimerPhase::Running {
                    self.session.replay_rec.push(cmd, &self.session.player);
                }
            }
            TimerPhase::Armed | TimerPhase::Idle => {
                if matches!(phase_before, TimerPhase::Running | TimerPhase::Finished) {
                    if self.session.replay_rec.is_active() {
                        self.session.replay_rec.clear();
                    }
                    if let Some(g) = self.session.pb_ghost.as_mut() {
                        g.stop();
                    }
                }
            }
        }
    }

    fn on_finish(&mut self) {
        self.session.finish_recorded = true;
        let time = self.session.run_timer.time_secs;
        let splits = self.session.run_timer.splits.clone();
        if let Some(pb) = self.session.pb_time {
            self.session.pb_delta = Some(time - pb);
        } else {
            self.session.pb_delta = Some(0.0);
        }
        if self.session.run_timer.practice {
            // Theoretical time only: shown and frozen, but not a result.
            self.session.replay_rec.clear();
            println!(
                "Finished in {} — PRACTICE (loc load), not recorded",
                format_time(time)
            );
            return;
        }
        let Some(name) = self.session.level.map_name().map(|s| s.to_string()) else {
            self.session.replay_rec.clear();
            return;
        };

        let saved_path = self
            .session
            .replay_rec
            .finish(&name, time, &self.session.vars, &splits)
            .and_then(|replay| {
                let path = replay::new_run_replay_path(&name, time);
                match replay.save(&path) {
                    Ok(()) => {
                        println!("Replay saved {} ({} splits)", path.display(), splits.len());
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
                    self.session.pb_time = Some(new_pb);
                    self.session.pb_delta = Some(0.0);
                    self.session.pb_flash_left = PB_FLASH_SECS;
                    if let Some(src) = saved_path.as_ref() {
                        let pb_path = replay::pb_replay_path(&name);
                        if let Err(e) = std::fs::copy(src, &pb_path) {
                            eprintln!("PB replay copy failed: {e}");
                        } else {
                            println!("PB replay {}", pb_path.display());
                        }
                    }
                    self.session.pb_splits = splits;
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
                        self.session
                            .pb_time
                            .map(format_time)
                            .unwrap_or_else(|| "-".into())
                    );
                }
                Err(e) => eprintln!("PB write failed: {e}"),
            }
        }
    }

    // -- page input --------------------------------------------------------

    /// Keyboard on any menu page — shell or pause. One handler, so a key that
    /// works on one page works on all of them.
    fn page_key(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) {
        match code {
            KeyCode::Escape => self.page_back(event_loop),
            KeyCode::Tab => {
                if self.buttons().is_empty() {
                    return;
                }
                self.focus = match self.focus {
                    Focus::Rows => Focus::Buttons,
                    Focus::Buttons => Focus::Rows,
                };
            }
            KeyCode::KeyQ | KeyCode::KeyE if matches!(self.mode, Mode::Playing) => {
                let dir = if code == KeyCode::KeyE { 1 } else { -1 };
                self.cycle_tab(dir);
            }
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => match self.focus {
                Focus::Rows => {
                    let row = self.selected_row();
                    self.activate_row(row, event_loop);
                }
                Focus::Buttons => {
                    let i = self.button_selected;
                    self.activate_button(i, event_loop);
                }
            },
            KeyCode::KeyX | KeyCode::Backspace | KeyCode::Delete => {
                let row = self.selected_row();
                self.delete_loc_row(row);
            }
            KeyCode::ArrowUp | KeyCode::KeyW => {
                if self.focus == Focus::Rows {
                    self.move_selection(-1);
                }
            }
            KeyCode::ArrowDown | KeyCode::KeyS => {
                if self.focus == Focus::Rows {
                    self.move_selection(1);
                }
            }
            KeyCode::ArrowLeft | KeyCode::KeyA => self.page_horizontal(-1),
            KeyCode::ArrowRight | KeyCode::KeyD => self.page_horizontal(1),
            KeyCode::Home => {
                self.set_selected_row(0);
                self.snap_selection_into_range();
            }
            KeyCode::End => {
                let n = self.page_entries().len();
                self.set_selected_row(n.saturating_sub(1));
                self.snap_selection_into_range();
            }
            _ => {}
        }
    }

    /// ←→ adjusts the selected setting, or walks the action bar when it has
    /// focus.
    fn page_horizontal(&mut self, dir: i32) {
        match self.focus {
            Focus::Rows => {
                let row = self.selected_row();
                self.adjust_row(row, dir);
            }
            Focus::Buttons => {
                let n = self.buttons().len();
                if n > 0 {
                    self.button_selected =
                        ((self.button_selected as i32 + dir).rem_euclid(n as i32)) as usize;
                }
            }
        }
    }

    fn cycle_tab(&mut self, dir: i32) {
        let n = PauseTab::ALL.len() as i32;
        let i = (self.pause_tab.index() as i32 + dir).rem_euclid(n) as usize;
        self.set_tab(PauseTab::ALL[i]);
    }

    fn set_tab(&mut self, tab: PauseTab) {
        if self.pause_tab == tab {
            return;
        }
        self.pause_tab = tab;
        self.focus = Focus::Rows;
        self.snap_selection_into_range();
    }

    fn delete_loc_row(&mut self, row: usize) {
        if self.page_id() != PageId::Locs {
            return;
        }
        let Some(i) = locs::loc_index_for_row(self.session.locs.len(), row) else {
            return;
        };
        self.session.locs.remove(i);
        self.persist_locs();
        println!("deleted loc #{} ({} left)", i + 1, self.session.locs.len());
        let last = locs::locs_page_len(self.session.locs.len()) - 1;
        let row = row.min(last);
        self.set_selected_row(row);
    }

    /// Left click anywhere on a menu page.
    fn page_click(&mut self, event_loop: &ActiveEventLoop) {
        let (mx, my) = self.cursor_px;
        let Some((page, layout)) = self.page_layout() else {
            return;
        };

        if let Some(i) = layout.tabs.iter().position(|r| r.contains(mx, my)) {
            if let Some(tab) = PauseTab::ALL.get(i).copied() {
                self.set_tab(tab);
            }
            return;
        }
        if let Some(i) = layout.buttons.iter().position(|r| r.contains(mx, my)) {
            self.focus = Focus::Buttons;
            self.button_selected = i;
            self.activate_button(i, event_loop);
            return;
        }
        if let Some(drawn) = layout.panel.row_at(mx, my) {
            let row = drawn + page.panel.scroll;
            let Some(entry) = self.page_entries().into_iter().nth(row) else {
                return;
            };
            if !entry.0.kind.selectable() {
                return;
            }
            self.focus = Focus::Rows;
            self.set_selected_row(row);
            // A slider follows the cursor from here until the button comes up.
            if let (RowKind::Slider(_), RowAction::Adjust(s)) = (entry.0.kind, &entry.1) {
                if let Some(frac) = layout.panel.slider_frac_at(mx) {
                    let s = *s;
                    self.dragging = Some(row);
                    self.set_setting_frac(s, frac);
                }
                return;
            }
            // A loc row selects on click; Enter or "Load selected" is what goes
            // there, so a misclick in the list can't teleport you.
            if matches!(entry.1, RowAction::Loc(_)) {
                return;
            }
            self.activate_row(row, event_loop);
            return;
        }
        // Clicking off the panel resumes / backs out, as it always has — but
        // never where "back" would quit the game.
        if !layout.panel.contains(mx, my) && self.back_is_click_safe() {
            self.page_back(event_loop);
        }
    }

    /// Cursor moved with the left button down: keep feeding a slider.
    fn page_drag(&mut self) {
        let Some(row) = self.dragging else {
            return;
        };
        let Some(RowAction::Adjust(s)) = self.action_at(row) else {
            self.dragging = None;
            return;
        };
        let Some((_, layout)) = self.page_layout() else {
            return;
        };
        // No `slider_frac_at` guard here: once a drag has started, tracking the
        // cursor past the left end of the track should read as 0, not as "stop".
        let (x0, x1) = layout.panel.slider_track();
        let frac = ((self.cursor_px.0 - x0) / (x1 - x0)).clamp(0.0, 1.0);
        self.set_setting_frac(s, frac);
    }

    /// Right click: delete the loc under the cursor, else go back a page.
    fn page_right_click(&mut self, event_loop: &ActiveEventLoop) {
        let (mx, my) = self.cursor_px;
        if let Some((page, layout)) = self.page_layout() {
            if let Some(drawn) = layout.panel.row_at(mx, my) {
                let row = drawn + page.panel.scroll;
                if matches!(self.action_at(row), Some(RowAction::Loc(_))) {
                    self.delete_loc_row(row);
                    return;
                }
            }
        }
        if self.back_is_click_safe() {
            self.page_back(event_loop);
        }
    }

    /// Wheel adjusts the hovered setting, otherwise steps the list.
    fn page_scroll(&mut self, dy: f32) {
        if dy.abs() < 0.01 {
            return;
        }
        let dir = if dy > 0.0 { 1 } else { -1 };
        let (mx, my) = self.cursor_px;
        let hovered = self.page_layout().and_then(|(page, layout)| {
            layout
                .panel
                .row_at(mx, my)
                .map(|d| d + page.panel.scroll)
                .filter(|r| *r < page.panel.rows.len())
        });
        if let Some(row) = hovered {
            if matches!(self.action_at(row), Some(RowAction::Adjust(s)) if slider_range(s).is_some())
            {
                self.focus = Focus::Rows;
                self.set_selected_row(row);
                self.adjust_row(row, dir);
                return;
            }
        }
        // Scrolling a list moves the cursor; the window follows it.
        self.focus = Focus::Rows;
        self.move_selection(-dir);
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
                    // Any open page owns input entirely: no capture, no loc
                    // binds, no player keys.
                    if self.page_open() {
                        if let Some(b) = self.rebinding.take() {
                            if repeat {
                                self.rebinding = Some(b);
                            } else if code != KeyCode::Escape && self.binds.set(b, code) {
                                self.settings.binds = self.binds.to_map();
                                self.persist_settings();
                            } else if code != KeyCode::Escape {
                                // Not a bindable key: keep waiting.
                                self.rebinding = Some(b);
                            }
                            return;
                        }
                        let repeatable = matches!(
                            code,
                            KeyCode::ArrowUp
                                | KeyCode::ArrowDown
                                | KeyCode::ArrowLeft
                                | KeyCode::ArrowRight
                                | KeyCode::KeyW
                                | KeyCode::KeyS
                                | KeyCode::KeyA
                                | KeyCode::KeyD
                        );
                        if !repeat || repeatable {
                            self.page_key(code, event_loop);
                        }
                        return;
                    }
                    self.keys.insert(code);
                    match self.binds.bind_of(code) {
                        Some(Bind::Reset) => self.reset(),
                        Some(Bind::ResetStage) => self.reset_stage(),
                        Some(Bind::Practice) => {
                            if !repeat {
                                let on = !self.session.practice_mode;
                                self.set_practice_mode(on);
                            }
                        }
                        _ => {}
                    }
                    match code {
                        KeyCode::Escape => {
                            if self.mouse_captured {
                                self.open_menu();
                            } else {
                                // Uncaptured + Esc → back to the title screen.
                                self.leave_to_main_menu();
                            }
                        }
                        KeyCode::BracketLeft => {
                            self.adjust_setting(Setting::Sens, -1);
                            self.persist_settings();
                        }
                        KeyCode::BracketRight => {
                            self.adjust_setting(Setting::Sens, 1);
                            self.persist_settings();
                        }
                        KeyCode::Minus => self.adjust_setting(Setting::Airaccel, -1),
                        KeyCode::Equal => self.adjust_setting(Setting::Airaccel, 1),
                        _ => {}
                    }
                }
                ElementState::Released => {
                    self.keys.remove(&code);
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_px = (position.x as f32, position.y as f32);
                // A slider keeps following the cursor while the button is down,
                // which is the difference between a bar and a real slider.
                if self.dragging.is_some() {
                    self.page_drag();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.page_open() {
                    let dy = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                    };
                    self.page_scroll(dy);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Released {
                    if button == MouseButton::Left {
                        self.dragging = None;
                    }
                    return;
                }
                // Loc binds are live only during actual play. On a menu page the
                // same clicks drive the UI, so a click on a panel can never move
                // the player.
                if self.page_open() {
                    match button {
                        MouseButton::Left => self.page_click(event_loop),
                        MouseButton::Right => self.page_right_click(event_loop),
                        _ => {}
                    }
                } else if !self.mouse_captured {
                    if button == MouseButton::Left {
                        self.set_capture(true);
                    }
                } else {
                    match button {
                        MouseButton::Left => self.load_selected_loc(),
                        MouseButton::Right => self.save_loc(),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.poll_load();
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;
                if self.session.pb_flash_left > 0.0 {
                    self.session.pb_flash_left = (self.session.pb_flash_left - dt).max(0.0);
                }
                if self.session.split_flash_left > 0.0 {
                    self.session.split_flash_left = (self.session.split_flash_left - dt).max(0.0);
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
            if !self.mouse_captured || self.page_open() {
                return;
            }
            let yaw_scale = self.settings.mouse_sens * MOUSE_YAW_SCALE;
            let pitch_scale = self.settings.mouse_sens * MOUSE_PITCH_SCALE;
            self.session.player.viewangles.yaw -= dx as f32 * yaw_scale;
            self.session.player.viewangles.pitch += dy as f32 * pitch_scale;
            self.session.player.viewangles.pitch =
                self.session.player.viewangles.pitch.clamp(-89.0, 89.0);
            if self.session.player.viewangles.yaw > 180.0 {
                self.session.player.viewangles.yaw -= 360.0;
            }
            if self.session.player.viewangles.yaw < -180.0 {
                self.session.player.viewangles.yaw += 360.0;
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

/// BSPs are not in the repo (too large); point the user at the fetcher.
fn fetch_hint(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("surf_summit");
    format!(
        "map file not found. BSPs are not committed — fetch it with:\n  \
         python3 tools/fetch_maps.py {stem}\n(or `--batch tests` for the \
         test corpus, `--all` for everything)"
    )
}

/// Maps offered by the picker: every `.bsp` under `assets/maps`, alphabetical,
/// with the generated graybox arena last so it is always available even on a
/// checkout with no maps fetched.
fn discover_maps() -> Vec<MapEntry> {
    let mut out: Vec<MapEntry> = std::fs::read_dir(surf_app::assets::maps_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("bsp"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            let label = stem.strip_prefix("surf_").unwrap_or(&stem).to_string();
            Some(MapEntry {
                label,
                name: stem,
                path: Some(p),
                pb: None,
                wr: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out.push(MapEntry {
        label: "graybox arena".into(),
        name: GRAYBOX_MAP.to_string(),
        path: None,
        pb: None,
        wr: None,
    });
    out
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
                // Bare argument: a map path, or a short name like `summit`.
                map_path = Some(surf_app::assets::resolve_map_arg(s));
                i += 1;
            }
        }
    }

    LaunchOpts {
        map_path,
        graybox,
        window_w,
        window_h,
        perf_secs,
        vsync,
        ghost_path,
    }
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn main() {
    let opts = parse_args();
    println!("mx-surf");
    println!("Assets: {}", surf_app::assets::root().display());
    println!("Click to capture. WASD, Space, R reset, T stage, Esc menu, [ ] sens, - = airaccel.");
    println!(
        "Movevars: aa={} accel={} friction={} tick={:.0}Hz autobhop={} (Momentum surf defaults).",
        MoveVars::momentum_surf().airaccelerate,
        MoveVars::momentum_surf().accelerate,
        MoveVars::momentum_surf().friction,
        1.0 / MoveVars::momentum_surf().tick_interval,
        MoveVars::momentum_surf().autobhop,
    );
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
