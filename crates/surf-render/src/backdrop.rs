//! Procedural full-screen background for the shell pages (title screen, map
//! picker, settings, leaderboard, loading).
//!
//! The shell used to dim the live world behind it at 90% opacity, which meant
//! the "menu background" was whatever map happened to be loaded — the graybox
//! arena on a cold start. This draws a real one instead: a dark blueprint
//! backdrop, in the direction locked for the world art (off-black ink, one
//! accent, ordered dither reserved for the sky) so the menus and the eventual
//! Blueprint look are the same family.
//!
//! Everything is analytic in the fragment shader — no textures, no vertex
//! buffer, one fullscreen triangle. Cost is a single dependent-free pass.

/// Time / resolution / fade, pushed once per frame.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct BackdropUniform {
    time: f32,
    width: f32,
    height: f32,
    /// 0 = invisible, 1 = fully drawn. Lets a page fade the backdrop in.
    fade: f32,
}

const SHADER: &str = r#"
struct U {
    time: f32,
    width: f32,
    height: f32,
    fade: f32,
}
@group(0) @binding(0) var<uniform> u: U;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen triangle: no vertex buffer, three synthesized corners.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = pos[vi];
    var o: VsOut;
    o.clip = vec4<f32>(p, 0.0, 1.0);
    // uv: 0,0 top-left → 1,1 bottom-right.
    o.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return o;
}

// 4x4 ordered Bayer, 0..1. The dither is deliberately confined to the sky —
// that is the rule the art direction locks, and it is also the only place a
// smooth gradient bands badly on an 8-bit target.
fn bayer4(p: vec2<f32>) -> f32 {
    let x = i32(p.x) & 3;
    let y = i32(p.y) & 3;
    var m = array<f32, 16>(
         0.0,  8.0,  2.0, 10.0,
        12.0,  4.0, 14.0,  6.0,
         3.0, 11.0,  1.0,  9.0,
        15.0,  7.0, 13.0,  5.0,
    );
    return m[y * 4 + x] / 16.0;
}

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q += dot(q, q + 45.32);
    return fract(q.x * q.y);
}

/// Signed distance to a box rotated by `rot` radians about `centre`.
fn sd_ramp(p: vec2<f32>, centre: vec2<f32>, half: vec2<f32>, rot: f32) -> f32 {
    let c = cos(rot);
    let s = sin(rot);
    let d = p - centre;
    let q = abs(vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c)) - half;
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0);
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let aspect = max(u.width, 1.0) / max(u.height, 1.0);
    // Aspect-corrected space so the shapes don't stretch on a wide window.
    let p = vec2<f32>((v.uv.x - 0.5) * aspect, v.uv.y - 0.5);
    let px = vec2<f32>(v.uv.x * u.width, v.uv.y * u.height);
    let t = u.time;

    // Linear values for the intended sRGB swatches — the target is an sRGB
    // surface, so writing #0A0D12 as 0.039 would come out three stops light.
    let ink      = vec3<f32>(0.00304, 0.00402, 0.00605); // #0A0D12
    let ink_high = vec3<f32>(0.00802, 0.01161, 0.01938); // #161C26
    let accent   = vec3<f32>(0.02843, 0.52100, 0.21223); // #2FBF7F

    let horizon = 0.66;

    // --- sky: vertical gradient, dithered ---
    var col = mix(ink, ink_high, smoothstep(-0.15, horizon, v.uv.y));
    // A wide, soft glow sitting on the horizon, off to one side so the
    // composition isn't symmetric.
    let glow_c = vec2<f32>(0.20 * aspect, horizon - 0.5);
    let gd = p - glow_c;
    col += accent * exp(-dot(gd, gd) * 7.0) * 0.030;
    // Ordered dither, sky only. At these levels a smooth gradient bands hard on
    // an 8-bit target; a ±1/255 pattern is the cheapest honest fix and it is
    // the texture the art direction reserves for the sky anyway.
    if v.uv.y < horizon {
        col += (bayer4(px) - 0.5) * 0.0016;
    }

    // --- receding blueprint grid below the horizon ---
    // Derivatives must be evaluated in uniform control flow, so the grid is
    // computed for every pixel and masked afterwards rather than branched on.
    let d = max(v.uv.y - horizon, 0.0009);
    let z = 0.9 / d;
    let gx = (v.uv.x - 0.5) * z * 3.0;
    let gz = z - t * 0.5;
    let wx = fwidth(gx);
    let wz = fwidth(gz);
    // Distance to the nearest cell boundary, in cell units: 0 on a line,
    // 0.5 at a cell centre. Testing the centre instead (the easy sign slip)
    // fills every cell solid and reads as a flat wash, not a grid.
    let ex = 0.5 - abs(fract(gx) - 0.5);
    let ez = 0.5 - abs(fract(gz) - 0.5);
    let lx = 1.0 - smoothstep(0.0, wx, ex);
    let lz = 1.0 - smoothstep(0.0, wz, ez);
    // Once a cell is narrower than a pixel the "line" saturates to solid — that
    // is aliasing, not geometry, so fade the grid out as it becomes unresolvable
    // rather than letting it wash the lower half green.
    let resolve = (1.0 - smoothstep(0.08, 0.35, wx)) * (1.0 - smoothstep(0.08, 0.35, wz));
    let line = clamp(max(lx, lz), 0.0, 1.0) * resolve;
    let below = step(horizon, v.uv.y);
    // Strongest near the viewer; the only fade is at the horizon, where the
    // cells stop being resolvable.
    let gfade = below * smoothstep(0.0, 0.03, d);
    col += accent * line * gfade * 0.085;

    // --- horizon rule ---
    let hl = 1.0 - smoothstep(0.0, 2.0 / u.height, abs(v.uv.y - horizon));
    col += accent * hl * 0.09;

    // --- ramp wedges, drifting slowly in parallax ---
    // Three depth layers: the far one is barely separated from the sky, the
    // near one is a hard silhouette. Same idea as the map behind a real surf
    // horizon, without pretending to be a render of one.
    let drift = sin(t * 0.05) * 0.010;
    var centres = array<vec2<f32>, 3>(
        vec2<f32>(-0.44 * aspect + drift,        0.11),
        vec2<f32>( 0.42 * aspect - drift * 0.7,  0.20),
        vec2<f32>(-0.02 * aspect + drift * 0.4,  0.31),
    );
    var halves = array<vec2<f32>, 3>(
        vec2<f32>(0.24, 0.021),
        vec2<f32>(0.20, 0.018),
        vec2<f32>(0.16, 0.015),
    );
    var rots = array<f32, 3>(-0.34, 0.30, -0.18);
    var edges = array<f32, 3>(0.34, 0.24, 0.17);
    for (var i = 0; i < 3; i = i + 1) {
        let sd = sd_ramp(p, centres[i], halves[i], rots[i]);
        let body = 1.0 - smoothstep(0.0, 0.003, sd);
        col = mix(col, ink * 0.4, body * 0.9);
        // Accent edge: on the world the accent marks surf ramps and nothing
        // else, and the same rule holds here.
        let edge = 1.0 - smoothstep(0.0012, 0.0042, abs(sd));
        col += accent * edge * edges[i];
    }

    // --- grain + vignette ---
    col += (hash21(px + floor(t * 12.0)) - 0.5) * 0.0012;
    let vig = 1.0 - smoothstep(0.30, 0.95, length(vec2<f32>(p.x / max(aspect, 0.001), p.y)) * 1.6);
    col *= mix(0.35, 1.0, vig);

    return vec4<f32>(max(col, vec3<f32>(0.0)), clamp(u.fade, 0.0, 1.0));
}
"#;

pub struct Backdrop {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Set by `prepare`; `draw` is a no-op while zero.
    fade: f32,
}

impl Backdrop {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop_u"),
            size: std::mem::size_of::<BackdropUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop_pipe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
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
        Self {
            pipeline,
            uniform,
            bind_group,
            fade: 0.0,
        }
    }

    pub fn prepare(&mut self, queue: &wgpu::Queue, time: f32, width: u32, height: u32, fade: f32) {
        self.fade = fade.clamp(0.0, 1.0);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&BackdropUniform {
                time,
                width: width.max(1) as f32,
                height: height.max(1) as f32,
                fade: self.fade,
            }),
        );
    }

    pub fn draw<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.fade <= 0.0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
