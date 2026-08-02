use crate::camera::{Camera, CameraUniform};
use crate::hud::{HudRenderer, HudState};
use crate::mesh::{GpuMaterials, GpuMesh, MeshVertex};
use crate::skybox::SkyboxRenderer;
use surf_map::{LightmapAtlas, MaterialAtlas, SkyboxAtlas};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var albedo: texture_2d_array<f32>;
@group(1) @binding(1) var albedo_samp: sampler;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var lightmap_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tex: f32,
    @location(4) lm_uv: vec2<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tex: f32,
    @location(3) lm_uv: vec2<f32>,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(v.position, 1.0);
    out.color = v.color;
    out.uv = v.uv;
    out.tex = v.tex;
    out.lm_uv = v.lm_uv;
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let lm = textureSample(lightmap, lightmap_samp, v.lm_uv).rgb;
    // Stub 1×1 white lightmap → lm ≈ 1; real atlas darkens/lights surfaces.
    let light = max(lm, vec3<f32>(0.05, 0.05, 0.05));
    let layer = i32(v.tex + 0.5);
    if (layer <= 0) {
        return vec4<f32>(v.color * light, 1.0);
    }
    let sample = textureSample(albedo, albedo_samp, v.uv, layer);
    return vec4<f32>(sample.rgb * light * 2.0, 1.0);
}
"#;

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub pipeline: wgpu::RenderPipeline,
    pub camera_buffer: wgpu::Buffer,
    pub camera_bind_group: wgpu::BindGroup,
    pub materials: GpuMaterials,
    pub skybox: Option<SkyboxRenderer>,
    pub depth_view: wgpu::TextureView,
    pub mesh: GpuMesh,
    pub hud: HudRenderer,
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        mesh: GpuMesh,
        atlas: &MaterialAtlas,
        lightmaps: &LightmapAtlas,
        sky: &SkyboxAtlas,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("textured"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&CameraUniform {
                view_proj: [[0.0; 4]; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera_bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera_bg"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let materials = GpuMaterials::upload(&device, &queue, atlas, lightmaps);
        let skybox = SkyboxRenderer::try_new(
            &device,
            &queue,
            config.format,
            &camera_bind_group_layout,
            sky,
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipe_layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &materials.bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("textured_pipe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
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
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 32,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Float32,
                        },
                        wgpu::VertexAttribute {
                            offset: 36,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let depth_view = create_depth_view(&device, config.width, config.height);
        let hud = HudRenderer::new(&device, &queue, config.format);

        Self {
            device,
            queue,
            config,
            pipeline,
            camera_buffer,
            camera_bind_group,
            materials,
            skybox,
            depth_view,
            mesh,
            hud,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.depth_view = create_depth_view(&self.device, width, height);
    }

    pub fn render(
        &mut self,
        surface: &wgpu::Surface<'_>,
        camera: &Camera,
        hud: HudState,
    ) -> Result<(), wgpu::SurfaceError> {
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&camera.uniform()),
        );
        if let Some(sky) = &self.skybox {
            sky.write_camera(&self.queue, &camera.sky_uniform());
        }

        let frame = surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.45,
                            g: 0.62,
                            b: 0.85,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(sky) = &self.skybox {
                sky.draw(&mut pass);
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.materials.bind_group, &[]);
            pass.set_vertex_buffer(0, self.mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(self.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.mesh.index_count, 0, 0..1);
        }

        self.hud.prepare(
            &self.device,
            &self.queue,
            self.config.width,
            self.config.height,
            hud,
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.hud.render(&mut pass);
        }
        self.hud.trim();

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    depth.create_view(&wgpu::TextureViewDescriptor::default())
}
