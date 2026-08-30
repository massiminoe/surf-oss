//! Neon polyline trail for ghost playback (on-ramp vs airborne).

use bytemuck::{Pod, Zeroable};

use surf_core::math::Vec3;

/// Lift trail above feet so it reads on ramps (≈ mid CS:S hull).
const TRAIL_Z_OFFSET: f32 = 28.0;

const SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> cam: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos.x, v.pos.z, -v.pos.y, 1.0);
    o.color = v.color;
    return o;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#;

/// One sample along a ghost path. `on_ramp` = grounded (surfing contact).
#[derive(Clone, Copy, Debug)]
pub struct TrailPoint {
    pub origin: Vec3,
    pub on_ramp: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TrailVert {
    pos: [f32; 3],
    color: [f32; 4],
}

fn neon_color(on_ramp: bool) -> [f32; 4] {
    if on_ramp {
        // Cyan — on ramp / grounded.
        [0.15, 0.95, 1.0, 0.90]
    } else {
        // Magenta — airborne / off ramp.
        [1.0, 0.25, 0.85, 0.90]
    }
}

pub struct TrailRenderer {
    pipeline: wgpu::RenderPipeline,
    vbo: wgpu::Buffer,
    capacity: u32,
    vertex_count: u32,
}

impl TrailRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trail"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trail_layout"),
            bind_group_layouts: &[camera_bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trail_pipe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TrailVert>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
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
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let capacity = 8_192u32;
        let vbo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trail_vbo"),
            size: (capacity as u64) * std::mem::size_of::<TrailVert>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vbo,
            capacity,
            vertex_count: 0,
        }
    }

    /// Replace trail geometry. Empty / single-point clears the draw.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, points: &[TrailPoint]) {
        if points.len() < 2 {
            self.vertex_count = 0;
            return;
        }

        // Subsample long ghosts so the VBO stays modest (~2 min @ 66 Hz).
        let step = ((points.len() / 6_000) + 1).max(1);
        let mut verts: Vec<TrailVert> = Vec::with_capacity(points.len() / step + 2);
        for (i, p) in points.iter().enumerate() {
            if i % step != 0 && i + 1 != points.len() {
                continue;
            }
            let o = p.origin + Vec3::new(0.0, 0.0, TRAIL_Z_OFFSET);
            verts.push(TrailVert {
                pos: [o.x, o.y, o.z],
                color: neon_color(p.on_ramp),
            });
        }
        if verts.len() < 2 {
            self.vertex_count = 0;
            return;
        }

        let need = verts.len() as u32;
        if need > self.capacity {
            let cap = need.next_power_of_two().max(self.capacity * 2);
            self.vbo = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trail_vbo"),
                size: (cap as u64) * std::mem::size_of::<TrailVert>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = cap;
        }

        queue.write_buffer(&self.vbo, 0, bytemuck::cast_slice(&verts));
        self.vertex_count = verts.len() as u32;
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera_bg: &'a wgpu::BindGroup) {
        if self.vertex_count < 2 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bg, &[]);
        pass.set_vertex_buffer(0, self.vbo.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}
