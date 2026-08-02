//! osx-surf app: graybox (M0), real BSP (M1), timer/zones (M2).
//!
//! Usage:
//!   cargo run -p surf-app --release
//!   cargo run -p surf-app --release -- assets/maps/surf_summit.bsp
//!   cargo run -p surf-app --release -- --graybox
//!
//! Controls: WASD move, mouse look, Space jump (autobhop), R reset, Esc quit.
//!
//! macOS note: NSEvent mouse deltas are OS-accelerated. For fair feel, disable
//! pointer acceleration (System Settings → Mouse → Pointer acceleration off),
//! or: `defaults write -g com.apple.mouse.scaling -integer -1`

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use surf_app::pb::PbStore;
use surf_app::timer::{RunTimer, TimerPhase};
use surf_app::zones::{self, MapZones};
use surf_core::graybox::{self, GrayboxMesh, GrayboxWorld};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::{MoveVars, PlayerState, UserCmd};
use surf_core::{air_strafe_sync, tick, World};
use surf_map::{LightmapAtlas, LoadedMap, MaterialAtlas, SkyboxAtlas};
use surf_render::{Camera, GpuMesh, HudState, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Source m_yaw / m_pitch defaults.
const MOUSE_YAW_SCALE: f32 = 0.022;
const MOUSE_PITCH_SCALE: f32 = 0.022;
/// Feel-check default (~5× stock 1.0 sens). `[` / `]` adjust at runtime.
const DEFAULT_SENS: f32 = 5.0;

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

    fn title(&self) -> String {
        match self {
            Level::Graybox(_) => "osx-surf M0 — graybox".into(),
            Level::Map(m) => format!("osx-surf M3 — {}", m.name),
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
    accumulator: f32,
    last_frame: Instant,
    prev_origin: Vec3,
    alpha: f32,
    hud_timer: f32,
    mouse_sens: f32,
    sync_display: f32,
    zones: Option<MapZones>,
    run_timer: RunTimer,
    pb_store: Option<PbStore>,
    pb_time: Option<f32>,
    /// Live delta vs PB while running/finished; set on finish.
    pb_delta: Option<f32>,
    finish_recorded: bool,
}

impl App {
    fn new(level: Level, zones: Option<MapZones>) -> Self {
        let title_base = level.title();
        let (spawn_origin, spawn_angles) = level.spawn();
        let mut player = PlayerState {
            origin: spawn_origin,
            viewangles: spawn_angles,
            grounded: true,
            ..PlayerState::default()
        };
        let vars = MoveVars::momentum_surf();
        for _ in 0..10 {
            player = tick(level.world(), &player, &UserCmd::default(), &vars);
        }
        let track_type = zones
            .as_ref()
            .map(|z| z.track_type)
            .unwrap_or(surf_app::zones::TrackType::Linear);
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
        Self {
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
            accumulator: 0.0,
            last_frame: Instant::now(),
            alpha: 1.0,
            hud_timer: 0.0,
            mouse_sens: DEFAULT_SENS,
            sync_display: 0.0,
            zones,
            run_timer: RunTimer::new(track_type),
            pb_store,
            pb_time,
            pb_delta: None,
            finish_recorded: false,
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
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&self.title_base)
                        .with_inner_size(PhysicalSize::new(1280, 800)),
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
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mesh = GpuMesh::from_graybox(&device, self.level.mesh());
        let atlas = self.level.materials();
        let lightmaps = self.level.lightmaps();
        let sky = self.level.skybox();
        let renderer = Renderer::new(device, queue, config, mesh, &atlas, &lightmaps, &sky);

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
            self.player = tick(self.level.world(), &self.player, &cmd, &self.vars);

            let mut soft_respawned = false;
            if let Some((dest, angles)) = self.level.touch_teleport(self.player.origin) {
                self.player.origin = dest;
                self.player.viewangles = angles;
                // Source keeps velocity on ordinary trigger_teleport.
                soft_respawned = true;
            }
            if self.player.origin.z < self.level.kill_z() {
                let (spawn_origin, spawn_angles) = self.level.spawn();
                self.player.origin = spawn_origin;
                self.player.viewangles = spawn_angles;
                self.player.velocity = Vec3::ZERO;
                self.player.grounded = true;
                soft_respawned = true;
            }

            if soft_respawned {
                // Linear: fail TP / kill-z into start must not cancel the run.
                self.run_timer.notify_soft_respawn();
            }
            if let Some(zones) = self.zones.as_ref() {
                let was_finished = self.run_timer.is_finished();
                self.run_timer.tick(zones, &mut self.player, tick_dt);
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
    }

    fn render_frame(&mut self) {
        let (Some(surface), Some(renderer), Some(window)) = (
            self.surface.as_ref(),
            self.renderer.as_mut(),
            self.window.as_ref(),
        ) else {
            return;
        };

        let origin = self.prev_origin.lerp(self.player.origin, self.alpha);
        let eye = origin + Vec3::new(0.0, 0.0, self.player.hull().eye_height);
        let aspect = renderer.config.width as f32 / renderer.config.height.max(1) as f32;
        let camera = Camera::new(eye, self.player.viewangles, aspect);

        let speed = self.player.velocity.length_2d();
        let hud = HudState {
            speed,
            sync: self.sync_display,
            grounded: self.player.grounded,
            speed_scale: 3500.0,
            time_secs: self.run_timer.display_time(),
            pb_delta_secs: self.pb_delta,
            timer_finished: self.run_timer.is_finished(),
        };

        match renderer.render(surface, &camera, hud) {
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
            let g = if self.player.grounded { "G" } else { "A" };
            let t = self
                .run_timer
                .display_time()
                .map(surf_app::timer::format_time)
                .unwrap_or_else(|| "-".into());
            window.set_title(&format!(
                "{}  |  {speed:7.1} u/s  t {t}  sync {:4.0}%  [{g}]  aa {:.0}  sens {:.1}",
                self.title_base,
                self.sync_display,
                self.vars.airaccelerate,
                self.mouse_sens,
            ));
        }
    }

    fn on_finish(&mut self) {
        self.finish_recorded = true;
        let time = self.run_timer.time_secs;
        if let Some(pb) = self.pb_time {
            self.pb_delta = Some(time - pb);
        } else {
            self.pb_delta = Some(0.0);
        }
        let Some(name) = self.level.map_name().map(|s| s.to_string()) else {
            return;
        };
        if let Some(store) = self.pb_store.as_ref() {
            match store.record_finish(&name, time) {
                Ok(Some(new_pb)) => {
                    self.pb_time = Some(new_pb);
                    self.pb_delta = Some(0.0);
                    println!("PB! {} — {}", name, surf_app::timer::format_time(new_pb));
                }
                Ok(None) => {
                    println!(
                        "Finished {} in {} (PB {})",
                        name,
                        surf_app::timer::format_time(time),
                        self.pb_time
                            .map(surf_app::timer::format_time)
                            .unwrap_or_else(|| "-".into())
                    );
                }
                Err(e) => eprintln!("PB write failed: {e}"),
            }
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
                        ..
                    },
                ..
            } => match state {
                ElementState::Pressed => {
                    self.keys.insert(code);
                    match code {
                        KeyCode::Escape => {
                            if self.mouse_captured {
                                self.set_capture(false);
                            } else {
                                event_loop.exit();
                            }
                        }
                        KeyCode::KeyR => self.reset(),
                        KeyCode::BracketLeft => {
                            self.mouse_sens = (self.mouse_sens / 1.25).max(0.5);
                        }
                        KeyCode::BracketRight => {
                            self.mouse_sens = (self.mouse_sens * 1.25).min(20.0);
                        }
                        KeyCode::Minus => {
                            self.vars.airaccelerate = (self.vars.airaccelerate / 1.5).max(1.0);
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
                if !self.mouse_captured {
                    self.set_capture(true);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;
                self.simulate(dt);
                self.render_frame();
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
            if !self.mouse_captured {
                return;
            }
            let yaw_scale = self.mouse_sens * MOUSE_YAW_SCALE;
            let pitch_scale = self.mouse_sens * MOUSE_PITCH_SCALE;
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

fn load_zones_for(map_path: &std::path::Path) -> Option<MapZones> {
    let zpath = zones::zones_path_for_map(map_path);
    match zones::load_zones_file(&zpath) {
        Ok(z) => {
            println!(
                "  zones={}  start_cap={:.0}  cps={}",
                zpath.display(),
                z.main.limit_start_ground_speed,
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

fn parse_args() -> (Level, Option<MapZones>) {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--graybox") {
        return (Level::Graybox(graybox::surf_ramp_arena()), None);
    }
    args.retain(|a| a != "--graybox");
    if let Some(path) = args.first() {
        let path = PathBuf::from(path);
        println!("Loading map {} …", path.display());
        let map = LoadedMap::load_path(&path).unwrap_or_else(|e| {
            eprintln!("failed to load {}: {e}", path.display());
            std::process::exit(1);
        });
        println!(
            "  brushes={}  tris={}  teleports={}  spawn=({:.0},{:.0},{:.0})",
            map.world.brushes.len(),
            map.mesh.tris.len(),
            map.teleports.len(),
            map.spawn_origin.x,
            map.spawn_origin.y,
            map.spawn_origin.z,
        );
        let zones = load_zones_for(&path);
        return (Level::Map(map), zones);
    }
    // Default M1/M2 target when present; else graybox.
    let summit = PathBuf::from("assets/maps/surf_summit.bsp");
    if summit.is_file() {
        println!("Loading default map {} …", summit.display());
        match LoadedMap::load_path(&summit) {
            Ok(map) => {
                println!(
                    "  brushes={}  tris={}  teleports={}  spawn=({:.0},{:.0},{:.0})",
                    map.world.brushes.len(),
                    map.mesh.tris.len(),
                    map.teleports.len(),
                    map.spawn_origin.x,
                    map.spawn_origin.y,
                    map.spawn_origin.z,
                );
                let zones = load_zones_for(&summit);
                return (Level::Map(map), zones);
            }
            Err(e) => {
                eprintln!("summit load failed ({e}); falling back to graybox");
            }
        }
    }
    (Level::Graybox(graybox::surf_ramp_arena()), None)
}

fn main() {
    let (level, zones) = parse_args();
    let title = level.title();
    println!("{title}");
    println!(
        "Click to capture. WASD, Space, R reset, [ ] sens, - = airaccel, Esc release/quit."
    );
    println!(
        "Movevars: aa={} accel={} friction={} tick={:.0}Hz autobhop={} (Momentum surf defaults).",
        MoveVars::momentum_surf().airaccelerate,
        MoveVars::momentum_surf().accelerate,
        MoveVars::momentum_surf().friction,
        1.0 / MoveVars::momentum_surf().tick_interval,
        MoveVars::momentum_surf().autobhop,
    );
    if zones.is_some() {
        println!("Timer: leave start (or jump) to begin; touch end to finish. R clears run.");
    }
    println!(
        "macOS: disable pointer acceleration for fair mouse feel \
         (System Settings → Mouse → Pointer acceleration)."
    );

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(level, zones);
    event_loop.run_app(&mut app).expect("run");
}
