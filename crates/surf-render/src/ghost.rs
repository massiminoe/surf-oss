//! Subtle wireframe CS:S hull for PB ghost playback.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use surf_core::math::Vec3;
use surf_core::movement::Hull;

const SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> cam: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos.x, v.pos.z, -v.pos.y, 1.0);
    return o;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Muted cool wire — low alpha so overlaps barely tint the view.
    return vec4<f32>(0.55, 0.72, 0.85, 0.22);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GhostVert {
    pos: [f32; 3],
}

pub struct GhostRenderer {
    pipeline: wgpu::RenderPipeline,
    vbo: wgpu::Buffer,
    ibo: wgpu::Buffer,
    index_count: u32,
}

impl GhostRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ghost"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ghost_layout"),
            bind_group_layouts: &[camera_bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ghost_pipe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GhostVert>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    }],
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
                topology: wgpu::PrimitiveTopology::LineList,
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

        let hull = Hull::css_stand();
        let (verts, inds) = hull_wire_mesh(Vec3::ZERO, hull);
        let vbo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ghost_vbo"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let ibo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ghost_ibo"),
            contents: bytemuck::cast_slice(&inds),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipeline,
            vbo,
            ibo,
            index_count: inds.len() as u32,
        }
    }

    pub fn update(&self, queue: &wgpu::Queue, origin: Vec3, ducked: bool) {
        let hull = if ducked {
            Hull::css_duck()
        } else {
            Hull::css_stand()
        };
        let (verts, _) = hull_wire_mesh(origin, hull);
        queue.write_buffer(&self.vbo, 0, bytemuck::cast_slice(&verts));
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera_bg: &'a wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bg, &[]);
        pass.set_vertex_buffer(0, self.vbo.slice(..));
        pass.set_index_buffer(self.ibo.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// Optional ghost pose passed into [`crate::pipeline::Renderer::render`].
#[derive(Clone, Copy, Debug)]
pub struct GhostPose {
    pub origin: Vec3,
    pub ducked: bool,
}

fn hull_wire_mesh(origin: Vec3, hull: Hull) -> (Vec<[f32; 3]>, Vec<u16>) {
    let mins = origin + hull.mins;
    let maxs = origin + hull.maxs;
    let corners = [
        [mins.x, mins.y, mins.z],
        [maxs.x, mins.y, mins.z],
        [maxs.x, maxs.y, mins.z],
        [mins.x, maxs.y, mins.z],
        [mins.x, mins.y, maxs.z],
        [maxs.x, mins.y, maxs.z],
        [maxs.x, maxs.y, maxs.z],
        [mins.x, maxs.y, maxs.z],
    ];
    // 12 edges of the AABB as a line list.
    let inds: Vec<u16> = vec![
        0, 1, 1, 2, 2, 3, 3, 0, // bottom
        4, 5, 5, 6, 6, 7, 7, 4, // top
        0, 4, 1, 5, 2, 6, 3, 7, // verticals
    ];
    (corners.to_vec(), inds)
}
