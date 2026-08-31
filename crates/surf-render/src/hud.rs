//! Modern HUD: glyphon text (JetBrains Mono), centered speed + info box,
//! and the Esc menu.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

const FONT_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
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

/// One pause-overlay panel (settings on the left, locs on the right).
#[derive(Clone, Debug, Default)]
pub struct MenuPanel {
    /// Heading — names the box.
    pub title: String,
    /// Rows as `(label, value)`. Value may be empty (e.g. Quit).
    pub items: Vec<(String, String)>,
    pub hint: String,
    pub selected: usize,
    /// Row under the cursor, if any.
    pub hovered: Option<usize>,
    /// Panel owns the keyboard. Only the focused panel draws a hard selection.
    pub focused: bool,
}

/// Settings / pause overlay content (preformatted by the app). Both panels are
/// always drawn — the loc list is a peer of the settings list, not a sub-page.
#[derive(Clone, Debug, Default)]
pub struct MenuHud {
    pub main: MenuPanel,
    pub locs: MenuPanel,
    /// Total finishes on this map (for the recent heading, under the settings).
    pub recent_count: u32,
    pub recent: Vec<MenuRecentEntry>,
}

/// Pixel geometry of one panel. The renderer draws from this and the app
/// hit-tests against it, so a click always lands on the row you can see.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelLayout {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Top of the first row.
    pub items_y0: f32,
    pub row_h: f32,
    pub rows: usize,
}

impl PanelLayout {
    /// Row index under a cursor position, if it is over one.
    pub fn row_at(&self, mx: f32, my: f32) -> Option<usize> {
        if self.rows == 0 || self.row_h <= 0.0 {
            return None;
        }
        if mx < self.x || mx > self.x + self.w {
            return None;
        }
        if my < self.items_y0 {
            return None;
        }
        let row = ((my - self.items_y0) / self.row_h).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        if row < self.rows {
            Some(row)
        } else {
            None
        }
    }

    pub fn contains(&self, mx: f32, my: f32) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h
    }
}

/// Geometry for the whole pause overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MenuLayout {
    pub main: PanelLayout,
    pub locs: PanelLayout,
}

const PANEL_GAP: f32 = 22.0;
const PANEL_TITLE_H: f32 = 36.0;
const PANEL_HINT_H: f32 = 18.0;
const PANEL_PAD_X: f32 = 28.0;
const RECENT_ROW_H: f32 = 22.0;

fn panel_row_h(rows: usize) -> f32 {
    // Tighten rows once a list is long so a taller panel still fits on small
    // windows. Stays clear of the 22 px line height, so rows never overlap.
    if rows > 14 {
        25.0
    } else {
        30.0
    }
}

/// Lay out both panels side by side, centred as a pair. Pure — the app calls
/// this to hit-test the exact rows the renderer drew.
pub fn menu_layout(
    w: f32,
    h: f32,
    main_rows: usize,
    loc_rows: usize,
    recent_rows: usize,
) -> MenuLayout {
    let want_main = 440.0_f32;
    let want_locs = 360.0_f32;
    let available = (w - 48.0).max(280.0);
    let total = want_main + want_locs + PANEL_GAP;
    // Squeeze both panels proportionally rather than letting either fall off.
    let scale = if total > available {
        available / total
    } else {
        1.0
    };
    let main_w = (want_main * scale).max(200.0);
    let locs_w = (want_locs * scale).max(160.0);
    let group_w = main_w + locs_w + PANEL_GAP * scale;
    let group_x = ((w - group_w) * 0.5).max(12.0);

    let title_y = (h * 0.16).max(48.0);
    let panel_y = title_y - 28.0;
    let items_y0 = title_y + PANEL_TITLE_H + 18.0;

    let main_row_h = panel_row_h(main_rows);
    let main_hint_y = items_y0 + main_rows as f32 * main_row_h + 10.0;
    // Recent footer hangs under the settings panel only.
    let recent_title_y = main_hint_y + PANEL_HINT_H + 22.0;
    let recent_y0 = recent_title_y + 22.0;
    let main_bottom = if recent_rows > 0 {
        recent_y0 + recent_rows as f32 * RECENT_ROW_H + 28.0
    } else {
        recent_title_y + 36.0
    };

    let locs_row_h = panel_row_h(loc_rows);
    let locs_hint_y = items_y0 + loc_rows as f32 * locs_row_h + 10.0;
    let locs_bottom = locs_hint_y + PANEL_HINT_H + 26.0;

    MenuLayout {
        main: PanelLayout {
            x: group_x,
            y: panel_y,
            w: main_w,
            h: (main_bottom - panel_y).max(200.0),
            items_y0,
            row_h: main_row_h,
            rows: main_rows,
        },
        locs: PanelLayout {
            x: group_x + main_w + PANEL_GAP * scale,
            y: panel_y,
            w: locs_w,
            h: (locs_bottom - panel_y).max(160.0),
            items_y0,
            row_h: locs_row_h,
            rows: loc_rows,
        },
    }
}

/// A full-screen shell page: title screen, map picker, or loading. One centred
/// panel, unlike the pause overlay's side-by-side pair.
#[derive(Clone, Debug, Default)]
pub struct ShellHud {
    pub title: String,
    pub subtitle: String,
    /// `(label, value)` rows — value is a PB time, a tag, or empty.
    pub items: Vec<(String, String)>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub hint: String,
    /// Status or error line under the list.
    pub message: Option<String>,
    /// Rows are drawn from here; the app scrolls long lists itself so what it
    /// hit-tests and what is drawn stay the same slice.
    pub scroll: usize,
}

/// Most shell rows on screen at once. A longer map list scrolls.
pub const SHELL_ROWS_VISIBLE: usize = 16;

/// Geometry for a shell page. Pure, like [`menu_layout`], so the app hit-tests
/// exactly the rows the renderer drew.
pub fn shell_layout(w: f32, h: f32, rows: usize) -> PanelLayout {
    let pw = (w * 0.46).clamp(320.0, 560.0);
    let x = ((w - pw) * 0.5).max(12.0);
    let row_h = panel_row_h(rows);
    let y = (h * 0.13).max(36.0);
    let items_y0 = y + PANEL_TITLE_H + 74.0;
    let bottom = items_y0 + rows as f32 * row_h + 84.0;
    PanelLayout {
        x,
        y,
        w: pw,
        h: (bottom - y).max(200.0),
        items_y0,
        row_h,
        rows,
    }
}

#[derive(Clone, Debug)]
pub struct HudState {
    /// Horizontal speed u/s.
    pub speed: f32,
    /// 0..100 air-strafe sync estimate.
    pub sync: f32,
    pub grounded: bool,
    /// Retained for callers; the speed bar it scaled is gone.
    pub speed_scale: f32,
    /// Elapsed run time (seconds). `None` hides the timer line.
    pub time_secs: Option<f32>,
    /// Absolute PB for idle/armed display.
    pub pb_time_secs: Option<f32>,
    /// PB delta seconds (negative = ahead). Shown while running/finished.
    pub pb_delta_secs: Option<f32>,
    pub timer_phase: HudTimerPhase,
    /// Show the sync percentage as a row in the info box.
    pub show_sync_bar: bool,
    pub show_keys: Option<ShowKeysState>,
    /// Briefly show "PB!" after a new personal best.
    pub pb_flash: bool,
    /// Checkpoint split flash, e.g. `"CP2 12.340  -0.210"`.
    pub split_line: Option<String>,
    /// Staged maps: `"STAGE 2/5"` while armed/running/finished.
    pub stage_line: Option<String>,
    /// Run was resumed from a saved loc: the clock is real, the run is not.
    /// Folded into the phase label so a practice time can never be misread.
    pub practice: bool,
    /// Practice mode armed — loadloc is unlocked. Shown so it's always visible
    /// whether a stray click can yank you out of a run.
    pub practice_mode: bool,
    /// Live time delta vs PB ghost (negative = ahead). Shown while racing ghost.
    pub ghost_time_delta: Option<f32>,
    /// Live 2D speed delta vs PB ghost (positive = faster than ghost).
    pub ghost_speed_delta: Option<f32>,
    pub menu: Option<MenuHud>,
    /// Title screen / map picker / loading page. Takes over the whole frame.
    pub shell: Option<ShellHud>,
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
            practice: false,
            practice_mode: false,
            pb_flash: false,
            split_line: None,
            stage_line: None,
            ghost_time_delta: None,
            ghost_speed_delta: None,
            menu: None,
            shell: None,
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
    sync_buf: Buffer,
    keys_buf: Buffer,
    perf_buf: Buffer,
    menu_title_buf: Buffer,
    menu_bufs: Vec<Buffer>,
    menu_hint_buf: Buffer,
    locs_title_buf: Buffer,
    locs_bufs: Vec<Buffer>,
    locs_hint_buf: Buffer,
    shell_title_buf: Buffer,
    shell_sub_buf: Buffer,
    shell_bufs: Vec<Buffer>,
    shell_hint_buf: Buffer,
    shell_msg_buf: Buffer,
    recent_title_buf: Buffer,
    recent_bufs: Vec<Buffer>,
    bar_pipeline: wgpu::RenderPipeline,
    bar_vbo: wgpu::Buffer,
    bar_capacity: u32,
    bar_vert_count: u32,
}

impl HudRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(FONT_MEDIUM.to_vec());

        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        // Speed is the headline read (reference HUD: large, centered, above
        // the info box) — everything else is deliberately quieter.
        let speed_buf = Buffer::new(&mut font_system, Metrics::new(58.0, 66.0));
        let time_buf = Buffer::new(&mut font_system, Metrics::new(20.0, 26.0));
        let delta_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
        let phase_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let pb_abs_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let flash_buf = Buffer::new(&mut font_system, Metrics::new(32.0, 38.0));
        let split_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 21.0));
        let ghost_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let sync_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let keys_buf = Buffer::new(&mut font_system, Metrics::new(18.0, 22.0));
        let perf_buf = Buffer::new(&mut font_system, Metrics::new(13.0, 16.0));
        let menu_title_buf = Buffer::new(&mut font_system, Metrics::new(26.0, 32.0));
        // Must stay >= the app's menu row count; extra rows are silently
        // truncated rather than wrapping.
        let menu_bufs = (0..24)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(17.0, 22.0)))
            .collect();
        let menu_hint_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let locs_title_buf = Buffer::new(&mut font_system, Metrics::new(26.0, 32.0));
        let locs_bufs = (0..20)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(17.0, 22.0)))
            .collect();
        let locs_hint_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let shell_title_buf = Buffer::new(&mut font_system, Metrics::new(38.0, 44.0));
        let shell_sub_buf = Buffer::new(&mut font_system, Metrics::new(14.0, 19.0));
        let shell_bufs: Vec<Buffer> = (0..SHELL_ROWS_VISIBLE)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(18.0, 23.0)))
            .collect();
        let shell_hint_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let shell_msg_buf = Buffer::new(&mut font_system, Metrics::new(14.0, 19.0));
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
            sync_buf,
            keys_buf,
            perf_buf,
            menu_title_buf,
            locs_title_buf,
            locs_bufs,
            locs_hint_buf,
            menu_bufs,
            menu_hint_buf,
            shell_title_buf,
            shell_sub_buf,
            shell_bufs,
            shell_hint_buf,
            shell_msg_buf,
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
            80.0,
        );

        let time_label = hud
            .time_secs
            .map(|t| format!("Time: {}", format_hud_time(t)));
        if let Some(ref t) = time_label {
            set_buf_text(&mut self.font_system, &mut self.time_buf, t, attrs, w, 30.0);
        }

        let base_phase_label = match hud.timer_phase {
            HudTimerPhase::Finished => Some("FINISH"),
            HudTimerPhase::Armed => hud.stage_line.as_deref().or(Some("START")),
            HudTimerPhase::Running => hud.stage_line.as_deref(),
            HudTimerPhase::Idle => None,
        };
        // Both states always show, even on Idle/Running where there'd be no
        // label: a loaded run must never be mistaken for a clean one, and you
        // must be able to see at a glance whether Mouse1 is armed.
        let practice_tag = if hud.practice {
            Some("PRACTICE")
        } else if hud.practice_mode {
            Some("PRACTICE MODE")
        } else {
            None
        };
        let phase_label: Option<String> = match (base_phase_label, practice_tag) {
            (Some(p), Some(tag)) => Some(format!("{p} · {tag}")),
            (Some(p), None) => Some(p.to_string()),
            (None, Some(tag)) => Some(tag.to_string()),
            (None, None) => None,
        };
        if let Some(ref p) = phase_label {
            set_buf_text(
                &mut self.font_system,
                &mut self.phase_buf,
                p,
                attrs,
                w,
                22.0,
            );
        }

        let show_pb_abs = matches!(hud.timer_phase, HudTimerPhase::Idle | HudTimerPhase::Armed)
            && hud.pb_time_secs.is_some();
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
            (Some(td), Some(sd)) => Some(format!("GHOST {}  {:+.0} u/s", format_hud_delta(td), sd)),
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

        // The sync bar is gone; the toggle now drives a line in the info box.
        let sync_label = if hud.show_sync_bar {
            Some(format!("Sync: {:.0}%", hud.sync.clamp(0.0, 100.0)))
        } else {
            None
        };
        if let Some(ref s) = sync_label {
            set_buf_text(&mut self.font_system, &mut self.sync_buf, s, attrs, w, 22.0);
        }

        let keys_label = hud.show_keys.map(format_showkeys);
        if let Some(ref k) = keys_label {
            set_buf_text(&mut self.font_system, &mut self.keys_buf, k, attrs, w, 28.0);
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

        // Shell page text (title screen / map picker / loading).
        let shell_open = hud.shell.is_some();
        let mut shell_item_count = 0usize;
        if let Some(ref sh) = hud.shell {
            set_buf_text(&mut self.font_system, &mut self.shell_title_buf, &sh.title, attrs, w, 44.0);
            set_buf_text(&mut self.font_system, &mut self.shell_sub_buf, &sh.subtitle, attrs, w, 19.0);
            let start = sh.scroll.min(sh.items.len());
            let end = (start + self.shell_bufs.len()).min(sh.items.len());
            shell_item_count = end - start;
            for i in 0..shell_item_count {
                let (ref label, ref value) = sh.items[start + i];
                let line = if value.is_empty() {
                    label.clone()
                } else {
                    format!("{label:<20}{value:>10}")
                };
                set_buf_text(&mut self.font_system, &mut self.shell_bufs[i], &line, attrs, w, 23.0);
            }
            set_buf_text(&mut self.font_system, &mut self.shell_hint_buf, &sh.hint, attrs, w, 16.0);
            set_buf_text(
                &mut self.font_system,
                &mut self.shell_msg_buf,
                sh.message.as_deref().unwrap_or(""),
                attrs,
                w,
                19.0,
            );
        }

        // Pause menu text
        let menu_open = hud.menu.is_some();
        let mut menu_item_count = 0usize;
        let mut locs_item_count = 0usize;
        let mut recent_count = 0usize;
        if let Some(ref menu) = hud.menu {
            set_buf_text(
                &mut self.font_system,
                &mut self.menu_title_buf,
                &menu.main.title,
                attrs,
                w,
                36.0,
            );
            menu_item_count = menu.main.items.len().min(self.menu_bufs.len());
            for i in 0..menu_item_count {
                let (ref label, ref value) = menu.main.items[i];
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
                &menu.main.hint,
                attrs,
                w,
                18.0,
            );

            set_buf_text(
                &mut self.font_system,
                &mut self.locs_title_buf,
                &menu.locs.title,
                attrs,
                w,
                36.0,
            );
            locs_item_count = menu.locs.items.len().min(self.locs_bufs.len());
            for i in 0..locs_item_count {
                let (ref label, ref value) = menu.locs.items[i];
                let line = if value.is_empty() {
                    label.clone()
                } else {
                    format!("{label:<14}{value:>10}")
                };
                set_buf_text(
                    &mut self.font_system,
                    &mut self.locs_bufs[i],
                    &line,
                    attrs,
                    w,
                    26.0,
                );
            }
            set_buf_text(
                &mut self.font_system,
                &mut self.locs_hint_buf,
                &menu.locs.hint,
                attrs,
                w,
                18.0,
            );
            {
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
        }

        let speed_color = if hud.grounded {
            Color::rgb(245, 245, 247)
        } else {
            Color::rgb(140, 210, 255)
        };
        let time_color = if hud.timer_phase == HudTimerPhase::Finished && !hud.practice {
            Color::rgb(255, 210, 90)
        } else {
            Color::rgb(230, 232, 238)
        };
        let phase_color = if hud.practice {
            Color::rgb(255, 170, 80)
        } else if hud.practice_mode {
            // Armed but not yet tainted — present, not alarming.
            Color::rgb(190, 165, 120)
        } else {
            Color::rgb(150, 155, 165)
        };
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
        let sync_color = Color::rgb(200, 185, 140);
        let keys_color = Color::rgb(200, 205, 215);
        let perf_color = Color::rgb(160, 165, 175);
        let menu_title_color = Color::rgb(245, 246, 250);
        let menu_sel = Color::rgb(255, 220, 130);
        let menu_sel_dim = Color::rgb(198, 176, 122);
        let menu_hover = Color::rgb(235, 238, 246);
        let menu_norm = Color::rgb(200, 204, 214);
        let menu_hint_color = Color::rgb(120, 126, 138);
        let recent_title_color = Color::rgb(140, 146, 160);
        let recent_norm = Color::rgb(190, 196, 208);
        let recent_pb = Color::rgb(255, 210, 110);

        let mut areas: Vec<TextArea> = Vec::with_capacity(24);

        // Menu layout (pixel space) — the same function the app hit-tests with,
        // so a click always lands on the row that was drawn.
        let layout = menu_layout(w, h, menu_item_count, locs_item_count, recent_count);
        let main_p = layout.main;
        let locs_p = layout.locs;
        let panel_pad_x = PANEL_PAD_X;
        let row_h = main_p.row_h;
        let hint_h = PANEL_HINT_H;
        let recent_row_h = RECENT_ROW_H;
        let menu_title_y = main_p.y + 28.0;
        let items_y0 = main_p.items_y0;
        let hint_y = items_y0 + menu_item_count as f32 * row_h + 10.0;
        let recent_title_y = hint_y + hint_h + 22.0;
        let recent_y0 = recent_title_y + 22.0;
        let text_left = main_p.x + panel_pad_x;
        let locs_text_left = locs_p.x + panel_pad_x;
        let locs_hint_y = locs_p.items_y0 + locs_item_count as f32 * locs_p.row_h + 10.0;

        // Center info box geometry, filled in when it has any rows.
        let mut info_box: Option<(f32, f32, f32, f32)> = None;

        if !menu_open && !shell_open {
            // Headline speed: large, centered, just above the info box.
            let speed_w = line_width(&self.speed_buf);
            let speed_top = h * 0.40;
            areas.push(TextArea {
                buffer: &self.speed_buf,
                left: (w - speed_w) * 0.5,
                top: speed_top,
                scale: 1.0,
                bounds,
                default_color: speed_color,
                custom_glyphs: &[],
            });

            // Rows of the box, in the reference HUD's order.
            let mut rows: Vec<(&Buffer, Color, f32)> = Vec::with_capacity(8);
            if time_label.is_some() {
                rows.push((&self.time_buf, time_color, 27.0));
            }
            if hud.split_line.is_some() {
                rows.push((&self.split_buf, split_color, 23.0));
            }
            if phase_label.is_some() {
                rows.push((&self.phase_buf, phase_color, 22.0));
            }
            if pb_abs_label.is_some() {
                rows.push((&self.pb_abs_buf, pb_abs_color, 22.0));
            }
            if delta_label.is_some() {
                rows.push((&self.delta_buf, delta_color, 23.0));
            }
            if ghost_label.is_some() {
                rows.push((&self.ghost_buf, ghost_color, 22.0));
            }
            if sync_label.is_some() {
                rows.push((&self.sync_buf, sync_color, 22.0));
            }

            if !rows.is_empty() {
                let pad_x = 24.0;
                let pad_y = 14.0;
                let content_w = rows
                    .iter()
                    .map(|(b, _, _)| line_width(b))
                    .fold(0.0f32, f32::max);
                let content_h: f32 = rows.iter().map(|(_, _, lh)| *lh).sum();
                let box_w = (content_w + pad_x * 2.0).min(w - 24.0);
                let box_h = content_h + pad_y * 2.0;
                let box_x = (w - box_w) * 0.5;
                let box_y = (h * 0.60).max(speed_top + 80.0);
                info_box = Some((box_x, box_y, box_w, box_h));

                let mut y = box_y + pad_y;
                for (buf, color, lh) in rows {
                    let lw = line_width(buf);
                    areas.push(TextArea {
                        buffer: buf,
                        left: (w - lw) * 0.5,
                        top: y,
                        scale: 1.0,
                        bounds,
                        default_color: color,
                        custom_glyphs: &[],
                    });
                    y += lh;
                }
            }

            if hud.pb_flash {
                let fw = line_width(&self.flash_buf);
                areas.push(TextArea {
                    buffer: &self.flash_buf,
                    left: (w - fw) * 0.5,
                    top: speed_top - 54.0,
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
            // Row text is brightest on the focused panel's selection; the
            // unfocused panel keeps a dimmer marker so you never lose your place.
            let row_color = |panel: &MenuPanel, i: usize| {
                if i == panel.selected {
                    if panel.focused {
                        menu_sel
                    } else {
                        menu_sel_dim
                    }
                } else if panel.hovered == Some(i) {
                    menu_hover
                } else {
                    menu_norm
                }
            };

            let tw = line_width(&self.menu_title_buf);
            areas.push(TextArea {
                buffer: &self.menu_title_buf,
                left: main_p.x + (main_p.w - tw) * 0.5,
                top: menu_title_y,
                scale: 1.0,
                bounds,
                default_color: menu_title_color,
                custom_glyphs: &[],
            });
            for i in 0..menu_item_count {
                areas.push(TextArea {
                    buffer: &self.menu_bufs[i],
                    left: text_left,
                    top: items_y0 + i as f32 * row_h + 4.0,
                    scale: 1.0,
                    bounds,
                    default_color: row_color(&menu.main, i),
                    custom_glyphs: &[],
                });
            }
            let hw = line_width(&self.menu_hint_buf);
            areas.push(TextArea {
                buffer: &self.menu_hint_buf,
                left: main_p.x + (main_p.w - hw) * 0.5,
                top: hint_y,
                scale: 1.0,
                bounds,
                default_color: menu_hint_color,
                custom_glyphs: &[],
            });

            let ltw = line_width(&self.locs_title_buf);
            areas.push(TextArea {
                buffer: &self.locs_title_buf,
                left: locs_p.x + (locs_p.w - ltw) * 0.5,
                top: menu_title_y,
                scale: 1.0,
                bounds,
                default_color: menu_title_color,
                custom_glyphs: &[],
            });
            for i in 0..locs_item_count {
                areas.push(TextArea {
                    buffer: &self.locs_bufs[i],
                    left: locs_text_left,
                    top: locs_p.items_y0 + i as f32 * locs_p.row_h + 4.0,
                    scale: 1.0,
                    bounds,
                    default_color: row_color(&menu.locs, i),
                    custom_glyphs: &[],
                });
            }
            let lhw = line_width(&self.locs_hint_buf);
            areas.push(TextArea {
                buffer: &self.locs_hint_buf,
                left: locs_p.x + (locs_p.w - lhw) * 0.5,
                top: locs_hint_y,
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
        // Shell page: title, subtitle, rows, message, hint.
        let shell_p = shell_layout(w, h, shell_item_count);
        if let Some(ref sh) = hud.shell {
            let text_left = shell_p.x + PANEL_PAD_X;
            areas.push(TextArea {
                buffer: &self.shell_title_buf,
                left: text_left,
                top: shell_p.y + 26.0,
                scale: 1.0,
                bounds,
                default_color: Color::rgb(240, 238, 232),
                custom_glyphs: &[],
            });
            areas.push(TextArea {
                buffer: &self.shell_sub_buf,
                left: text_left,
                top: shell_p.y + 78.0,
                scale: 1.0,
                bounds,
                default_color: Color::rgb(130, 136, 148),
                custom_glyphs: &[],
            });
            for i in 0..shell_item_count {
                let logical = sh.scroll + i;
                let color = if logical == sh.selected {
                    Color::rgb(255, 226, 140)
                } else {
                    Color::rgb(198, 202, 210)
                };
                areas.push(TextArea {
                    buffer: &self.shell_bufs[i],
                    left: text_left + 14.0,
                    top: shell_p.items_y0 + i as f32 * shell_p.row_h + 4.0,
                    scale: 1.0,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
            }
            let msg_y = shell_p.items_y0 + shell_item_count as f32 * shell_p.row_h + 16.0;
            if sh.message.is_some() {
                areas.push(TextArea {
                    buffer: &self.shell_msg_buf,
                    left: text_left,
                    top: msg_y,
                    scale: 1.0,
                    bounds,
                    default_color: Color::rgb(232, 138, 116),
                    custom_glyphs: &[],
                });
            }
            areas.push(TextArea {
                buffer: &self.shell_hint_buf,
                left: text_left,
                top: msg_y + 26.0,
                scale: 1.0,
                bounds,
                default_color: Color::rgb(118, 124, 136),
                custom_glyphs: &[],
            });
        }

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
        if let Some(ref sh) = hud.shell {
            // Near-opaque over the live world behind it: the world reads as a
            // backdrop, not as something you could still be playing.
            push_rect_ndc(&mut verts, -1.0, -1.0, 2.0, 2.0, [0.03, 0.035, 0.045, 0.90]);
            push_rect_px(&mut verts, shell_p.x, shell_p.y, shell_p.w, shell_p.h, w, h, [0.09, 0.095, 0.12, 0.95]);
            push_rect_px(&mut verts, shell_p.x, shell_p.y, shell_p.w, 2.0, w, h, [0.95, 0.78, 0.32, 0.95]);
            if let Some(hov) = sh.hovered {
                if hov >= sh.scroll && hov - sh.scroll < shell_item_count && hov != sh.selected {
                    push_rect_px(
                        &mut verts,
                        shell_p.x + 10.0,
                        shell_p.items_y0 + (hov - sh.scroll) as f32 * shell_p.row_h,
                        shell_p.w - 20.0,
                        shell_p.row_h,
                        w,
                        h,
                        [1.0, 1.0, 1.0, 0.05],
                    );
                }
            }
            if sh.selected >= sh.scroll && sh.selected - sh.scroll < shell_item_count {
                let sel_y = shell_p.items_y0 + (sh.selected - sh.scroll) as f32 * shell_p.row_h;
                push_rect_px(&mut verts, shell_p.x + 10.0, sel_y, shell_p.w - 20.0, shell_p.row_h, w, h, [1.0, 0.88, 0.40, 0.10]);
                push_rect_px(&mut verts, shell_p.x + 10.0, sel_y + 5.0, 3.0, shell_p.row_h - 10.0, w, h, [1.0, 0.86, 0.35, 0.95]);
            }
        } else if menu_open {
            if let Some(ref menu) = hud.menu {
                // Soft full-screen dim.
                push_rect_ndc(&mut verts, -1.0, -1.0, 2.0, 2.0, [0.02, 0.02, 0.04, 0.62]);
                // Both panels: fill, top accent (brighter on the focused one),
                // hover wash, then the selection band and tick.
                for (panel, geom, count) in [
                    (&menu.main, main_p, menu_item_count),
                    (&menu.locs, locs_p, locs_item_count),
                ] {
                    push_rect_px(
                        &mut verts,
                        geom.x,
                        geom.y,
                        geom.w,
                        geom.h,
                        w,
                        h,
                        [0.09, 0.095, 0.12, 0.94],
                    );
                    let accent = if panel.focused {
                        [0.95, 0.78, 0.32, 0.95]
                    } else {
                        [0.55, 0.47, 0.26, 0.75]
                    };
                    push_rect_px(&mut verts, geom.x, geom.y, geom.w, 2.0, w, h, accent);

                    if let Some(hov) = panel.hovered {
                        if hov < count && Some(hov) != Some(panel.selected) {
                            push_rect_px(
                                &mut verts,
                                geom.x + 10.0,
                                geom.items_y0 + hov as f32 * geom.row_h,
                                geom.w - 20.0,
                                geom.row_h,
                                w,
                                h,
                                [1.0, 1.0, 1.0, 0.05],
                            );
                        }
                    }

                    if panel.selected < count {
                        let sel_y = geom.items_y0 + panel.selected as f32 * geom.row_h;
                        let (band, tick) = if panel.focused {
                            ([1.0, 0.88, 0.40, 0.10], [1.0, 0.86, 0.35, 0.95])
                        } else {
                            ([1.0, 0.88, 0.40, 0.05], [1.0, 0.86, 0.35, 0.45])
                        };
                        push_rect_px(
                            &mut verts,
                            geom.x + 10.0,
                            sel_y,
                            geom.w - 20.0,
                            geom.row_h,
                            w,
                            h,
                            band,
                        );
                        push_rect_px(
                            &mut verts,
                            geom.x + 10.0,
                            sel_y + 5.0,
                            3.0,
                            geom.row_h - 10.0,
                            w,
                            h,
                            tick,
                        );
                    }
                }
                // Hairline above the recent section (settings panel only).
                let sep_y = hint_y + hint_h + 12.0;
                push_rect_px(
                    &mut verts,
                    main_p.x + panel_pad_x,
                    sep_y,
                    main_p.w - panel_pad_x * 2.0,
                    1.0,
                    w,
                    h,
                    [1.0, 1.0, 1.0, 0.08],
                );
            }
        } else if let Some((bx, by, bw, bh)) = info_box {
            // Light box behind the run/CP/ghost readout — enough contrast to
            // stay legible over bright geometry without hiding the map.
            push_rect_px(&mut verts, bx, by, bw, bh, w, h, [0.06, 0.07, 0.09, 0.42]);
            let edge = [1.0, 1.0, 1.0, 0.10];
            push_rect_px(&mut verts, bx, by, bw, 1.0, w, h, edge);
            push_rect_px(&mut verts, bx, by + bh - 1.0, bw, 1.0, w, h, edge);
            push_rect_px(&mut verts, bx, by, 1.0, bh, w, h, edge);
            push_rect_px(&mut verts, bx + bw - 1.0, by, 1.0, bh, w, h, edge);
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
        let _ = self.text_renderer.render(&self.atlas, &self.viewport, pass);
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
    for p in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
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
    for p in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_sit_side_by_side_without_overlapping() {
        let l = menu_layout(1920.0, 1080.0, 18, 8, 5);
        assert!(l.main.x + l.main.w <= l.locs.x, "panels overlap");
        assert!(l.locs.x + l.locs.w <= 1920.0, "locs panel off-screen");
        assert_eq!(l.main.y, l.locs.y, "panels should be top-aligned");
        // Group is centred: equal margins either side.
        let left = l.main.x;
        let right = 1920.0 - (l.locs.x + l.locs.w);
        assert!(
            (left - right).abs() < 1.0,
            "group not centred: {left} vs {right}"
        );
    }

    #[test]
    fn narrow_windows_shrink_panels_instead_of_dropping_them() {
        let l = menu_layout(900.0, 700.0, 18, 8, 5);
        assert!(l.main.x >= 0.0);
        assert!(
            l.locs.x + l.locs.w <= 900.0,
            "locs panel must stay on-screen"
        );
        assert!(
            l.main.x + l.main.w <= l.locs.x,
            "panels overlap when narrow"
        );
    }

    #[test]
    fn row_hit_test_matches_drawn_rows() {
        let l = menu_layout(1920.0, 1080.0, 18, 6, 5);
        let p = l.locs;
        // Centre of each row hits that row.
        for i in 0..p.rows {
            let y = p.items_y0 + (i as f32 + 0.5) * p.row_h;
            assert_eq!(p.row_at(p.x + 20.0, y), Some(i), "row {i}");
        }
        // Just above the first row, and past the last, hit nothing.
        assert_eq!(p.row_at(p.x + 20.0, p.items_y0 - 1.0), None);
        let past = p.items_y0 + p.rows as f32 * p.row_h + 1.0;
        assert_eq!(p.row_at(p.x + 20.0, past), None);
        // Horizontally outside the panel hits nothing, even at a valid row y.
        let y = p.items_y0 + p.row_h * 0.5;
        assert_eq!(p.row_at(p.x - 5.0, y), None);
        assert_eq!(p.row_at(p.x + p.w + 5.0, y), None);
    }

    #[test]
    fn clicks_on_one_panel_never_hit_the_other() {
        let l = menu_layout(1920.0, 1080.0, 18, 10, 5);
        let y = l.main.items_y0 + l.main.row_h * 0.5;
        // A point in the gap belongs to neither panel.
        let gap_x = l.main.x + l.main.w + 2.0;
        assert!(gap_x < l.locs.x);
        assert_eq!(l.main.row_at(gap_x, y), None);
        assert_eq!(l.locs.row_at(gap_x, y), None);
    }

    #[test]
    fn empty_locs_panel_has_no_rows_to_hit() {
        let l = menu_layout(1920.0, 1080.0, 18, 0, 5);
        assert_eq!(l.locs.rows, 0);
        assert_eq!(l.locs.row_at(l.locs.x + 10.0, l.locs.items_y0 + 5.0), None);
    }
}
