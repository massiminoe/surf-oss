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
//! Direction "Instrument" (Max, 2026-09-02): a one-point-perspective grid with
//! a horizon and ruled major rows, and a dithered dot-field sky. The ramp
//! section that used to stand on the plane was dropped (Max, 2026-09-03).
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


@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let aspect = max(u.width, 1.0) / max(u.height, 1.0);
    // Aspect-corrected space so the shapes don't stretch on a wide window.
    let p = vec2<f32>((v.uv.x - 0.5) * aspect, v.uv.y - 0.5);
    let px = vec2<f32>(v.uv.x * u.width, v.uv.y * u.height);
    let t = u.time;
    let one_px = 1.0 / max(u.height, 1.0);

    // Linear values for the intended sRGB swatches — the target is an sRGB
    // surface, so writing #0C1012 as 0.047 would come out three stops light.
    let ground = vec3<f32>(0.00335, 0.00518, 0.00605); // #0C1012
    let ink    = vec3<f32>(0.82279, 0.87137, 0.84652); // #E9EEEC
    let accent = vec3<f32>(0.02843, 0.52100, 0.21223); // #2FBF7F

    // The horizon sits a little above centre so the ground plane carries the
    // composition and the panels float against the quiet sky.
    let horizon = 0.54;
    var col = ground;

    // --- sky: a sparse ordered dot field that thins toward the horizon ---
    // 1-bit dither is the one texture the art direction reserves for the sky.
    // Sampled on a 4px lattice so the dots read as a pattern, not as noise.
    let cell = floor(px / 4.0);
    let on_lattice = step(0.5, 1.0 - abs(fract(px.x / 4.0 + 0.5) * 2.0 - 1.0))
        * step(0.5, 1.0 - abs(fract(px.y / 4.0 + 0.5) * 2.0 - 1.0));
    let alt = fract(cell.y * 0.5) * 2.0;           // checkerboard offset row
    let lx = fract(cell.x * 0.5 + alt * 0.5) * 2.0; // 0 or 1
    let height = clamp(1.0 - v.uv.y / horizon, 0.0, 1.0);
    let density = height * height * 0.38;
    let dot_on = step(hash21(cell), density) * lx;
    let sky = step(v.uv.y, horizon);
    col += ink * dot_on * on_lattice * sky * 0.07;

    // --- ground plane: a true one-point perspective grid ---
    // Derivatives must be evaluated in uniform control flow, so the grid is
    // computed for every pixel and masked afterwards rather than branched on.
    let d = max(v.uv.y - horizon, 0.0009);
    let z = 0.42 / d;                    // depth, 1 = nearest row
    let gx = (v.uv.x - 0.5) * z * 4.0;   // lateral, in cells
    let gz = z - t * 0.35;               // rows glide toward the viewer
    let wx = fwidth(gx);
    let wz = fwidth(gz);
    // Distance to the nearest cell boundary, in cell units: 0 on a line,
    // 0.5 at a cell centre. Testing the centre instead (the easy sign slip)
    // fills every cell solid and reads as a flat wash, not a grid.
    let ex = 0.5 - abs(fract(gx) - 0.5);
    let ez = 0.5 - abs(fract(gz) - 0.5);
    let lx_ = 1.0 - smoothstep(0.0, wx * 1.2, ex);
    let lz_ = 1.0 - smoothstep(0.0, wz * 1.2, ez);
    // Every fifth depth row is a ruled major line, like a tick on a scale.
    let major = step(0.5 - abs(fract(gz / 5.0) - 0.5), wz * 0.24);
    // Once a cell is narrower than a pixel the "line" saturates to solid — that
    // is aliasing, not geometry, so fade the grid out as it becomes unresolvable
    // rather than letting it wash the horizon flat.
    let resolve = (1.0 - smoothstep(0.10, 0.45, wx)) * (1.0 - smoothstep(0.10, 0.45, wz));
    let below = 1.0 - sky;
    let line = clamp(max(lx_ * 0.85, lz_ * (0.8 + major * 1.4)), 0.0, 2.2) * resolve;
    col += ink * line * below * 0.085;

    // --- horizon rule ---
    let hl = 1.0 - smoothstep(0.0, 1.5 * one_px, abs(v.uv.y - horizon));
    col += ink * hl * 0.22;

    // --- vignette ---
    let vig = 1.0 - smoothstep(0.40, 1.05, length(vec2<f32>(p.x / max(aspect, 0.001), p.y)) * 1.6);
    col *= mix(0.55, 1.0, vig);

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
