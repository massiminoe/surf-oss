use surf_core::graybox::GrayboxMesh;
use surf_core::math::Vec3;
use surf_map::{LightmapAtlas, MaterialAtlas};
use wgpu::util::DeviceExt;

/// World-mesh vertex, 44 bytes. `light` is a half-float linear multiplier on
/// the lightmap sample (1.0 for world faces, the baked per-vertex light for a
/// static prop); `color` is the flat fallback albedo with the displacement
/// blend weight in its alpha byte; `tex` is `[layer, layer2]`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    pub light: [u16; 4],
    pub color: [u8; 4],
    pub tex: [u16; 2],
}

impl MeshVertex {
    pub const STRIDE: u64 = std::mem::size_of::<MeshVertex>() as u64;

    /// The attribute list every pipeline that consumes this vertex shares.
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 6] = [
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x3,
        },
        wgpu::VertexAttribute {
            offset: 12,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 20,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 28,
            shader_location: 3,
            format: wgpu::VertexFormat::Float16x4,
        },
        wgpu::VertexAttribute {
            offset: 36,
            shader_location: 4,
            format: wgpu::VertexFormat::Unorm8x4,
        },
        wgpu::VertexAttribute {
            offset: 40,
            shader_location: 5,
            format: wgpu::VertexFormat::Uint16x2,
        },
    ];

    pub fn new(
        position: [f32; 3],
        uv: [f32; 2],
        lm_uv: [f32; 2],
        light: [f32; 3],
        color: [f32; 3],
        alpha: f32,
        tex: u32,
        tex2: u32,
    ) -> Self {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        Self {
            position,
            uv,
            lm_uv,
            light: [f32_to_f16(light[0]), f32_to_f16(light[1]), f32_to_f16(light[2]), f32_to_f16(1.0)],
            color: [byte(color[0]), byte(color[1]), byte(color[2]), byte(alpha)],
            tex: [tex.min(u16::MAX as u32) as u16, tex2.min(u16::MAX as u32) as u16],
        }
    }
}

/// IEEE half-precision encode (round-to-nearest-even), enough for a light
/// multiplier: 0..65504 with 11 bits of mantissa.
pub fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        // inf / nan
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        let round = (m >> 13) as u16 + u16::from((m & 0x1fff) > 0x1000 || ((m & 0x3fff) == 0x3000));
        return sign | round;
    }
    let mut h = sign | ((e as u16) << 10) | (mant >> 13) as u16;
    let rem = mant & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && (h & 1) == 1) {
        h += 1;
    }
    h
}

pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    /// The index buffer is partitioned into three runs, drawn in this order:
    /// `0..opaque_index_count` opaque, `opaque_index_count..translucent_end`
    /// `$translucent` (blended `src.a` over, no depth write), and
    /// `translucent_end..index_count` `$additive` (`dst += src`). Triangle
    /// order in the source mesh is untouched (prop ranges still index it);
    /// only the index buffer is partitioned.
    pub opaque_index_count: u32,
    /// End of the translucent run — see `opaque_index_count`.
    pub translucent_end: u32,
}

impl GpuMesh {
    /// The atlas says which texture-array layers are `$additive` or
    /// `$translucent`; a graybox atlas declares neither and everything is
    /// opaque.
    pub fn from_graybox(device: &wgpu::Device, mesh: &GrayboxMesh, atlas: &MaterialAtlas) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut translucent = Vec::new();
        let mut additive = Vec::new();
        for tri in &mesh.tris {
            let base = vertices.len() as u32;
            let corners = [
                (tri.a, tri.uv_a, tri.lm_a, tri.light[0], tri.alpha[0]),
                (tri.b, tri.uv_b, tri.lm_b, tri.light[1], tri.alpha[1]),
                (tri.c, tri.uv_c, tri.lm_c, tri.light[2], tri.alpha[2]),
            ];
            for (p, uv, lm, light, alpha) in corners {
                vertices.push(MeshVertex::new(
                    source_to_yup(p),
                    uv,
                    lm,
                    light,
                    tri.color,
                    alpha,
                    tri.tex,
                    tri.tex2,
                ));
            }
            let layer = tri.tex as usize;
            let list = if atlas.additive_layers.get(layer).copied().unwrap_or(false) {
                &mut additive
            } else if atlas
                .translucent_layers
                .get(layer)
                .copied()
                .unwrap_or(false)
            {
                &mut translucent
            } else {
                &mut indices
            };
            list.extend_from_slice(&[base, base + 1, base + 2]);
        }
        let opaque_index_count = indices.len() as u32;
        indices.extend_from_slice(&translucent);
        let translucent_end = indices.len() as u32;
        indices.extend_from_slice(&additive);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_verts"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh_indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            opaque_index_count,
            translucent_end,
        }
    }
}

/// One description of the world material bindings, so the layout used to build
/// the pipeline and the layout used on a map reload cannot drift apart.
const MAT_BGL_ENTRIES: [wgpu::BindGroupLayoutEntry; 4] = [
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
    wgpu::BindGroupLayoutEntry {
        binding: 2,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    },
    wgpu::BindGroupLayoutEntry {
        binding: 3,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    },
];

const MAT_BGL: wgpu::BindGroupLayoutDescriptor = wgpu::BindGroupLayoutDescriptor {
    label: Some("mat_bgl"),
    entries: &MAT_BGL_ENTRIES,
};

pub struct GpuMaterials {
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl GpuMaterials {
    /// The layout the world pipeline is built against. A reload MUST reuse the
    /// one already stored on `GpuMaterials` — a fresh layout is a different
    /// object to wgpu, and the pipeline would stop accepting the bind group.
    pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&MAT_BGL)
    }

    /// Rebuild just the bind group for a new map, against the existing layout.
    /// Layer counts differ per map; a layout does not encode array length, so
    /// the same layout serves every map.
    pub fn reload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &MaterialAtlas,
        lightmaps: &LightmapAtlas,
    ) {
        let rebuilt = Self::upload_with(
            device,
            queue,
            atlas,
            lightmaps,
            Some(&self.bind_group_layout),
        );
        self.bind_group = rebuilt.bind_group;
    }

    pub fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &MaterialAtlas,
        lightmaps: &LightmapAtlas,
    ) -> Self {
        Self::upload_with(device, queue, atlas, lightmaps, None)
    }

    fn upload_with(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &MaterialAtlas,
        lightmaps: &LightmapAtlas,
        reuse: Option<&wgpu::BindGroupLayout>,
    ) -> Self {
        let size = atlas.layer_size;
        let layers = atlas.layer_count.max(1);
        // Only the levels the atlas actually carries (a graybox atlas has one).
        let mip_level_count = (1 + atlas.mips.len() as u32).min(size.max(1).ilog2() + 1);
        let albedo = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("albedo_array"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut level_size = size;
        for level in 0..mip_level_count {
            let data: &[u8] = if level == 0 {
                &atlas.rgba
            } else {
                &atlas.mips[(level - 1) as usize]
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &albedo,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(level_size * 4),
                    rows_per_image: Some(level_size),
                },
                wgpu::Extent3d {
                    width: level_size,
                    height: level_size,
                    depth_or_array_layers: layers,
                },
            );
            level_size = (level_size / 2).max(1);
        }
        let albedo_view = albedo.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        // Trilinear + 16× anisotropic: a surf ramp is seen at a grazing angle
        // for most of a run, which is exactly where isotropic mips blur.
        let albedo_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("albedo_samp"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });

        let lm_w = lightmaps.width.max(1);
        let lm_h = lightmaps.height.max(1);
        let lightmap = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lightmap"),
            size: wgpu::Extent3d {
                width: lm_w,
                height: lm_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Luxels are stored sRGB-encoded (`L / 2`, see surf-map's
            // lightmap module) so the shadow end keeps its precision; the
            // sampler hands back linear.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &lightmap,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lightmaps.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(lm_w * 4),
                rows_per_image: Some(lm_h),
            },
            wgpu::Extent3d {
                width: lm_w,
                height: lm_h,
                depth_or_array_layers: 1,
            },
        );
        let lm_view = lightmap.create_view(&wgpu::TextureViewDescriptor::default());
        let lm_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lm_samp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let owned_layout;
        let bind_group_layout: &wgpu::BindGroupLayout = match reuse {
            Some(l) => l,
            None => {
                owned_layout = device.create_bind_group_layout(&MAT_BGL);
                &owned_layout
            }
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mat_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&albedo_samp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&lm_samp),
                },
            ],
        });
        Self {
            bind_group,
            bind_group_layout: match reuse {
                Some(l) => l.clone(),
                None => device.create_bind_group_layout(&MAT_BGL),
            },
        }
    }
}

fn source_to_yup(v: Vec3) -> [f32; 3] {
    [v.x, v.z, -v.y]
}
