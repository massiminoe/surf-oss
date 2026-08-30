//! 2D Source skybox: six faces as a cube at infinity (rotation-only view).

use surf_map::SkyboxAtlas;
use wgpu::util::DeviceExt;

use crate::camera::CameraUniform;
use crate::mesh::MeshVertex;

const SKY_SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sky: texture_2d_array<f32>;
@group(1) @binding(1) var sky_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tex: f32,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tex: f32,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    var clip = camera.view_proj * vec4<f32>(v.position, 1.0);
    // Force depth to far plane.
    clip.z = 0.0; // far plane under reversed-Z
    out.clip = clip;
    out.uv = v.uv;
    out.tex = v.tex;
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let layer = i32(v.tex + 0.5);
    return textureSample(sky, sky_samp, v.uv, layer);
}
"#;

pub struct SkyboxRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    tex_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl SkyboxRenderer {
    pub fn try_new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        atlas: &SkyboxAtlas,
    ) -> Option<Self> {
        if atlas.is_empty() {
            return None;
        }
        let size = atlas.layer_size;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky_array"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky_samp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let tex_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky_bg"),
            layout: &tex_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_camera"),
            contents: bytemuck::bytes_of(&CameraUniform {
                view_proj: [[0.0; 4]; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky_cam_bg"),
            layout: camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky"),
            source: wgpu::ShaderSource::Wgsl(SKY_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky_layout"),
            bind_group_layouts: &[camera_bgl, &tex_bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky_pipe"),
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
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The cube is drawn from the inside, so the faces we must keep are the
                // ones whose winding reads as front-facing from the eye. Culling
                // `Front` here discarded all six and left the pass clear colour
                // showing through as a flat blue "sky" on every map.
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let (verts, indices) = sky_cube_mesh();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_verts"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_idx"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Some(Self {
            pipeline,
            camera_buffer,
            camera_bind_group,
            tex_bind_group,
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        })
    }

    pub fn write_camera(&self, queue: &wgpu::Queue, sky_uniform: &CameraUniform) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(sky_uniform));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.tex_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// Unit cube in Y-up render space. Layers: 0=ft 1=bk 2=lf 3=rt 4=up 5=dn.
fn sky_cube_mesh() -> (Vec<MeshVertex>, Vec<u32>) {
    let s = 1.0f32;
    // Face quads: winding is front-facing from inside the cube (cull Back).
    // Positions in Y-up: x right, y up, z back (after Source→Y-up, +Y source → -Z).
    // Axis-aligned cube in Y-up space. Layers: ft,bk,lf,rt,up,dn.
    //
    // The per-face UVs are Source's own sky orientation, not a guess: the cube
    // is viewed from the *inside*, so all four side faces read mirrored in u,
    // and the two caps are turned a quarter-turn relative to them. Measured by
    // sampling both faces along each of the 12 shared cube edges and minimising
    // the seam mismatch over all 8 orientations per face — the same assignment
    // came out optimal on every map with a detailed sky (cyberwave 19.3 -> 1.95
    // mean mismatch out of 255, lovetunnel 15.0 -> 1.35, frost 22.9 -> 2.51,
    // nyx tied-best; summit and boreas are smooth gradients and score ~0 either
    // way, which is exactly why this went unnoticed).
    let faces: [([[f32; 3]; 4], u32); 6] = [
        ([[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s]], 0), // ft
        ([[s, -s, s], [-s, -s, s], [-s, s, s], [s, s, s]], 1),     // bk
        ([[-s, -s, s], [-s, -s, -s], [-s, s, -s], [-s, s, s]], 2), // lf
        ([[s, -s, -s], [s, -s, s], [s, s, s], [s, s, -s]], 3),     // rt
        ([[-s, s, -s], [s, s, -s], [s, s, s], [-s, s, s]], 4),     // up
        ([[-s, -s, s], [s, -s, s], [s, -s, -s], [-s, -s, -s]], 5), // dn
    ];
    // Sides: (u,v) -> (1-u, v).  up: -> (v, u).  dn: -> (1-v, 1-u).
    let side_uvs = [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]];
    let up_uvs = [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
    let dn_uvs = [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for (corners, layer) in faces {
        let uvs = match layer {
            4 => up_uvs,
            5 => dn_uvs,
            _ => side_uvs,
        };
        let base = verts.len() as u32;
        for i in 0..4 {
            verts.push(MeshVertex {
                position: corners[i],
                color: [1.0, 1.0, 1.0],
                uv: uvs[i],
                tex: layer as f32,
                lm_uv: [0.0, 0.0],
                _pad: 0.0,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample the atlas the way the fragment shader does: uv (0,0) is the
    /// top-left of the layer's image.
    fn texel(atlas: &SkyboxAtlas, layer: usize, u: f32, v: f32) -> [f32; 3] {
        let n = atlas.layer_size as usize;
        let col = ((u.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1);
        let row = ((v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1);
        let i = (layer * n * n + row * n + col) * 4;
        [
            atlas.rgba[i] as f32,
            atlas.rgba[i + 1] as f32,
            atlas.rgba[i + 2] as f32,
        ]
    }

    /// Where does `p` land in this face's uv parameterisation? The quad is an
    /// axis-aligned rectangle, so projecting onto its two edge vectors is exact.
    fn uv_at(quad: &[MeshVertex; 4], p: [f32; 3]) -> (f32, f32) {
        let c0 = quad[0].position;
        let e1 = sub(quad[1].position, c0);
        let e2 = sub(quad[3].position, c0);
        let d = sub(p, c0);
        let a = dot(d, e1) / dot(e1, e1);
        let b = dot(d, e2) / dot(e2, e2);
        let u = quad[0].uv[0]
            + a * (quad[1].uv[0] - quad[0].uv[0])
            + b * (quad[3].uv[0] - quad[0].uv[0]);
        let v = quad[0].uv[1]
            + a * (quad[1].uv[1] - quad[0].uv[1])
            + b * (quad[3].uv[1] - quad[0].uv[1]);
        (u, v)
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// The six sky faces must line up where the cube folds.
    ///
    /// This is the check that caught the orientation being wrong: every face
    /// decoded fine and the cube drew, but the side faces were un-mirrored and
    /// the caps a quarter-turn out, so the nebula visibly stepped along the
    /// cube edges. Sampling both faces along each shared edge measures exactly
    /// that, and it is invisible to any test that only asks "is there sky?".
    #[test]
    fn sky_cube_faces_line_up_along_their_shared_edges() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/maps/surf_cyberwave.bsp");
        if !path.exists() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }
        let map = surf_map::LoadedMap::load_path(&path).expect("load map");
        let atlas = &map.skybox;
        assert!(!atlas.is_empty(), "cyberwave ships a full custom skybox");

        let (verts, _) = sky_cube_mesh();
        let quads: Vec<[MeshVertex; 4]> = verts
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        assert_eq!(quads.len(), 6);

        let key = |p: [f32; 3]| {
            (
                (p[0] * 8.0).round() as i32,
                (p[1] * 8.0).round() as i32,
                (p[2] * 8.0).round() as i32,
            )
        };

        let mut edges = 0usize;
        let mut total = 0.0f64;
        for a in 0..6 {
            for b in (a + 1)..6 {
                let shared: Vec<[f32; 3]> = quads[a]
                    .iter()
                    .filter(|va| {
                        quads[b]
                            .iter()
                            .any(|vb| key(vb.position) == key(va.position))
                    })
                    .map(|v| v.position)
                    .collect();
                if shared.len() != 2 {
                    continue;
                }
                edges += 1;
                let (p, q) = (shared[0], shared[1]);
                let n = 128;
                let mut sum = 0.0f64;
                for i in 0..n {
                    // Stay just off the corners: they are shared by three faces.
                    let t = 0.02 + 0.96 * (i as f32) / (n - 1) as f32;
                    let pt = [
                        p[0] + t * (q[0] - p[0]),
                        p[1] + t * (q[1] - p[1]),
                        p[2] + t * (q[2] - p[2]),
                    ];
                    let (ua, va) = uv_at(&quads[a], pt);
                    let (ub, vb) = uv_at(&quads[b], pt);
                    let ca = texel(atlas, quads[a][0].tex as usize, ua, va);
                    let cb = texel(atlas, quads[b][0].tex as usize, ub, vb);
                    sum += ((ca[0] - cb[0]).abs() + (ca[1] - cb[1]).abs() + (ca[2] - cb[2]).abs())
                        as f64
                        / 3.0;
                }
                total += sum / n as f64;
            }
        }
        assert_eq!(edges, 12, "a cube has 12 edges");
        let mean = total / edges as f64;
        // Measured 1.95/255 with the correct orientation; 19.33 with the side
        // faces un-mirrored and the caps turned the wrong way.
        assert!(
            mean < 6.0,
            "sky faces do not meet at the cube edges: mean mismatch {mean:.2}/255 \
             (correct orientation measures ~1.95; ~19.3 means the per-face UVs are wrong)"
        );
    }
}
