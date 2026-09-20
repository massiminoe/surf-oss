//! Dedicated content setup window. The downloader never owns UI state.
use std::{sync::Arc, time::Instant};
use surf_app::content::SetupEvent;
use surf_render::{
    Camera, GpuMesh, HudState, MenuPage, MenuPanel, MenuRow, Renderer, RowTone, ViewParams,
};
use winit::{event_loop::ActiveEventLoop, window::Window};

pub struct SetupWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    renderer: Renderer,
    steps: [(bool, String); 3],
    active: usize,
    log: Vec<String>,
    pub running: bool,
    failed: bool,
    pub cursor: (f32, f32),
    pub scroll: usize,
    pub selected_button: usize,
    started: Instant,
}

impl SetupWindow {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, String> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("surf-oss — Set up content")
                        .with_inner_size(winit::dpi::LogicalSize::new(1000., 760.))
                        .with_min_inner_size(winit::dpi::LogicalSize::new(900., 680.)),
                )
                .map_err(|e| e.to_string())?,
        );
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or("No graphics adapter for setup")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Content setup"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .map_err(|e| e.to_string())?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: caps
                .formats
                .iter()
                .copied()
                .find(|f| f.is_srgb())
                .unwrap_or(caps.formats[0]),
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let level = surf_app::session::Session::graybox(None, None).level;
        let atlas = level.materials();
        let mesh = GpuMesh::from_graybox(&device, level.mesh(), &atlas);
        let renderer = Renderer::new(
            device,
            queue,
            config,
            mesh,
            &atlas,
            &level.lightmaps(),
            &level.skybox(),
        );
        let game = surf_map::stock::discover_game_dir()
            .filter(|p| surf_map::stock::validate_game_dir(p).is_ok());
        let found = game.is_some();
        Ok(Self {
            window,
            surface,
            renderer,
            steps: [
                (
                    found,
                    game.map(|p| p.display().to_string()).unwrap_or_else(|| {
                        "Not found — install CS:S through Steam, or select its folder.".into()
                    }),
                ),
                (
                    false,
                    format!(
                        "{} community maps, with timing zones",
                        surf_app::content::catalog().maps.len()
                    ),
                ),
                (
                    false,
                    "KSF rankings and available top-10 ghost replays".into(),
                ),
            ],
            active: 0,
            log: vec![
                "Existing maps are verified and reused. Downloads can be retried.".into(),
                "CS:S supplies local textures; surf-oss does not download Valve assets.".into(),
            ],
            running: false,
            failed: false,
            cursor: (0., 0.),
            scroll: 0,
            selected_button: 0,
            started: Instant::now(),
        })
    }
    pub fn begin(&mut self) {
        self.running = true;
        self.failed = false;
        self.active = 0;
        for step in &mut self.steps {
            step.0 = false;
        }
        self.log.clear();
        self.scroll = 0;
    }
    pub fn update(&mut self, event: SetupEvent) {
        match event {
            SetupEvent::Step {
                index,
                complete,
                detail,
            } => {
                self.active = index;
                self.steps[index] = (complete, detail.clone());
                self.log.push(detail);
            }
            SetupEvent::Activity(message) => {
                // Replace byte updates for a single transfer, keeping other activity.
                if message.starts_with("Downloading ")
                    && self
                        .log
                        .last()
                        .is_some_and(|s| s.split('·').next() == message.split('·').next())
                {
                    self.log.pop();
                }
                self.log.push(message);
            }
            SetupEvent::Finished(result) => {
                self.running = false;
                self.failed = result.is_err();
                self.log.push(result.err().unwrap_or_else(|| {
                    "Everything is ready. Close this window and choose Play.".into()
                }));
            }
        }
        self.scroll = 0;
        self.window.request_redraw();
    }
    pub fn page(&self) -> MenuPage {
        let titles = [
            "1. Counter-Strike: Source",
            "2. Maps and timing zones",
            "3. Leaderboards and replays",
        ];
        let mut rows = Vec::new();
        for (i, (done, detail)) in self.steps.iter().enumerate() {
            let (status, tone) = if *done {
                ("✓ Ready", RowTone::Good)
            } else if self.running && i == self.active {
                ("In progress", RowTone::Accent)
            } else if self.failed {
                ("Incomplete", RowTone::Warn)
            } else {
                ("Pending", RowTone::Dim)
            };
            rows.push(MenuRow::text(titles[i], status).with_tone(tone));
            for line in wrap(detail, 52) {
                rows.push(MenuRow::text(line, "").with_tone(RowTone::Dim));
            }
        }
        rows.push(MenuRow::header(
            "Activity · newest first (scroll for history)",
        ));
        let lines: Vec<_> = self.log.iter().rev().flat_map(|m| wrap(m, 52)).collect();
        for line in lines
            .iter()
            .skip(self.scroll.min(lines.len().saturating_sub(1)))
        {
            rows.push(MenuRow::text(line, ""));
        }
        MenuPage {
            title: if self.running {
                "Setting up surf-oss"
            } else if self.failed {
                "Setup needs attention"
            } else if self.steps.iter().all(|s| s.0) {
                "Ready to surf"
            } else {
                "Welcome to surf-oss"
            }
            .into(),
            subtitle: "Check your game files, then install the surf collection.".into(),
            panel: MenuPanel {
                rows,
                selected: usize::MAX,
                ..Default::default()
            },
            buttons: if self.running {
                vec!["Cancel setup".into(), "Hide window".into()]
            } else {
                vec![
                    if self.failed {
                        "Retry setup"
                    } else {
                        "Set up / refresh"
                    }
                    .into(),
                    "Done".into(),
                ]
            },
            button_selected: Some(self.selected_button),
            wide: true,
            backdrop: true,
            busy: self.running,
            message: Some(
                if self.running {
                    "You can hide this window while downloads continue."
                } else {
                    "Enter starts setup · Esc closes this window"
                }
                .into(),
            ),
            ..Default::default()
        }
    }
    pub fn button(&self) -> Option<usize> {
        let page = self.page();
        surf_render::layout_for(
            self.renderer.config.width as f32,
            self.renderer.config.height as f32,
            &page,
        )
        .buttons
        .iter()
        .position(|r| r.contains(self.cursor.0, self.cursor.1))
    }
    pub fn scroll_by(&mut self, older: bool) {
        let count = self.log.iter().map(|m| wrap(m, 52).len()).sum::<usize>();
        self.scroll = if older {
            (self.scroll + 1).min(count.saturating_sub(1))
        } else {
            self.scroll.saturating_sub(1)
        };
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.renderer.resize(width, height);
        self.surface
            .configure(&self.renderer.device, &self.renderer.config);
    }
    pub fn draw(&mut self) {
        let page = self.page();
        let camera = Camera::new(
            surf_core::math::Vec3::ZERO,
            surf_core::math::Angle::default(),
            1.,
        );
        let hud = HudState {
            page: Some(page),
            time: self.started.elapsed().as_secs_f32(),
            ..Default::default()
        };
        if let Err(error) = self.renderer.render(
            &self.surface,
            &camera,
            hud,
            None,
            None,
            ViewParams::new(1., 0.),
        ) {
            if matches!(
                error,
                wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated
            ) {
                self.surface
                    .configure(&self.renderer.device, &self.renderer.config);
            }
        }
    }
}
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            while line.chars().count() > width {
                let limit = line
                    .char_indices()
                    .nth(width)
                    .map(|(i, _)| i)
                    .unwrap_or(line.len());
                let cut = line[..limit]
                    .rfind('/')
                    .filter(|i| *i > 0)
                    .map(|i| i + 1)
                    .unwrap_or(limit);
                let rest = line.split_off(cut);
                lines.push(line);
                line = rest;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}
