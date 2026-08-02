//! Modern HUD: glyphon text (JetBrains Mono) + minimal speed/sync bars.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

const FONT_MEDIUM: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
const FONT_FAMILY: &str = "JetBrains Mono";

#[derive(Clone, Copy, Debug)]
pub struct HudState {
    /// Horizontal speed u/s.
    pub speed: f32,
    /// 0..100 air-strafe sync estimate.
    pub sync: f32,
    pub grounded: bool,
    /// Speed bar full-scale (display only).
    pub speed_scale: f32,
    /// Elapsed run time (seconds). `None` hides the timer line.
    pub time_secs: Option<f32>,
    /// PB delta seconds (negative = ahead). Shown when `Some`.
    pub pb_delta_secs: Option<f32>,
    /// Finished run — gold timer tint.
    pub timer_finished: bool,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            speed: 0.0,
            sync: 0.0,
            grounded: false,
            speed_scale: 3500.0,
            time_secs: None,
            pb_delta_secs: None,
            timer_finished: false,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HudVert {
    pos: [f32; 2],
    color: [f32; 3],
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
        let time_buf = Buffer::new(&mut font_system, Metrics::new(22.0, 28.0));
        let delta_buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud_bars"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec3<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
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
    return vec4<f32>(v.color, 0.85);
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
                            format: wgpu::VertexFormat::Float32x3,
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

        let bar_capacity = 256u32;
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

        let speed_label = format!("{:.0}", hud.speed);
        let time_label = hud.time_secs.map(format_hud_time);
        let delta_label = hud.pb_delta_secs.map(format_hud_delta);

        self.speed_buf
            .set_size(&mut self.font_system, Some(w), Some(40.0));
        self.speed_buf.set_text(
            &mut self.font_system,
            &speed_label,
            attrs,
            Shaping::Advanced,
        );
        self.speed_buf
            .shape_until_scroll(&mut self.font_system, false);

        if let Some(ref t) = time_label {
            self.time_buf
                .set_size(&mut self.font_system, Some(w), Some(32.0));
            self.time_buf
                .set_text(&mut self.font_system, t, attrs, Shaping::Advanced);
            self.time_buf
                .shape_until_scroll(&mut self.font_system, false);
        }
        if let Some(ref d) = delta_label {
            self.delta_buf
                .set_size(&mut self.font_system, Some(w), Some(24.0));
            self.delta_buf
                .set_text(&mut self.font_system, d, attrs, Shaping::Advanced);
            self.delta_buf
                .shape_until_scroll(&mut self.font_system, false);
        }

        let speed_color = if hud.grounded {
            Color::rgb(245, 245, 247)
        } else {
            Color::rgb(140, 210, 255)
        };
        let time_color = if hud.timer_finished {
            Color::rgb(255, 210, 90)
        } else {
            Color::rgb(230, 232, 238)
        };
        let delta_color = match hud.pb_delta_secs {
            Some(d) if d < 0.0 => Color::rgb(110, 230, 140),
            Some(d) if d > 0.0 => Color::rgb(240, 110, 110),
            _ => Color::rgb(180, 180, 190),
        };

        let top_pad = (h * 0.06).max(28.0);
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };

        let speed_w = line_width(&self.speed_buf);
        let time_w = line_width(&self.time_buf);
        let delta_w = line_width(&self.delta_buf);

        let mut areas = Vec::with_capacity(3);
        areas.push(TextArea {
            buffer: &self.speed_buf,
            left: (w - speed_w) * 0.5,
            top: top_pad,
            scale: 1.0,
            bounds,
            default_color: speed_color,
            custom_glyphs: &[],
        });

        let mut y = top_pad + 34.0;
        if time_label.is_some() {
            areas.push(TextArea {
                buffer: &self.time_buf,
                left: (w - time_w) * 0.5,
                top: y,
                scale: 1.0,
                bounds,
                default_color: time_color,
                custom_glyphs: &[],
            });
            y += 28.0;
        }
        if delta_label.is_some() {
            areas.push(TextArea {
                buffer: &self.delta_buf,
                left: (w - delta_w) * 0.5,
                top: y,
                scale: 1.0,
                bounds,
                default_color: delta_color,
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
        push_rect(&mut verts, -0.96, -0.94, 0.42, 0.012, [0.12, 0.12, 0.14]);
        let speed_t = (hud.speed / hud.speed_scale.max(1.0)).clamp(0.0, 1.0);
        let speed_bar = if hud.grounded {
            [0.40, 0.78, 0.55]
        } else {
            [0.35, 0.70, 0.95]
        };
        push_rect(&mut verts, -0.96, -0.94, 0.42 * speed_t, 0.012, speed_bar);
        push_rect(&mut verts, -0.96, -0.91, 0.42, 0.008, [0.12, 0.12, 0.14]);
        let sync_t = (hud.sync / 100.0).clamp(0.0, 1.0);
        push_rect(
            &mut verts,
            -0.96,
            -0.91,
            0.42 * sync_t,
            0.008,
            [0.92, 0.72, 0.28],
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
        let _ = self
            .text_renderer
            .render(&self.atlas, &self.viewport, pass);

        if self.bar_vert_count > 0 {
            pass.set_pipeline(&self.bar_pipeline);
            pass.set_vertex_buffer(0, self.bar_vbo.slice(..));
            pass.draw(0..self.bar_vert_count, 0..1);
        }
    }

    pub fn trim(&mut self) {
        self.atlas.trim();
    }
}

fn line_width(buf: &Buffer) -> f32 {
    buf.layout_runs()
        .map(|run| run.line_w)
        .fold(0.0f32, f32::max)
}

fn push_rect(out: &mut Vec<HudVert>, x: f32, y: f32, w: f32, h: f32, color: [f32; 3]) {
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
