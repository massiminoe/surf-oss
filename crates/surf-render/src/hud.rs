//! HUD: glyphon text (JetBrains Mono) for the gameplay readout, plus the
//! menu system — one page model shared by the pause overlay and every shell
//! page (title screen, map picker, settings, leaderboard, loading).
//!
//! There used to be two parallel implementations: a side-by-side pair of pause
//! panels and a separate single-panel shell. They drifted (only one had
//! scrolling, only one had hover). Now both build a [`MenuPage`] and go through
//! one layout function and one draw path, so a feature added to menus is added
//! everywhere at once.
//!
//! [`page_layout`] is the single source of geometry: it returns the exact rects
//! the renderer draws, and the app hit-tests the same values, so a click can
//! never land on a row other than the one on screen.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

use crate::backdrop::Backdrop;

const FONT_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
const FONT_FAMILY: &str = "JetBrains Mono";
/// Menu type (direction "Instrument", Max 2026-09-02): a humanist sans for
/// titles and labels, the mono kept for every number, tab and key hint so
/// digits stay tabular. Three static instances — cosmic-text does not drive a
/// variable weight axis.
const FONT_SANS_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/InstrumentSans-Regular.ttf");
const FONT_SANS_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/InstrumentSans-Medium.ttf");
const FONT_SANS_SEMIBOLD: &[u8] =
    include_bytes!("../../../assets/fonts/InstrumentSans-SemiBold.ttf");
const SANS_FAMILY: &str = "Instrument Sans";

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

// ---------------------------------------------------------------------------
// Menu model
// ---------------------------------------------------------------------------

/// What a row *is*, which decides how it draws and whether it can be selected.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum RowKind {
    /// Section heading. Never selectable — the app's cursor skips it.
    Header,
    #[default]
    Item,
    /// Continuous value: draws a track under the row, `frac` full.
    Slider(f32),
    /// Read-only line (a leaderboard entry).
    Text,
}

impl RowKind {
    pub fn selectable(&self) -> bool {
        !matches!(self, RowKind::Header)
    }
}

/// Colour role for a row's value. Keeps the palette in the renderer instead of
/// scattering RGB triples through the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowTone {
    #[default]
    Normal,
    Dim,
    /// A PB, a win, a time ahead.
    Good,
    /// Something armed / destructive.
    Warn,
    Accent,
}

/// One menu line: `label` left, optional `note` in the middle column, `value`
/// right-aligned. Three columns cover every page we have (settings, locs,
/// leaderboard) without per-page layout code.
#[derive(Clone, Debug, Default)]
pub struct MenuRow {
    pub label: String,
    pub note: String,
    pub value: String,
    pub kind: RowKind,
    pub tone: RowTone,
}

impl MenuRow {
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            kind: RowKind::Header,
            ..Default::default()
        }
    }

    pub fn item(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            ..Default::default()
        }
    }

    pub fn slider(label: impl Into<String>, value: impl Into<String>, frac: f32) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            kind: RowKind::Slider(frac.clamp(0.0, 1.0)),
            ..Default::default()
        }
    }

    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            kind: RowKind::Text,
            ..Default::default()
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    pub fn with_tone(mut self, tone: RowTone) -> Self {
        self.tone = tone;
        self
    }
}

/// The scrolling list of a page. `selected` / `hovered` / `scroll` are all in
/// *logical* row coordinates; the renderer draws `rows[scroll..]`.
#[derive(Clone, Debug, Default)]
pub struct MenuPanel {
    pub rows: Vec<MenuRow>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub scroll: usize,
    /// Panel owns the keyboard (vs. the button bar).
    pub focused: bool,
}

/// A full menu page: pause overlay or shell screen.
#[derive(Clone, Debug, Default)]
pub struct MenuPage {
    pub title: String,
    pub subtitle: String,
    /// Section tabs. Empty = no tab strip.
    pub tabs: Vec<String>,
    pub tab: usize,
    pub tab_hovered: Option<usize>,
    pub panel: MenuPanel,
    /// Always-visible action bar under the list (Resume / Back / Quit …).
    pub buttons: Vec<String>,
    /// `Some` = keyboard focus is on the button bar, at this index.
    pub button_selected: Option<usize>,
    pub button_hovered: Option<usize>,
    /// Status or error line under the list.
    pub message: Option<String>,
    pub hint: String,
    /// Wider panel, for pages with three real columns (leaderboard).
    pub wide: bool,
    /// Draw the procedural backdrop over the world (shell pages). `false` just
    /// dims the world, which is what the pause overlay wants.
    pub backdrop: bool,
    /// Indeterminate progress bar under the title (loading).
    pub busy: bool,
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, mx: f32, my: f32) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h
    }
}

/// Pixel geometry of the row list. The renderer draws from this and the app
/// hit-tests against it, so a click always lands on the row you can see.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelLayout {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Top of the first *drawn* row.
    pub items_y0: f32,
    pub row_h: f32,
    /// How many rows fit / are drawn (not the logical row count).
    pub rows: usize,
}

impl PanelLayout {
    /// Drawn-row index under a cursor position, if it is over one.
    pub fn row_at(&self, mx: f32, my: f32) -> Option<usize> {
        if self.rows == 0 || self.row_h <= 0.0 {
            return None;
        }
        if mx < self.x || mx > self.x + self.w || my < self.items_y0 {
            return None;
        }
        let row = ((my - self.items_y0) / self.row_h).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        (row < self.rows).then_some(row)
    }

    pub fn contains(&self, mx: f32, my: f32) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h
    }

    /// Horizontal span of a slider track. Deliberately the right half of the
    /// row: clicking a label still just selects, clicking the track sets the
    /// value the way a slider should. The right end stops short of the value
    /// column so the bar never runs under the number it is showing.
    pub fn slider_track(&self) -> (f32, f32) {
        let x0 = self.x + self.w * 0.44;
        let x1 = self.x + self.w - PANEL_PAD_X - VALUE_GUTTER;
        (x0, x1.max(x0 + 1.0))
    }

    /// Fraction 0..1 for a click at `mx` on a slider row, or `None` when the
    /// cursor is left of the track (the label half).
    pub fn slider_frac_at(&self, mx: f32) -> Option<f32> {
        let (x0, x1) = self.slider_track();
        if mx < x0 - 8.0 {
            return None;
        }
        Some(((mx - x0) / (x1 - x0)).clamp(0.0, 1.0))
    }
}

/// Everything a page draws, in pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageLayout {
    pub panel: PanelLayout,
    pub tabs: Vec<Rect>,
    pub buttons: Vec<Rect>,
    pub title_y: f32,
    pub subtitle_y: f32,
    pub busy_y: f32,
    pub message_y: f32,
    pub hint_y: f32,
}

/// Counts a page needs laid out. Kept as a struct so adding a region later
/// can't silently reorder positional arguments at a call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageSpec {
    /// Total logical rows (the layout decides how many of them fit).
    pub rows: usize,
    pub tabs: usize,
    pub buttons: usize,
    pub wide: bool,
}

const PANEL_PAD_X: f32 = 30.0;
/// Room left of the label column for the selection mark.
const CURSOR_INSET: f32 = 14.0;
/// Width reserved at the right of a row for its value text.
const VALUE_GUTTER: f32 = 66.0;
const ROW_H: f32 = 28.0;
const TAB_H: f32 = 30.0;
const BUTTON_H: f32 = 34.0;
/// Tabs and buttons are text with an underline, sized to a fixed column and
/// left-aligned — a strip of equal boxes read as a toolbar, not a menu.
const TAB_W: f32 = 112.0;
const BUTTON_W: f32 = 118.0;

/// Hard cap on drawn rows — must stay <= the renderer's row buffer pool.
pub const PANEL_ROWS_MAX: usize = 22;

/// Lay out a menu page. Pure: the app calls this to hit-test exactly what the
/// renderer drew.
pub fn page_layout(w: f32, h: f32, spec: PageSpec) -> PageLayout {
    let want = if spec.wide { w * 0.62 } else { w * 0.48 };
    let (lo, hi) = if spec.wide {
        (420.0, 900.0)
    } else {
        (360.0, 660.0)
    };
    let pw = want.clamp(lo, hi).min((w - 40.0).max(240.0));
    let px = ((w - pw) * 0.5).max(12.0);
    let py = (h * 0.11).max(26.0);

    let title_y = py + 26.0;
    let subtitle_y = py + 60.0;
    let has_tabs = spec.tabs > 0;
    let tabs_y = py + 92.0;
    let items_y0 = if has_tabs {
        tabs_y + TAB_H + 14.0
    } else {
        py + 98.0
    };

    // Reserve the space below the list before deciding how many rows fit.
    let below = 22.0 /* message */ + 24.0 /* hint */
        + if spec.buttons > 0 { BUTTON_H + 26.0 } else { 14.0 }
        + 24.0;
    let avail = (h - items_y0 - below - 16.0).max(0.0);
    let fits = (avail / ROW_H).floor().max(0.0) as usize;
    let rows = spec.rows.min(fits).min(PANEL_ROWS_MAX);

    let items_end = items_y0 + rows as f32 * ROW_H;
    let message_y = items_end + 12.0;
    let hint_y = message_y + 24.0;
    let buttons_y = hint_y + 26.0;

    let inner_x = px + PANEL_PAD_X;
    let inner_w = pw - PANEL_PAD_X * 2.0;

    let tabs = if has_tabs {
        let tw = TAB_W.min(inner_w / spec.tabs as f32);
        (0..spec.tabs)
            .map(|i| Rect {
                x: inner_x + i as f32 * tw,
                y: tabs_y,
                w: tw,
                h: TAB_H,
            })
            .collect()
    } else {
        Vec::new()
    };

    let buttons = if spec.buttons > 0 {
        let bw = BUTTON_W.min(inner_w / spec.buttons as f32);
        (0..spec.buttons)
            .map(|i| Rect {
                x: inner_x + i as f32 * bw,
                y: buttons_y,
                w: bw,
                h: BUTTON_H,
            })
            .collect()
    } else {
        Vec::new()
    };

    let bottom = if spec.buttons > 0 {
        buttons_y + BUTTON_H + 22.0
    } else {
        hint_y + 30.0
    };

    PageLayout {
        panel: PanelLayout {
            x: px,
            y: py,
            w: pw,
            h: (bottom - py).max(180.0),
            items_y0,
            row_h: ROW_H,
            rows,
        },
        tabs,
        buttons,
        title_y,
        subtitle_y,
        busy_y: subtitle_y + 30.0,
        message_y,
        hint_y,
    }
}

/// Layout for `page`, so the app and the renderer agree without rebuilding the
/// spec in two places.
pub fn layout_for(w: f32, h: f32, page: &MenuPage) -> PageLayout {
    page_layout(
        w,
        h,
        PageSpec {
            rows: page.panel.rows.len(),
            tabs: page.tabs.len(),
            buttons: page.buttons.len(),
            wide: page.wide,
        },
    )
}

// ---------------------------------------------------------------------------
// HUD state
// ---------------------------------------------------------------------------

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
    pub practice: bool,
    /// Practice mode armed — loadloc is unlocked.
    pub practice_mode: bool,
    /// Live time delta vs PB ghost (negative = ahead).
    pub ghost_time_delta: Option<f32>,
    /// Live 2D speed delta vs PB ghost (positive = faster than ghost).
    pub ghost_speed_delta: Option<f32>,
    /// Pause overlay or shell page. Takes over the frame when present.
    pub page: Option<MenuPage>,
    /// Optional FPS / frame-time / resolution line (bottom-right).
    pub perf_line: Option<String>,
    /// Seconds since app start — drives the backdrop and the busy bar.
    pub time: f32,
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
            page: None,
            perf_line: None,
            time: 0.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HudVert {
    pos: [f32; 2],
    color: [f32; 4],
}

// Palette. The accent is the one locked for the world art (#2FBF7F); amber is
// kept for times only, so "this is a result" and "this is selected" never read
// as the same thing.
// Quad colours are written to an sRGB surface, so these are the *linear*
// values of the intended swatches — spelling #2FBF7F as 0.184 would come out
// three stops light.
const C_ACCENT: [f32; 4] = [0.02843, 0.521, 0.21223, 1.0]; // #2FBF7F
// Direction "Instrument" swatches: ink #E9EEEC on ground #0C1012, two greys
// biased toward the accent (#8C9A96 / #4E5A57), accent #2FBF7F used only for
// the cursor mark, the selected value and slider fill.
const TXT_TITLE: Color = Color::rgb(233, 238, 236);
const TXT_SUB: Color = Color::rgb(140, 154, 150);
const TXT_HEADER: Color = Color::rgb(110, 122, 118);
const TXT_LABEL: Color = Color::rgb(196, 204, 201);
const TXT_LABEL_SEL: Color = Color::rgb(255, 255, 255);
const TXT_VALUE: Color = Color::rgb(233, 238, 236);
const TXT_ACCENT: Color = Color::rgb(47, 191, 127);
const TXT_GOOD: Color = Color::rgb(47, 191, 127);
const TXT_WARN: Color = Color::rgb(240, 170, 90);
const TXT_DIM: Color = Color::rgb(110, 122, 118);
const TXT_HINT: Color = Color::rgb(110, 122, 118);
const TXT_ERR: Color = Color::rgb(232, 138, 116);
/// Linear #E9EEEC (ink) and #0C1012 (ground) for the quad pass — the surface
/// is sRGB, so the byte values would come out three stops light.
const C_INK: [f32; 3] = [0.82279, 0.87137, 0.84652];
const C_GROUND: [f32; 3] = [0.00335, 0.00518, 0.00605];
fn ink(a: f32) -> [f32; 4] {
    [C_INK[0], C_INK[1], C_INK[2], a]
}
fn ground(a: f32) -> [f32; 4] {
    [C_GROUND[0], C_GROUND[1], C_GROUND[2], a]
}
fn accent(a: f32) -> [f32; 4] {
    [C_ACCENT[0], C_ACCENT[1], C_ACCENT[2], a]
}

pub struct HudRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    backdrop: Backdrop,
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
    page_title_buf: Buffer,
    page_sub_buf: Buffer,
    page_hint_buf: Buffer,
    page_msg_buf: Buffer,
    tab_bufs: Vec<Buffer>,
    label_bufs: Vec<Buffer>,
    note_bufs: Vec<Buffer>,
    value_bufs: Vec<Buffer>,
    button_bufs: Vec<Buffer>,
    bar_pipeline: wgpu::RenderPipeline,
    bar_vbo: wgpu::Buffer,
    bar_capacity: u32,
    bar_vert_count: u32,
}

/// Tab strip / button bar pools. Beyond this a page's extra entries are simply
/// not drawn, so keep them in step with what the app builds.
const MAX_TABS: usize = 5;
const MAX_BUTTONS: usize = 6;

impl HudRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(FONT_MEDIUM.to_vec());
        font_system.db_mut().load_font_data(FONT_SANS_REGULAR.to_vec());
        font_system.db_mut().load_font_data(FONT_SANS_MEDIUM.to_vec());
        font_system.db_mut().load_font_data(FONT_SANS_SEMIBOLD.to_vec());

        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        // Speed is the headline read — everything else is deliberately quieter.
        let speed_buf = Buffer::new(&mut font_system, Metrics::new(44.0, 50.0));
        let time_buf = Buffer::new(&mut font_system, Metrics::new(30.0, 36.0));
        let delta_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
        let phase_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let pb_abs_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let flash_buf = Buffer::new(&mut font_system, Metrics::new(32.0, 38.0));
        let split_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 21.0));
        let ghost_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let sync_buf = Buffer::new(&mut font_system, Metrics::new(15.0, 20.0));
        let keys_buf = Buffer::new(&mut font_system, Metrics::new(18.0, 22.0));
        let perf_buf = Buffer::new(&mut font_system, Metrics::new(13.0, 16.0));

        let page_title_buf = Buffer::new(&mut font_system, Metrics::new(24.0, 30.0));
        let page_sub_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let page_hint_buf = Buffer::new(&mut font_system, Metrics::new(12.0, 16.0));
        let page_msg_buf = Buffer::new(&mut font_system, Metrics::new(13.0, 18.0));
        let tab_bufs = (0..MAX_TABS)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(12.0, 16.0)))
            .collect();
        let mk_rows = |fs: &mut FontSystem, size: f32| {
            (0..PANEL_ROWS_MAX)
                .map(|_| Buffer::new(fs, Metrics::new(size, size + 5.0)))
                .collect::<Vec<_>>()
        };
        let label_bufs = mk_rows(&mut font_system, 15.0);
        let note_bufs = mk_rows(&mut font_system, 12.0);
        let value_bufs = mk_rows(&mut font_system, 13.0);
        let button_bufs = (0..MAX_BUTTONS)
            .map(|_| Buffer::new(&mut font_system, Metrics::new(12.0, 16.0)))
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

        let bar_capacity = 4096u32;
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
            backdrop: Backdrop::new(device, format),
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
            page_title_buf,
            page_sub_buf,
            page_hint_buf,
            page_msg_buf,
            tab_bufs,
            label_bufs,
            note_bufs,
            value_bufs,
            button_bufs,
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
        let sans = Attrs::new()
            .family(Family::Name(SANS_FAMILY))
            .weight(Weight::NORMAL);
        let sans_title = Attrs::new()
            .family(Family::Name(SANS_FAMILY))
            .weight(Weight::MEDIUM);

        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };

        let page_open = hud.page.is_some();
        self.backdrop.prepare(
            queue,
            hud.time,
            width,
            height,
            match hud.page.as_ref() {
                Some(p) if p.backdrop => 1.0,
                _ => 0.0,
            },
        );

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
            .map(format_hud_time);
        if let Some(ref t) = time_label {
            set_buf_text(&mut self.font_system, &mut self.time_buf, t, attrs, w, 30.0);
        }

        let base_phase_label = match hud.timer_phase {
            HudTimerPhase::Finished => Some("FINISH"),
            HudTimerPhase::Armed => hud.stage_line.as_deref().or(Some("START")),
            HudTimerPhase::Running => hud.stage_line.as_deref(),
            HudTimerPhase::Idle => None,
        };
        // Both states always show, even where there'd be no label: a loaded run
        // must never be mistaken for a clean one, and you must be able to see at
        // a glance whether Mouse1 is armed.
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

        let sync_label = if hud.show_sync_bar {
            Some(format!("sync {:.0}", hud.sync.clamp(0.0, 100.0)))
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

        // --- menu page text ---
        let layout = hud
            .page
            .as_ref()
            .map(|p| layout_for(w, h, p))
            .unwrap_or_default();
        let mut drawn_rows = 0usize;
        let mut tab_count = 0usize;
        let mut button_count = 0usize;
        if let Some(ref page) = hud.page {
            set_buf_text(
                &mut self.font_system,
                &mut self.page_title_buf,
                &page.title,
                sans_title,
                w,
                44.0,
            );
            set_buf_text(
                &mut self.font_system,
                &mut self.page_sub_buf,
                &page.subtitle,
                attrs,
                w,
                20.0,
            );
            set_buf_text(
                &mut self.font_system,
                &mut self.page_hint_buf,
                &page.hint,
                attrs,
                w,
                18.0,
            );
            set_buf_text(
                &mut self.font_system,
                &mut self.page_msg_buf,
                page.message.as_deref().unwrap_or(""),
                sans,
                w,
                20.0,
            );

            tab_count = page
                .tabs
                .len()
                .min(self.tab_bufs.len())
                .min(layout.tabs.len());
            for i in 0..tab_count {
                set_buf_text(
                    &mut self.font_system,
                    &mut self.tab_bufs[i],
                    &page.tabs[i],
                    attrs,
                    w,
                    20.0,
                );
            }

            let start = page.panel.scroll.min(page.panel.rows.len());
            drawn_rows = layout.panel.rows.min(page.panel.rows.len() - start);
            let col_w = layout.panel.w - PANEL_PAD_X * 2.0;
            for i in 0..drawn_rows {
                let row = &page.panel.rows[start + i];
                set_buf_text(
                    &mut self.font_system,
                    &mut self.label_bufs[i],
                    &row.label,
                    if row.kind == RowKind::Header { attrs } else { sans },
                    col_w,
                    22.0,
                );
                set_buf_text(
                    &mut self.font_system,
                    &mut self.note_bufs[i],
                    &row.note,
                    attrs,
                    col_w,
                    20.0,
                );
                set_buf_text(
                    &mut self.font_system,
                    &mut self.value_bufs[i],
                    &row.value,
                    attrs,
                    col_w,
                    22.0,
                );
            }

            button_count = page
                .buttons
                .len()
                .min(self.button_bufs.len())
                .min(layout.buttons.len());
            for i in 0..button_count {
                set_buf_text(
                    &mut self.font_system,
                    &mut self.button_bufs[i],
                    &page.buttons[i],
                    attrs,
                    w,
                    20.0,
                );
            }
        }

        // In-run readout: ink for the facts, accent / warm red only for a
        // delta's sign, warn orange only for practice. Airborne no longer tints
        // the speed — the bar under it carries that.
        let behind = Color::rgb(232, 120, 104);
        let speed_color = TXT_TITLE;
        let time_color = if hud.timer_phase == HudTimerPhase::Finished && !hud.practice {
            TXT_ACCENT
        } else {
            TXT_TITLE
        };
        let phase_color = if hud.practice {
            TXT_WARN
        } else if hud.practice_mode {
            Color::rgb(190, 165, 120)
        } else {
            TXT_LABEL
        };
        let pb_abs_color = TXT_SUB;
        let delta_color = match hud.pb_delta_secs {
            Some(d) if d < 0.0 => TXT_GOOD,
            Some(d) if d > 0.0 => behind,
            _ => TXT_SUB,
        };
        let split_color = match hud.split_line.as_deref() {
            Some(s) if s.contains(" -") => TXT_GOOD,
            Some(s) if s.contains(" +") => behind,
            _ => TXT_LABEL,
        };
        let ghost_color = match hud.ghost_time_delta {
            Some(d) if d < 0.0 => TXT_GOOD,
            Some(d) if d > 0.0 => behind,
            _ => TXT_SUB,
        };

        let mut areas: Vec<TextArea> = Vec::with_capacity(96);

        // Speed bar + sync bar geometry, drawn into the quad pass below.
        let mut speed_bar: Option<(f32, f32, f32, f32)> = None; // x, y, w, frac
        let mut sync_bar: Option<(f32, f32, f32, f32)> = None;
        let mut crosshair = false;
        // Soft ground washes behind the two text blocks so they stay legible
        // over a bright sky or floor. No frame — the text is the shape.
        let mut washes: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(2);

        if !page_open {
            // Headline speed: top centre, with a hairline bar under it that
            // fills toward speed_scale. The edges of the screen carry the
            // rest, so the middle stays clear for the map.
            let speed_w = line_width(&self.speed_buf);
            let speed_top = 30.0;
            areas.push(TextArea {
                buffer: &self.speed_buf,
                left: (w - speed_w) * 0.5,
                top: speed_top,
                scale: 1.0,
                bounds,
                default_color: speed_color,
                custom_glyphs: &[],
            });
            let bar_w = 200.0;
            speed_bar = Some((
                (w - bar_w) * 0.5,
                speed_top + 58.0,
                bar_w,
                (hud.speed / hud.speed_scale.max(1.0)).clamp(0.0, 1.0),
            ));
            crosshair = true;

            // Timer block, bottom-left: the clock large, then its context
            // lines beneath it in reading order.
            let mut rows: Vec<(&Buffer, Color, f32)> = Vec::with_capacity(8);
            if time_label.is_some() {
                rows.push((&self.time_buf, time_color, 36.0));
            }
            if delta_label.is_some() {
                rows.push((&self.delta_buf, delta_color, 21.0));
            }
            if hud.split_line.is_some() {
                rows.push((&self.split_buf, split_color, 21.0));
            }
            if ghost_label.is_some() {
                rows.push((&self.ghost_buf, ghost_color, 20.0));
            }
            if pb_abs_label.is_some() {
                rows.push((&self.pb_abs_buf, pb_abs_color, 20.0));
            }
            let block_h: f32 = rows.iter().map(|(_, _, lh)| *lh).sum();
            let block_w = rows
                .iter()
                .map(|(b, _, _)| line_width(b))
                .fold(0.0f32, f32::max);
            let mut y = h - 56.0 - block_h;
            if !rows.is_empty() {
                washes.push((44.0 - 14.0, y - 10.0, block_w + 28.0, block_h + 20.0));
            }
            washes.push((
                (w - speed_w.max(bar_w)) * 0.5 - 20.0,
                speed_top - 8.0,
                speed_w.max(bar_w) + 40.0,
                76.0,
            ));
            for (buf, color, lh) in rows {
                areas.push(TextArea {
                    buffer: buf,
                    left: 44.0,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
                y += lh;
            }

            // Phase / stage / practice: top-left, small caps.
            if phase_label.is_some() {
                areas.push(TextArea {
                    buffer: &self.phase_buf,
                    left: 44.0,
                    top: 36.0,
                    scale: 1.0,
                    bounds,
                    default_color: phase_color,
                    custom_glyphs: &[],
                });
            }

            if hud.pb_flash {
                let fw = line_width(&self.flash_buf);
                areas.push(TextArea {
                    buffer: &self.flash_buf,
                    left: (w - fw) * 0.5,
                    top: speed_top + 84.0,
                    scale: 1.0,
                    bounds,
                    default_color: TXT_ACCENT,
                    custom_glyphs: &[],
                });
            }

            // Sync, bottom-right above the perf line, with a hairline bar.
            if sync_label.is_some() {
                let sw = line_width(&self.sync_buf);
                let sy = h - 56.0 - 30.0;
                areas.push(TextArea {
                    buffer: &self.sync_buf,
                    left: w - 44.0 - sw,
                    top: sy,
                    scale: 1.0,
                    bounds,
                    default_color: TXT_SUB,
                    custom_glyphs: &[],
                });
                sync_bar = Some((
                    w - 44.0 - 160.0,
                    sy + 26.0,
                    160.0,
                    (hud.sync / 100.0).clamp(0.0, 1.0),
                ));
            }

            if keys_label.is_some() {
                let kw = line_width(&self.keys_buf);
                areas.push(TextArea {
                    buffer: &self.keys_buf,
                    left: (w - kw) * 0.5,
                    top: h - 56.0,
                    scale: 1.0,
                    bounds,
                    default_color: TXT_LABEL,
                    custom_glyphs: &[],
                });
            }
        }

        // Perf line stays visible over the menus too.
        if hud.perf_line.is_some() {
            let pw = line_width(&self.perf_buf);
            areas.push(TextArea {
                buffer: &self.perf_buf,
                left: (w - pw - 16.0).max(8.0),
                top: h - 28.0,
                scale: 1.0,
                bounds,
                default_color: TXT_DIM,
                custom_glyphs: &[],
            });
        }

        let mut verts: Vec<HudVert> = Vec::new();

        if let Some(ref page) = hud.page {
            let p = layout.panel;
            let text_left = p.x + PANEL_PAD_X;
            let value_right = p.x + p.w - PANEL_PAD_X;

            let inner_w = p.w - PANEL_PAD_X * 2.0;
            let label_x = text_left + CURSOR_INSET;

            // Backdrop pages own the frame; the pause overlay dims the world
            // first. Either way the panel is a translucent sheet of ground with
            // a hairline frame and four corner ticks — no fill colour of its
            // own, no accent rule.
            if !page.backdrop {
                push_rect_ndc(&mut verts, -1.0, -1.0, 2.0, 2.0, ground(0.60));
            }
            push_rect_px(&mut verts, p.x, p.y, p.w, p.h, w, h, ground(0.93));
            let frame = ink(0.12);
            push_rect_px(&mut verts, p.x, p.y, p.w, 1.0, w, h, frame);
            push_rect_px(&mut verts, p.x, p.y + p.h - 1.0, p.w, 1.0, w, h, frame);
            push_rect_px(&mut verts, p.x, p.y, 1.0, p.h, w, h, frame);
            push_rect_px(&mut verts, p.x + p.w - 1.0, p.y, 1.0, p.h, w, h, frame);
            let tick = 10.0;
            let tick_c = ink(1.0);
            for (cx, cy, dx, dy) in [
                (p.x, p.y, 1.0, 1.0),
                (p.x + p.w, p.y, -1.0, 1.0),
                (p.x, p.y + p.h, 1.0, -1.0),
                (p.x + p.w, p.y + p.h, -1.0, -1.0),
            ] {
                let hx = if dx > 0.0 { cx } else { cx - tick };
                let vy = if dy > 0.0 { cy } else { cy - tick };
                let hy = if dy > 0.0 { cy } else { cy - 1.0 };
                let vx = if dx > 0.0 { cx } else { cx - 1.0 };
                push_rect_px(&mut verts, hx, hy, tick, 1.0, w, h, tick_c);
                push_rect_px(&mut verts, vx, vy, 1.0, tick, w, h, tick_c);
            }

            // Tab strip: text with a hairline under the strip and a solid
            // underline under the active tab.
            if tab_count > 0 {
                push_rect_px(
                    &mut verts,
                    text_left,
                    layout.tabs[0].y + TAB_H - 1.0,
                    inner_w,
                    1.0,
                    w,
                    h,
                    ink(0.12),
                );
            }
            for (i, r) in layout.tabs.iter().take(tab_count).enumerate() {
                let active = i == page.tab;
                let hovered = page.tab_hovered == Some(i);
                let tw = line_width(&self.tab_bufs[i]);
                if active {
                    push_rect_px(&mut verts, r.x, r.y + r.h - 1.0, tw, 1.0, w, h, ink(1.0));
                }
                areas.push(TextArea {
                    buffer: &self.tab_bufs[i],
                    left: r.x,
                    top: r.y + 8.0,
                    scale: 1.0,
                    bounds,
                    default_color: if active {
                        TXT_TITLE
                    } else if hovered {
                        TXT_LABEL
                    } else {
                        TXT_DIM
                    },
                    custom_glyphs: &[],
                });
            }

            // Cursor: a 5px accent square in the gutter left of the label.
            // No selection band — the row reads as selected from the mark,
            // the brighter label and the accent value.
            let start = page.panel.scroll;
            let sel_drawn = page.panel.selected.checked_sub(start);
            if let Some(sd) = sel_drawn.filter(|d| *d < drawn_rows) {
                let selectable = page.panel.rows[page.panel.selected].kind.selectable();
                if selectable {
                    let sel_y = p.items_y0 + sd as f32 * p.row_h;
                    let a = if page.panel.focused { 1.0 } else { 0.45 };
                    push_rect_px(
                        &mut verts,
                        text_left,
                        sel_y + (p.row_h - 5.0) * 0.5,
                        5.0,
                        5.0,
                        w,
                        h,
                        accent(a),
                    );
                }
            }
            if let Some(hv) = page.panel.hovered {
                if let Some(hd) = hv.checked_sub(start).filter(|d| *d < drawn_rows) {
                    if hv != page.panel.selected && page.panel.rows[hv].kind.selectable() {
                        push_rect_px(
                            &mut verts,
                            text_left,
                            p.items_y0 + hd as f32 * p.row_h,
                            inner_w,
                            p.row_h,
                            w,
                            h,
                            ink(0.025),
                        );
                    }
                }
            }

            // Rows.
            let (track_x0, track_x1) = p.slider_track();
            for i in 0..drawn_rows {
                let row = &page.panel.rows[start + i];
                let y = p.items_y0 + i as f32 * p.row_h;
                let selected = start + i == page.panel.selected;
                match row.kind {
                    RowKind::Header => {
                        // Headers are small mono captions sitting low in their
                        // row, with no rule of their own.
                        areas.push(TextArea {
                            buffer: &self.label_bufs[i],
                            left: text_left,
                            top: y + 12.0,
                            scale: 1.0,
                            bounds,
                            default_color: TXT_HEADER,
                            custom_glyphs: &[],
                        });
                        continue;
                    }
                    RowKind::Slider(frac) => {
                        let ty = y + p.row_h * 0.5;
                        push_rect_px(
                            &mut verts,
                            track_x0,
                            ty,
                            track_x1 - track_x0,
                            1.0,
                            w,
                            h,
                            ink(0.14),
                        );
                        let fill = (track_x1 - track_x0) * frac.clamp(0.0, 1.0);
                        let c = if selected { accent(1.0) } else { ink(0.9) };
                        push_rect_px(&mut verts, track_x0, ty, fill.max(1.0), 1.0, w, h, c);
                        // Thumb: a hairline tick, 2px when it is the live row.
                        let tw = if selected { 2.0 } else { 1.0 };
                        push_rect_px(
                            &mut verts,
                            track_x0 + fill - tw * 0.5,
                            ty - 4.0,
                            tw,
                            9.0,
                            w,
                            h,
                            c,
                        );
                    }
                    _ => {}
                }

                // Hairline under every item row.
                push_rect_px(
                    &mut verts,
                    text_left,
                    y + p.row_h - 1.0,
                    inner_w,
                    1.0,
                    w,
                    h,
                    ink(0.045),
                );

                areas.push(TextArea {
                    buffer: &self.label_bufs[i],
                    left: label_x,
                    top: y + 4.0,
                    scale: 1.0,
                    bounds,
                    default_color: if selected { TXT_LABEL_SEL } else { TXT_LABEL },
                    custom_glyphs: &[],
                });
                if !row.note.is_empty() {
                    let lw = line_width(&self.label_bufs[i]);
                    areas.push(TextArea {
                        buffer: &self.note_bufs[i],
                        left: label_x + lw + 10.0,
                        top: y + 7.0,
                        scale: 1.0,
                        bounds,
                        default_color: TXT_DIM,
                        custom_glyphs: &[],
                    });
                }
                if !row.value.is_empty() {
                    let vw = line_width(&self.value_bufs[i]);
                    areas.push(TextArea {
                        buffer: &self.value_bufs[i],
                        left: value_right - vw,
                        top: y + 6.0,
                        scale: 1.0,
                        bounds,
                        default_color: match row.tone {
                            RowTone::Good => TXT_GOOD,
                            RowTone::Warn => TXT_WARN,
                            RowTone::Accent => TXT_ACCENT,
                            RowTone::Dim => TXT_DIM,
                            RowTone::Normal => {
                                if selected {
                                    TXT_ACCENT
                                } else {
                                    TXT_VALUE
                                }
                            }
                        },
                        custom_glyphs: &[],
                    });
                }
            }

            // Scroll indicator: a thumb on the right edge of the list.
            let total = page.panel.rows.len();
            if total > drawn_rows && drawn_rows > 0 {
                let track_h = drawn_rows as f32 * p.row_h;
                let thumb_h = (track_h * drawn_rows as f32 / total as f32).max(18.0);
                let max_scroll = (total - drawn_rows) as f32;
                let t = (start as f32 / max_scroll.max(1.0)).clamp(0.0, 1.0);
                push_rect_px(
                    &mut verts,
                    p.x + p.w - 8.0,
                    p.items_y0,
                    1.0,
                    track_h,
                    w,
                    h,
                    ink(0.10),
                );
                push_rect_px(
                    &mut verts,
                    p.x + p.w - 8.0,
                    p.items_y0 + (track_h - thumb_h) * t,
                    1.0,
                    thumb_h,
                    w,
                    h,
                    ink(0.8),
                );
            }

            // Title left, subtitle (mono, dim) on the same baseline at right.
            areas.push(TextArea {
                buffer: &self.page_title_buf,
                left: text_left,
                top: layout.title_y,
                scale: 1.0,
                bounds,
                default_color: TXT_TITLE,
                custom_glyphs: &[],
            });
            let sw = line_width(&self.page_sub_buf);
            areas.push(TextArea {
                buffer: &self.page_sub_buf,
                left: (value_right - sw).max(text_left),
                top: layout.title_y + 10.0,
                scale: 1.0,
                bounds,
                default_color: TXT_SUB,
                custom_glyphs: &[],
            });

            // Indeterminate progress: a lozenge sweeping the panel width. There
            // is no real progress signal from the loader, so pretending to one
            // would be a lie — this only says "still working".
            if page.busy {
                let bx = text_left;
                let bw = inner_w;
                push_rect_px(&mut verts, bx, layout.busy_y, bw, 1.0, w, h, ink(0.12));
                let seg = bw * 0.28;
                let phase = (hud.time * 0.55).fract();
                // Ease so it decelerates at the ends instead of wrapping harshly.
                let eased = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                push_rect_px(
                    &mut verts,
                    bx + (bw - seg) * eased,
                    layout.busy_y,
                    seg,
                    1.0,
                    w,
                    h,
                    ink(1.0),
                );
            }

            if page.message.is_some() {
                areas.push(TextArea {
                    buffer: &self.page_msg_buf,
                    left: text_left,
                    top: layout.message_y,
                    scale: 1.0,
                    bounds,
                    default_color: TXT_ERR,
                    custom_glyphs: &[],
                });
            }
            areas.push(TextArea {
                buffer: &self.page_hint_buf,
                left: text_left,
                top: layout.hint_y,
                scale: 1.0,
                bounds,
                default_color: TXT_HINT,
                custom_glyphs: &[],
            });

            // Button bar: a hairline above, text below it, the focused one
            // underlined in accent.
            if button_count > 0 {
                push_rect_px(
                    &mut verts,
                    p.x,
                    layout.buttons[0].y - 2.0,
                    p.w,
                    1.0,
                    w,
                    h,
                    ink(0.12),
                );
            }
            for (i, r) in layout.buttons.iter().take(button_count).enumerate() {
                let focused = page.button_selected == Some(i);
                let hovered = page.button_hovered == Some(i);
                let bwid = line_width(&self.button_bufs[i]);
                if focused {
                    push_rect_px(&mut verts, r.x, r.y + r.h - 8.0, bwid, 1.0, w, h, accent(1.0));
                }
                areas.push(TextArea {
                    buffer: &self.button_bufs[i],
                    left: r.x,
                    top: r.y + 10.0,
                    scale: 1.0,
                    bounds,
                    default_color: if focused {
                        TXT_TITLE
                    } else if hovered {
                        TXT_LABEL
                    } else {
                        TXT_SUB
                    },
                    custom_glyphs: &[],
                });
            }
        } else {
            for (bx, by, bw, bh) in washes {
                push_rect_px(&mut verts, bx, by, bw, bh, w, h, ground(0.30));
            }
            if let Some((bx, by, bw, frac)) = speed_bar {
                push_rect_px(&mut verts, bx, by, bw, 1.0, w, h, ink(0.14));
                push_rect_px(&mut verts, bx, by - 1.0, (bw * frac).max(1.0), 3.0, w, h, ink(1.0));
            }
            if let Some((bx, by, bw, frac)) = sync_bar {
                push_rect_px(&mut verts, bx, by, bw, 1.0, w, h, ink(0.14));
                push_rect_px(&mut verts, bx, by - 1.0, (bw * frac).max(1.0), 3.0, w, h, ink(1.0));
            }
            if crosshair {
                // 1px hairline cross, 14px, with a 2px gap at the centre.
                let cx = (w * 0.5).floor();
                let cy = (h * 0.5).floor();
                let c = ink(0.9);
                push_rect_px(&mut verts, cx - 7.0, cy, 5.0, 1.0, w, h, c);
                push_rect_px(&mut verts, cx + 3.0, cy, 5.0, 1.0, w, h, c);
                push_rect_px(&mut verts, cx, cy - 7.0, 1.0, 5.0, w, h, c);
                push_rect_px(&mut verts, cx, cy + 3.0, 1.0, 5.0, w, h, c);
            }
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
        // Procedural backdrop, then quads (panels/bars), then text.
        self.backdrop.draw(pass);
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

pub fn format_hud_time(secs: f32) -> String {
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

    fn spec(rows: usize) -> PageSpec {
        PageSpec {
            rows,
            tabs: 3,
            buttons: 4,
            wide: false,
        }
    }

    #[test]
    fn row_hit_test_matches_drawn_rows() {
        let l = page_layout(1920.0, 1080.0, spec(12));
        let p = l.panel;
        assert_eq!(p.rows, 12, "all 12 rows should fit at 1080p");
        for i in 0..p.rows {
            let y = p.items_y0 + (i as f32 + 0.5) * p.row_h;
            assert_eq!(p.row_at(p.x + 20.0, y), Some(i), "row {i}");
        }
        assert_eq!(p.row_at(p.x + 20.0, p.items_y0 - 1.0), None);
        let past = p.items_y0 + p.rows as f32 * p.row_h + 1.0;
        assert_eq!(p.row_at(p.x + 20.0, past), None);
        let y = p.items_y0 + p.row_h * 0.5;
        assert_eq!(p.row_at(p.x - 5.0, y), None);
        assert_eq!(p.row_at(p.x + p.w + 5.0, y), None);
    }

    /// The whole point of computing capacity from the window: a long list on a
    /// short window must scroll, not run off the bottom into the buttons.
    #[test]
    fn a_long_list_on_a_short_window_is_capped_above_the_buttons() {
        let l = page_layout(1280.0, 620.0, spec(40));
        let p = l.panel;
        assert!(p.rows < 40, "list should have been capped, got {}", p.rows);
        assert!(p.rows > 0, "at least some rows must be drawn");
        let list_bottom = p.items_y0 + p.rows as f32 * p.row_h;
        let first_button = l.buttons[0];
        assert!(
            list_bottom <= first_button.y,
            "rows ({list_bottom}) overlap the button bar ({})",
            first_button.y
        );
        assert!(
            first_button.y + first_button.h <= 620.0,
            "buttons fall off the bottom of the window"
        );
    }

    #[test]
    fn tabs_and_buttons_tile_the_panel_without_overlapping() {
        let l = page_layout(1600.0, 900.0, spec(10));
        for strip in [&l.tabs, &l.buttons] {
            for pair in strip.windows(2) {
                assert!(
                    pair[0].x + pair[0].w <= pair[1].x + 0.01,
                    "strip entries overlap"
                );
            }
            let first = strip.first().unwrap();
            let last = strip.last().unwrap();
            assert!(first.x >= l.panel.x, "strip starts outside the panel");
            assert!(
                last.x + last.w <= l.panel.x + l.panel.w + 0.01,
                "strip ends outside the panel"
            );
        }
    }

    #[test]
    fn the_panel_stays_on_screen_on_a_narrow_window() {
        let l = page_layout(700.0, 560.0, spec(20));
        assert!(l.panel.x >= 0.0);
        assert!(l.panel.x + l.panel.w <= 700.0);
        assert!(l.panel.y + l.panel.h <= 560.0 + 1.0);
    }

    /// Clicking a label must not move a slider — only the track half does.
    #[test]
    fn the_slider_track_is_the_right_half_of_the_row() {
        let p = page_layout(1600.0, 900.0, spec(6)).panel;
        let (x0, x1) = p.slider_track();
        assert!(x0 > p.x + p.w * 0.4, "track starts too far left");
        assert!(x1 <= p.x + p.w, "track runs past the panel");
        assert_eq!(
            p.slider_frac_at(p.x + 20.0),
            None,
            "label half must not drag"
        );
        assert_eq!(p.slider_frac_at(x0), Some(0.0));
        assert_eq!(p.slider_frac_at(x1), Some(1.0));
        let mid = p.slider_frac_at((x0 + x1) * 0.5).unwrap();
        assert!(
            (mid - 0.5).abs() < 0.01,
            "midpoint should be 0.5, got {mid}"
        );
    }

    #[test]
    fn headers_are_not_selectable_but_items_are() {
        assert!(!RowKind::Header.selectable());
        assert!(RowKind::Item.selectable());
        assert!(RowKind::Slider(0.5).selectable());
    }
}
