use surf_core::graybox::GrayboxMesh;
use surf_core::math::Vec3;
use surf_map::{LightmapAtlas, MaterialAtlas};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    pub tex: f32,
    pub lm_uv: [f32; 2],
    pub _pad: f32,
}

pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

impl GpuMesh {
    pub fn from_graybox(device: &wgpu::Device, mesh: &GrayboxMesh) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for tri in &mesh.tris {
            let base = vertices.len() as u32;
            let corners = [
                (tri.a, tri.uv_a, tri.lm_a),
                (tri.b, tri.uv_b, tri.lm_b),
                (tri.c, tri.uv_c, tri.lm_c),
            ];
            for (p, uv, lm) in corners {
                vertices.push(MeshVertex {
                    position: source_to_yup(p),
                    color: tri.color,
                    uv,
                    tex: tri.tex as f32,
                    lm_uv: lm,
                    _pad: 0.0,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
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
        let rebuilt = Self::upload_with(device, queue, atlas, lightmaps, Some(&self.bind_group_layout));
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
        let albedo = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("albedo_array"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
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
                texture: &albedo,
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
                depth_or_array_layers: layers,
            },
        );
        let albedo_view = albedo.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let albedo_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("albedo_samp"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
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
            format: wgpu::TextureFormat::Rgba8Unorm, // linear lighting
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
