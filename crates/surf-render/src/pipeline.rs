use crate::camera::{Camera, CameraUniform};
use crate::hud::{HudRenderer, HudState};
use crate::mesh::{GpuMaterials, GpuMesh, MeshVertex};
use crate::skybox::SkyboxRenderer;
use surf_map::{LightmapAtlas, MaterialAtlas, SkyboxAtlas};
use wgpu::util::DeviceExt;

/// Per-frame readability knobs (CPU → fragment uniform).
///
/// A uniform block is 16-byte aligned, so the two live knobs are padded out
/// explicitly rather than left to whatever the driver assumes. `_pad` is
/// private: build one with [`ViewParams::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewParams {
    pub exposure: f32,
    pub shadow_lift: f32,
    _pad: [f32; 2],
}

impl ViewParams {
    pub fn new(exposure: f32, shadow_lift: f32) -> Self {
        Self {
            exposure,
            shadow_lift,
            _pad: [0.0; 2],
        }
    }
}

impl Default for ViewParams {
    fn default() -> Self {
        Self::new(1.0, 0.0)
    }
}

const SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> }
struct ViewParams {
    exposure: f32,
    shadow_lift: f32,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var albedo: texture_2d_array<f32>;
@group(1) @binding(1) var albedo_samp: sampler;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var lightmap_samp: sampler;
@group(2) @binding(0) var<uniform> params: ViewParams;

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
    @location(4) world_pos: vec3<f32>,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(v.position, 1.0);
    out.color = v.color;
    out.uv = v.uv;
    out.tex = v.tex;
    out.lm_uv = v.lm_uv;
    out.world_pos = v.position;
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let lm = textureSample(lightmap, lightmap_samp, v.lm_uv).rgb;
    // Lift dark luxels toward white without flattening bright areas as hard.
    let lifted = mix(lm, vec3<f32>(1.0, 1.0, 1.0), clamp(params.shadow_lift, 0.0, 1.0));
    let light = max(lifted, vec3<f32>(0.05, 0.05, 0.05));
    let layer = i32(v.tex + 0.5);
    var base: vec3<f32>;
    if (layer <= 0) {
        base = v.color * light;
    } else {
        let sample = textureSample(albedo, albedo_samp, v.uv, layer);
        // Cutout test. surf-map forces alpha to 1 on every layer whose VMT did
        // not declare $alphatest/$translucent, so this only bites foliage cards,
        // grates and fences — the materials that mean it.
        if (sample.a < 0.5) {
            discard;
        }
        base = sample.rgb * light * 2.0;
    }
    return vec4<f32>(base * params.exposure, 1.0);
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
    /// Kept so a map reload can rebuild the skybox, which binds the camera.
    camera_bind_group_layout: wgpu::BindGroupLayout,
    view_params_buffer: wgpu::Buffer,
    view_params_bind_group: wgpu::BindGroup,
    pub skybox: Option<SkyboxRenderer>,
    pub depth_view: wgpu::TextureView,
    pub mesh: GpuMesh,
    pub hud: HudRenderer,
    ghost: crate::ghost::GhostRenderer,
    trail: crate::trail::TrailRenderer,
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

        let view_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view_params"),
            contents: bytemuck::bytes_of(&ViewParams::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let view_params_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view_params_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let view_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view_params_bg"),
            layout: &view_params_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_params_buffer.as_entire_binding(),
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
            bind_group_layouts: &[
                &camera_bind_group_layout,
                &materials.bind_group_layout,
                &view_params_bgl,
            ],
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
                depth_compare: wgpu::CompareFunction::Greater,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let depth_view = create_depth_view(&device, config.width, config.height);
        let hud = HudRenderer::new(&device, &queue, config.format);
        let ghost =
            crate::ghost::GhostRenderer::new(&device, config.format, &camera_bind_group_layout);
        let trail =
            crate::trail::TrailRenderer::new(&device, config.format, &camera_bind_group_layout);

        Self {
            device,
            queue,
            config,
            pipeline,
            camera_buffer,
            camera_bind_group,
            camera_bind_group_layout,
            materials,
            view_params_buffer,
            view_params_bind_group,
            skybox,
            depth_view,
            mesh,
            hud,
            ghost,
            trail,
        }
    }

    /// Swap in a different map. Rebuilds only what is map-specific — mesh,
    /// material bind group, skybox — leaving the device, pipelines and HUD
    /// alone. The material bind group is rebuilt against the *existing* layout
    /// so the world pipeline keeps accepting it.
    pub fn load_level(
        &mut self,
        mesh: &surf_core::graybox::GrayboxMesh,
        atlas: &MaterialAtlas,
        lightmaps: &LightmapAtlas,
        sky: &SkyboxAtlas,
    ) {
        self.mesh = GpuMesh::from_graybox(&self.device, mesh);
        self.materials
            .reload(&self.device, &self.queue, atlas, lightmaps);
        self.skybox = crate::skybox::SkyboxRenderer::try_new(
            &self.device,
            &self.queue,
            self.config.format,
            &self.camera_bind_group_layout,
            sky,
        );
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
        ghost: Option<crate::ghost::GhostPose>,
        trail: Option<&[crate::trail::TrailPoint]>,
        view: ViewParams,
    ) -> Result<(), wgpu::SurfaceError> {
        self.update_frame(camera, ghost, trail, view);
        let frame = surface.get_current_texture()?;
        let view_tex = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self.encode_frame(&view_tex, Some(hud), ghost.is_some(), trail.is_some());
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Per-frame uniform / dynamic-buffer writes. Split out of `render` so the
    /// offscreen path (`offscreen::Offscreen`) shares the exact same state as
    /// the window path — a screenshot that diverged from the live frame would be
    /// worthless as a check.
    pub(crate) fn update_frame(
        &mut self,
        camera: &Camera,
        ghost: Option<crate::ghost::GhostPose>,
        trail: Option<&[crate::trail::TrailPoint]>,
        view: ViewParams,
    ) {
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&camera.uniform()),
        );
        self.queue
            .write_buffer(&self.view_params_buffer, 0, bytemuck::bytes_of(&view));
        if let Some(sky) = &self.skybox {
            sky.write_camera(&self.queue, &camera.sky_uniform());
        }
        if let Some(g) = ghost {
            self.ghost.update(&self.queue, g.origin, g.ducked);
        }
        match trail {
            Some(pts) if !pts.is_empty() => {
                self.trail.update(&self.device, &self.queue, pts);
            }
            _ => {
                self.trail.update(&self.device, &self.queue, &[]);
            }
        }
    }

    /// Encode one frame into `view_tex`. `hud` is `None` for offscreen captures
    /// that want the world only.
    pub(crate) fn encode_frame(
        &mut self,
        view_tex: &wgpu::TextureView,
        hud: Option<HudState>,
        draw_ghost: bool,
        draw_trail: bool,
    ) -> wgpu::CommandEncoder {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // A shell page paints its own opaque backdrop over the whole frame, so
        // the world behind it is invisible. Skipping it means sitting on the
        // main menu doesn't re-draw a 2.6M-triangle map every frame.
        let world_hidden = hud
            .as_ref()
            .and_then(|h| h.page.as_ref())
            .is_some_and(|p| p.backdrop);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: view_tex,
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
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if world_hidden {
                // Depth/colour are still cleared by the pass itself.
            } else {
                if let Some(sky) = &self.skybox {
                    sky.draw(&mut pass);
                }
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_bind_group(1, &self.materials.bind_group, &[]);
                pass.set_bind_group(2, &self.view_params_bind_group, &[]);
                pass.set_vertex_buffer(0, self.mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(self.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.mesh.index_count, 0, 0..1);
                if draw_trail {
                    self.trail.draw(&mut pass, &self.camera_bind_group);
                }
                if draw_ghost {
                    self.ghost.draw(&mut pass, &self.camera_bind_group);
                }
            }
        }

        if let Some(hud) = hud {
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
                        view: view_tex,
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
        }

        encoder
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
