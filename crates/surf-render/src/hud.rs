//! Modern HUD: glyphon text (JetBrains Mono) + speed/sync bars + Esc menu.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

const FONT_MEDIUM: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
const FONT_FAMILY: &str = "JetBrains Mono";

/// Timer phase for HUD labels (mirrors app timer without coupling crates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HudTimerPhase {
    #[default]
    Idle,
    Armed,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShowKeysState {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
}

/// One recent finish row for the pause menu.
#[derive(Clone, Debug, Default)]
pub struct MenuRecentEntry {
    pub time: String,
    pub is_pb: bool,
}

/// Settings / pause overlay content (preformatted by the app).
#[derive(Clone, Debug, Default)]
pub struct MenuHud {
    pub selected: usize,
    /// Rows as `(label, value)`. Value may be empty (e.g. Quit).
    pub items: Vec<(String, String)>,
    pub hint: String,
    /// Total finishes on this map (for the recent heading).
    pub recent_count: u32,
    pub recent: Vec<MenuRecentEntry>,
}

#[derive(Clone, Debug)]
pub struct HudState {
    /// Horizontal speed u/s.
    pub speed: f32,
    /// 0..100 air-strafe sync estimate.
    pub sync: f32,
    pub grounded: bool,
    /// Speed bar full-scale (display only; not physics cap).
    pub speed_scale: f32,
    /// Elapsed run time (seconds). `None` hides the timer line.
    pub time_secs: Option<f32>,
    /// Absolute PB for idle/armed display.
    pub pb_time_secs: Option<f32>,
    /// PB delta seconds (negative = ahead). Shown while running/finished.
    pub pb_delta_secs: Option<f32>,
    pub timer_phase: HudTimerPhase,
    pub show_sync_bar: bool,
    pub show_keys: Option<ShowKeysState>,
    /// Briefly show "PB!" after a new personal best.
    pub pb_flash: bool,
    /// Checkpoint split flash, e.g. `"CP2 12.340  -0.210"`.
    pub split_line: Option<String>,
    /// Live time delta vs PB ghost (negative = ahead). Shown while racing ghost.
    pub ghost_time_delta: Option<f32>,
    /// Live 2D speed delta vs PB ghost (positive = faster than ghost).
    pub ghost_speed_delta: Option<f32>,
    pub menu: Option<MenuHud>,
    /// Optional FPS / frame-time / resolution line (bottom-right).
    pub perf_line: Option<String>,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            speed: 0.0,
            sync: 0.0,
            grounded: false,
            speed_scale: 3500.0,
            time_secs: None,
            pb_time_secs: None,
            pb_delta_secs: None,
            timer_phase: HudTimerPhase::Idle,
            show_sync_bar: false,
            show_keys: None,
            pb_flash: false,
            split_line: None,
            ghost_time_delta: None,
            ghost_speed_delta: None,
            menu: None,
            perf_line: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HudVert {
    pos: [f32; 2],
    color: [f32; 4],
}

pub struct HudRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    speed_buf: Buffer,
    time_buf: Buffer,
    delta_buf: Buffer,
    phase_buf: Buffer,
    pb_abs_buf: Buffer,
    flash_buf: Buffer,
    split_buf: Buffer,
    ghost_buf: Buffer,
    keys_buf: Buffer,
    perf_buf: Buffer,
    menu_title_buf: Buffer,
    menu_bufs: Vec<Buffer>,
    menu_hint_buf: Buffer,
    recent_title_buf: Buffer,
    recent_bufs: Vec<Buffer>,
    bar_pipeline: wgpu::RenderPipeline,
    bar_vbo: wgpu::Buffer,
    bar_capacity: u32,
    bar_vert_count: u32,
}

impl HudRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(FONT_MEDIUM.to_vec());

        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        let speed_buf = Buffer::new(&mut font_system, Metrics::new(28.0, 34.0));
        let time_buf = Buffer::new(&mut font_system, Metrics::new(22.0, 26.0));
        let delta_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
        let phase_buf = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        let pb_abs_buf = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        let flash_buf = Buffer::new(&mut font_system, Metrics::new(32.0, 38.0));
        let split_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
        let ghost_buf = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        let keys_buf = Buffer::new(&mut font_system, Metrics::new(18.0, 22.0));
        let perf_buf = Buffer::new(&mut font_system, Metrics::new(13.0, 16.0));
        let menu_title_buf = Buffer::new(&mut font_system, Metrics::new(26.0, 32.0));
        // Must stay >= the app's menu row count; extra rows are silently
        // truncated rather than wrapping.
        let menu_bufs = (0..24)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(17.0, 22.0)))
            .collect();
        let menu_hint_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let recent_title_buf = Buffer::new(&mut font_system, Metrics::new(13.0, 17.0));
        let recent_bufs = (0..6)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(15.0, 20.0)))
            .collect();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud_bars"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}
@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4<f32>(v.pos, 0.0, 1.0);
    o.color = v.color;
    return o;
}
@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#
                .into(),
            ),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud_bar_layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let bar_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud_bar_pipe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVert>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let bar_capacity = 1024u32;
        let bar_vbo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud_bar_vbo"),
            size: (bar_capacity as u64) * std::mem::size_of::<HudVert>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            speed_buf,
            time_buf,
            delta_buf,
            phase_buf,
            pb_abs_buf,
            flash_buf,
            split_buf,
            ghost_buf,
            keys_buf,
            perf_buf,
            menu_title_buf,
            menu_bufs,
            menu_hint_buf,
            recent_title_buf,
            recent_bufs,
            bar_pipeline,
            bar_vbo,
            bar_capacity,
            bar_vert_count: 0,
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        hud: HudState,
    ) {
        self.viewport.update(
            queue,
            Resolution {
                width: width.max(1),
                height: height.max(1),
            },
        );

        let w = width.max(1) as f32;
        let h = height.max(1) as f32;
        let attrs = Attrs::new()
            .family(Family::Name(FONT_FAMILY))
            .weight(Weight::MEDIUM);

        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };

        // --- gameplay HUD text ---
        let speed_label = format!("{:.0}", hud.speed);
        set_buf_text(
            &mut self.font_system,
            &mut self.speed_buf,
            &speed_label,
            attrs,
            w,
            40.0,
        );

        let time_label = hud.time_secs.map(format_hud_time);
        if let Some(ref t) = time_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.time_buf,
                t,
                attrs,
                w,
                30.0,
            );
        }

        let phase_label = match hud.timer_phase {
            HudTimerPhase::Armed => Some("START"),
            HudTimerPhase::Finished => Some("FINISH"),
            _ => None,
        };
        if let Some(p) = phase_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.phase_buf,
                p,
                attrs,
                w,
                22.0,
            );
        }

        let show_pb_abs = matches!(
            hud.timer_phase,
            HudTimerPhase::Idle | HudTimerPhase::Armed
        ) && hud.pb_time_secs.is_some();
        let pb_abs_label = if show_pb_abs {
            hud.pb_time_secs
                .map(|t| format!("PB {}", format_hud_time(t)))
        } else {
            None
        };
        if let Some(ref p) = pb_abs_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.pb_abs_buf,
                p,
                attrs,
                w,
                22.0,
            );
        }

        let show_delta = matches!(
            hud.timer_phase,
            HudTimerPhase::Running | HudTimerPhase::Finished
        ) && hud.pb_delta_secs.is_some();
        let delta_label = if show_delta {
            hud.pb_delta_secs.map(format_hud_delta)
        } else {
            None
        };
        if let Some(ref d) = delta_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.delta_buf,
                d,
                attrs,
                w,
                24.0,
            );
        }

        if hud.pb_flash {
            set_buf_text(
                &mut self.font_system,
                &mut self.flash_buf,
                "PB!",
                attrs,
                w,
                44.0,
            );
        }

        if let Some(ref s) = hud.split_line {
            set_buf_text(
                &mut self.font_system,
                &mut self.split_buf,
                s,
                attrs,
                w,
                24.0,
            );
        }

        let ghost_label = match (hud.ghost_time_delta, hud.ghost_speed_delta) {
            (Some(td), Some(sd)) => Some(format!(
                "GHOST {}  {:+.0} u/s",
                format_hud_delta(td),
                sd
            )),
            (Some(td), None) => Some(format!("GHOST {}", format_hud_delta(td))),
            _ => None,
        };
        if let Some(ref g) = ghost_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.ghost_buf,
                g,
                attrs,
                w,
                22.0,
            );
        }

        let keys_label = hud.show_keys.map(format_showkeys);
        if let Some(ref k) = keys_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.keys_buf,
                k,
                attrs,
                w,
                28.0,
            );
        }

        if let Some(ref line) = hud.perf_line {
            set_buf_text(
                &mut self.font_system,
                &mut self.perf_buf,
                line,
                attrs,
                w,
                20.0,
            );
        }

        // Pause menu text
        let menu_open = hud.menu.is_some();
        let mut menu_item_count = 0usize;
        let mut recent_count = 0usize;
        if let Some(ref menu) = hud.menu {
            set_buf_text(
                &mut self.font_system,
                &mut self.menu_title_buf,
                "PAUSED",
                attrs,
                w,
                36.0,
            );
            menu_item_count = menu.items.len().min(self.menu_bufs.len());
            for i in 0..menu_item_count {
                let (ref label, ref value) = menu.items[i];
                let line = if value.is_empty() {
                    label.clone()
                } else {
                    format!("{label:<18}{value:>8}")
                };
                set_buf_text(
                    &mut self.font_system,
                    &mut self.menu_bufs[i],
                    &line,
                    attrs,
                    w,
                    26.0,
                );
            }
            set_buf_text(
                &mut self.font_system,
                &mut self.menu_hint_buf,
                &menu.hint,
                attrs,
                w,
                18.0,
            );
            let recent_heading = if menu.recent_count == 0 {
                "RECENT".into()
            } else {
                format!("RECENT  ·  {} finishes", menu.recent_count)
            };
            set_buf_text(
                &mut self.font_system,
                &mut self.recent_title_buf,
                &recent_heading,
                attrs,
                w,
                20.0,
            );
            if menu.recent.is_empty() {
                recent_count = 1;
                set_buf_text(
                    &mut self.font_system,
                    &mut self.recent_bufs[0],
                    "no finishes yet",
                    attrs,
                    w,
                    22.0,
                );
            } else {
                recent_count = menu.recent.len().min(self.recent_bufs.len());
                for i in 0..recent_count {
                    let e = &menu.recent[i];
                    let pb = if e.is_pb { "  PB" } else { "" };
                    let line = format!("#{:<2}  {}{pb}", i + 1, e.time);
                    set_buf_text(
                        &mut self.font_system,
                        &mut self.recent_bufs[i],
                        &line,
                        attrs,
                        w,
                        22.0,
                    );
                }
            }
        }

        let speed_color = if hud.grounded {
            Color::rgb(245, 245, 247)
        } else {
            Color::rgb(140, 210, 255)
        };
        let time_color = if hud.timer_phase == HudTimerPhase::Finished {
            Color::rgb(255, 210, 90)
        } else {
            Color::rgb(230, 232, 238)
        };
        let phase_color = Color::rgb(150, 155, 165);
        let pb_abs_color = Color::rgb(170, 175, 185);
        let delta_color = match hud.pb_delta_secs {
            Some(d) if d < 0.0 => Color::rgb(110, 230, 140),
            Some(d) if d > 0.0 => Color::rgb(240, 110, 110),
            _ => Color::rgb(180, 180, 190),
        };
        let flash_color = Color::rgb(255, 220, 100);
        let split_color = match hud.pb_delta_secs {
            // Color split flash by its own delta if we embedded sign in the string —
            // use ghost-style: green when line contains " -", else red/neutral.
            _ => {
                if let Some(ref s) = hud.split_line {
                    if s.contains(" -") {
                        Color::rgb(110, 230, 140)
                    } else if s.contains(" +") {
                        Color::rgb(240, 110, 110)
                    } else {
                        Color::rgb(210, 215, 225)
                    }
                } else {
                    Color::rgb(210, 215, 225)
                }
            }
        };
        let ghost_color = match hud.ghost_time_delta {
            Some(d) if d < 0.0 => Color::rgb(110, 230, 140),
            Some(d) if d > 0.0 => Color::rgb(240, 110, 110),
            _ => Color::rgb(160, 200, 220),
        };
        let keys_color = Color::rgb(200, 205, 215);
        let perf_color = Color::rgb(160, 165, 175);
        let menu_title_color = Color::rgb(245, 246, 250);
        let menu_sel = Color::rgb(255, 220, 130);
        let menu_norm = Color::rgb(200, 204, 214);
        let menu_hint_color = Color::rgb(120, 126, 138);
        let recent_title_color = Color::rgb(140, 146, 160);
        let recent_norm = Color::rgb(190, 196, 208);
        let recent_pb = Color::rgb(255, 210, 110);

        let top_pad = (h * 0.06).max(28.0);
        let mut areas: Vec<TextArea> = Vec::with_capacity(24);

        // Menu layout (pixel space) — also drives panel / selection quads.
        let panel_w = 440.0_f32.min(w - 48.0).max(320.0);
        let panel_x = (w - panel_w) * 0.5;
        let panel_pad_x = 28.0;
        // Tighten rows once the list is long so a taller menu still fits above
        // the recent-runs footer on small windows. Stays clear of the 22 px
        // line height, so rows never overlap.
        let row_h = if menu_item_count > 14 { 25.0 } else { 30.0 };
        let title_h = 36.0;
        let hint_h = 18.0;
        let recent_row_h = 22.0;
        let menu_title_y = (h * 0.16).max(48.0);
        let items_y0 = menu_title_y + title_h + 18.0;
        let hint_y = items_y0 + menu_item_count as f32 * row_h + 10.0;
        let recent_title_y = hint_y + hint_h + 22.0;
        let recent_y0 = recent_title_y + 22.0;
        let panel_y = menu_title_y - 28.0;
        let panel_bottom = if recent_count > 0 {
            recent_y0 + recent_count as f32 * recent_row_h + 28.0
        } else {
            recent_title_y + 36.0
        };
        let panel_h = (panel_bottom - panel_y).max(200.0);
        let text_left = panel_x + panel_pad_x;

        if !menu_open {
            let speed_w = line_width(&self.speed_buf);
            areas.push(TextArea {
                buffer: &self.speed_buf,
                left: (w - speed_w) * 0.5,
                top: top_pad,
                scale: 1.0,
                bounds,
                default_color: speed_color,
                custom_glyphs: &[],
            });

            let mut y = top_pad + 32.0;
            if let Some(p) = phase_label {
                let _ = p;
                let pw = line_width(&self.phase_buf);
                areas.push(TextArea {
                    buffer: &self.phase_buf,
                    left: (w - pw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: phase_color,
                    custom_glyphs: &[],
                });
                y += 18.0;
            }
            if time_label.is_some() {
                let tw = line_width(&self.time_buf);
                areas.push(TextArea {
                    buffer: &self.time_buf,
                    left: (w - tw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: time_color,
                    custom_glyphs: &[],
                });
                y += 26.0;
            }
            if pb_abs_label.is_some() {
                let pw = line_width(&self.pb_abs_buf);
                areas.push(TextArea {
                    buffer: &self.pb_abs_buf,
                    left: (w - pw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: pb_abs_color,
                    custom_glyphs: &[],
                });
                y += 18.0;
            }
            if delta_label.is_some() {
                let dw = line_width(&self.delta_buf);
                areas.push(TextArea {
                    buffer: &self.delta_buf,
                    left: (w - dw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: delta_color,
                    custom_glyphs: &[],
                });
                y += 20.0;
            }
            if ghost_label.is_some() {
                let gw = line_width(&self.ghost_buf);
                areas.push(TextArea {
                    buffer: &self.ghost_buf,
                    left: (w - gw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: ghost_color,
                    custom_glyphs: &[],
                });
                y += 18.0;
            }
            if hud.split_line.is_some() {
                let sw = line_width(&self.split_buf);
                areas.push(TextArea {
                    buffer: &self.split_buf,
                    left: (w - sw) * 0.5,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: split_color,
                    custom_glyphs: &[],
                });
                y += 20.0;
            }
            if hud.pb_flash {
                let fw = line_width(&self.flash_buf);
                areas.push(TextArea {
                    buffer: &self.flash_buf,
                    left: (w - fw) * 0.5,
                    top: y + 8.0,
                    scale: 1.0,
                    bounds,
                    default_color: flash_color,
                    custom_glyphs: &[],
                });
            }
            if keys_label.is_some() {
                let kw = line_width(&self.keys_buf);
                areas.push(TextArea {
                    buffer: &self.keys_buf,
                    left: (w - kw) * 0.5,
                    top: h - 48.0,
                    scale: 1.0,
                    bounds,
                    default_color: keys_color,
                    custom_glyphs: &[],
                });
            }
        }

        // Perf line stays visible over the pause menu too.
        if hud.perf_line.is_some() {
            let pw = line_width(&self.perf_buf);
            areas.push(TextArea {
                buffer: &self.perf_buf,
                left: (w - pw - 16.0).max(8.0),
                top: h - 28.0,
                scale: 1.0,
                bounds,
                default_color: perf_color,
                custom_glyphs: &[],
            });
        }

        if let Some(ref menu) = hud.menu {
            let tw = line_width(&self.menu_title_buf);
            areas.push(TextArea {
                buffer: &self.menu_title_buf,
                left: panel_x + (panel_w - tw) * 0.5,
                top: menu_title_y,
                scale: 1.0,
                bounds,
                default_color: menu_title_color,
                custom_glyphs: &[],
            });
            for i in 0..menu_item_count {
                let color = if i == menu.selected {
                    menu_sel
                } else {
                    menu_norm
                };
                areas.push(TextArea {
                    buffer: &self.menu_bufs[i],
                    left: text_left,
                    top: items_y0 + i as f32 * row_h + 4.0,
                    scale: 1.0,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
            }
            let hw = line_width(&self.menu_hint_buf);
            areas.push(TextArea {
                buffer: &self.menu_hint_buf,
                left: panel_x + (panel_w - hw) * 0.5,
                top: hint_y,
                scale: 1.0,
                bounds,
                default_color: menu_hint_color,
                custom_glyphs: &[],
            });
            areas.push(TextArea {
                buffer: &self.recent_title_buf,
                left: text_left,
                top: recent_title_y,
                scale: 1.0,
                bounds,
                default_color: recent_title_color,
                custom_glyphs: &[],
            });
            for i in 0..recent_count {
                let color = if menu.recent.get(i).is_some_and(|e| e.is_pb) {
                    recent_pb
                } else if menu.recent.is_empty() {
                    menu_hint_color
                } else {
                    recent_norm
                };
                areas.push(TextArea {
                    buffer: &self.recent_bufs[i],
                    left: text_left,
                    top: recent_y0 + i as f32 * recent_row_h,
                    scale: 1.0,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
            }
        }

        // Safety: glyphon needs buffers that were set; empty areas ok.
        let _ = self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        );

        let mut verts: Vec<HudVert> = Vec::new();
        if menu_open {
            if let Some(ref menu) = hud.menu {
                // Soft full-screen dim.
                push_rect_ndc(
                    &mut verts,
                    -1.0,
                    -1.0,
                    2.0,
                    2.0,
                    [0.02, 0.02, 0.04, 0.62],
                );
                // Panel fill + thin top accent.
                push_rect_px(
                    &mut verts,
                    panel_x,
                    panel_y,
                    panel_w,
                    panel_h,
                    w,
                    h,
                    [0.09, 0.095, 0.12, 0.94],
                );
                push_rect_px(
                    &mut verts,
                    panel_x,
                    panel_y,
                    panel_w,
                    2.0,
                    w,
                    h,
                    [0.95, 0.78, 0.32, 0.95],
                );
                // Selected row highlight + left tick.
                let sel_y = items_y0 + menu.selected as f32 * row_h;
                push_rect_px(
                    &mut verts,
                    panel_x + 10.0,
                    sel_y,
                    panel_w - 20.0,
                    row_h,
                    w,
                    h,
                    [1.0, 0.88, 0.40, 0.10],
                );
                push_rect_px(
                    &mut verts,
                    panel_x + 10.0,
                    sel_y + 5.0,
                    3.0,
                    row_h - 10.0,
                    w,
                    h,
                    [1.0, 0.86, 0.35, 0.95],
                );
                // Hairline above recent section.
                let sep_y = hint_y + hint_h + 12.0;
                push_rect_px(
                    &mut verts,
                    panel_x + panel_pad_x,
                    sep_y,
                    panel_w - panel_pad_x * 2.0,
                    1.0,
                    w,
                    h,
                    [1.0, 1.0, 1.0, 0.08],
                );
            }
        } else {
            push_rect_ndc(
                &mut verts,
                -0.96,
                -0.94,
                0.42,
                0.012,
                [0.12, 0.12, 0.14, 0.85],
            );
            let speed_t = (hud.speed / hud.speed_scale.max(1.0)).clamp(0.0, 1.0);
            let speed_bar = if hud.grounded {
                [0.40, 0.78, 0.55, 0.90]
            } else {
                [0.35, 0.70, 0.95, 0.90]
            };
            push_rect_ndc(
                &mut verts,
                -0.96,
                -0.94,
                0.42 * speed_t,
                0.012,
                speed_bar,
            );
            if hud.show_sync_bar {
                push_rect_ndc(
                    &mut verts,
                    -0.96,
                    -0.91,
                    0.42,
                    0.008,
                    [0.12, 0.12, 0.14, 0.85],
                );
                let sync_t = (hud.sync / 100.0).clamp(0.0, 1.0);
                push_rect_ndc(
                    &mut verts,
                    -0.96,
                    -0.91,
                    0.42 * sync_t,
                    0.008,
                    [0.92, 0.72, 0.28, 0.90],
                );
            }
        }

        let count = verts.len().min(self.bar_capacity as usize) as u32;
        if count > 0 {
            queue.write_buffer(
                &self.bar_vbo,
                0,
                bytemuck::cast_slice(&verts[..count as usize]),
            );
        }
        self.bar_vert_count = count;
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        // Quads first (dim/panel/bars), then text on top.
        if self.bar_vert_count > 0 {
            pass.set_pipeline(&self.bar_pipeline);
            pass.set_vertex_buffer(0, self.bar_vbo.slice(..));
            pass.draw(0..self.bar_vert_count, 0..1);
        }
        let _ = self
            .text_renderer
            .render(&self.atlas, &self.viewport, pass);
    }

    pub fn trim(&mut self) {
        self.atlas.trim();
    }
}

fn set_buf_text(
    font_system: &mut FontSystem,
    buf: &mut Buffer,
    text: &str,
    attrs: Attrs,
    w: f32,
    h: f32,
) {
    buf.set_size(font_system, Some(w), Some(h));
    buf.set_text(font_system, text, attrs, Shaping::Advanced);
    buf.shape_until_scroll(font_system, false);
}

fn format_showkeys(k: ShowKeysState) -> String {
    let f = if k.forward { "W" } else { "·" };
    let a = if k.left { "A" } else { "·" };
    let s = if k.back { "S" } else { "·" };
    let d = if k.right { "D" } else { "·" };
    let j = if k.jump { "JMP" } else { "···" };
    format!("{f}  {a}{s}{d}  {j}")
}

fn line_width(buf: &Buffer) -> f32 {
    buf.layout_runs()
        .map(|run| run.line_w)
        .fold(0.0f32, f32::max)
}

fn push_rect_ndc(out: &mut Vec<HudVert>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    let (x0, y0, x1, y1) = (x, y, x + w, y + h);
    for p in [
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y0],
        [x1, y1],
        [x0, y1],
    ] {
        out.push(HudVert { pos: p, color });
    }
}

/// Axis-aligned rect in top-left pixel space → clip-space verts (Y up).
fn push_rect_px(
    out: &mut Vec<HudVert>,
    x: f32,
    y: f32,
    rw: f32,
    rh: f32,
    screen_w: f32,
    screen_h: f32,
    color: [f32; 4],
) {
    let x0 = (x / screen_w) * 2.0 - 1.0;
    let x1 = ((x + rw) / screen_w) * 2.0 - 1.0;
    let y1 = 1.0 - (y / screen_h) * 2.0;
    let y0 = 1.0 - ((y + rh) / screen_h) * 2.0;
    for p in [
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y0],
        [x1, y1],
        [x0, y1],
    ] {
        out.push(HudVert { pos: p, color });
    }
}

fn format_hud_time(secs: f32) -> String {
    let ms_total = (secs.max(0.0) * 1000.0).round() as u32;
    let millis = ms_total % 1000;
    let total_secs = ms_total / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if minutes > 0 {
        format!("{minutes}:{seconds:02}.{millis:03}")
    } else {
        format!("{seconds}.{millis:03}")
    }
}

fn format_hud_delta(delta: f32) -> String {
    let sign = if delta < 0.0 { '-' } else { '+' };
    format!("{sign}{}", format_hud_time(delta.abs()))
}
