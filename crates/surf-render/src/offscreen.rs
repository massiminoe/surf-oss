//! Headless capture: load a map, point a camera at it, get a PNG.
//!
//! There is no window and no winit here, so this runs in tests and from agent
//! shells. It drives the *same* `Renderer` as the app — same shaders, same
//! uniforms, same mesh upload — so a capture is evidence about the real frame
//! rather than about a parallel code path.

use std::path::Path;

use surf_core::math::{Angle, Vec3};
use surf_map::LoadedMap;

use crate::camera::Camera;
use crate::mesh::GpuMesh;
use crate::pipeline::{Renderer, ViewParams};

/// Offscreen render target format. Matches the sRGB surface the app gets on
/// macOS, so tone/exposure in a capture matches what Max sees.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct Offscreen {
    renderer: Renderer,
    color: wgpu::Texture,
    width: u32,
    height: u32,
}

impl Offscreen {
    pub fn new(map: &LoadedMap, width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "no wgpu adapter".to_string())?;

        // Real maps blow past the default buffer limit (boreas' prop mesh alone
        // is hundreds of MB), so take what the adapter will give.
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("offscreen"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: Default::default(),
            },
            None,
        ))
        .map_err(|e| format!("no device: {e}"))?;

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: FORMAT,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };

        let mesh = GpuMesh::from_graybox(&device, &map.mesh, &map.materials);
        let renderer = Renderer::new(
            device,
            queue,
            config,
            mesh,
            &map.materials,
            &map.lightmaps,
            &map.skybox,
        );

        let color = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        Ok(Self {
            renderer,
            color,
            width,
            height,
        })
    }

    /// Render one frame and return it as RGBA8 rows, top-left origin.
    pub fn capture(&mut self, eye: Vec3, angles: Angle, view: ViewParams) -> Vec<u8> {
        self.capture_with_hud(eye, angles, view, None)
    }

    /// Same, but draws a HUD over the world — the only way to see a menu or a
    /// shell page without opening a window.
    pub fn capture_with_hud(
        &mut self,
        eye: Vec3,
        angles: Angle,
        view: ViewParams,
        hud: Option<crate::HudState>,
    ) -> Vec<u8> {
        let mut camera = Camera::new(eye, angles, self.width as f32 / self.height as f32);
        camera.fov_y_deg = 90.0;
        self.renderer.update_frame(&camera, None, None, view);

        let color_view = self
            .color
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.renderer.encode_frame(&color_view, hud, false, false);

        // Copy to a mappable buffer. wgpu requires 256-byte-aligned copy rows.
        let unpadded = self.width * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let readback = self.renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.renderer
            .queue
            .submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.renderer.device.poll(wgpu::Maintain::Wait);
        rx.recv().expect("map").expect("map ok");

        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded * self.height) as usize);
        for row in 0..self.height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + unpadded as usize]);
        }
        drop(data);
        readback.unmap();
        out
    }

    /// Render `frames` frames back-to-back with no readback and return the mean
    /// wall time per frame. Not a substitute for a real present-path capture
    /// (no compositor, no vsync) but it does measure our own draw cost.
    pub fn bench(&mut self, eye: Vec3, angles: Angle, view: ViewParams, frames: u32) -> f32 {
        let camera = Camera::new(eye, angles, self.width as f32 / self.height as f32);
        let color_view = self
            .color
            .create_view(&wgpu::TextureViewDescriptor::default());
        // Warm up: first frame pays pipeline + upload costs.
        for _ in 0..3 {
            self.renderer.update_frame(&camera, None, None, view);
            let enc = self.renderer.encode_frame(&color_view, None, false, false);
            self.renderer.queue.submit(std::iter::once(enc.finish()));
        }
        self.renderer.device.poll(wgpu::Maintain::Wait);

        let t0 = std::time::Instant::now();
        for _ in 0..frames {
            self.renderer.update_frame(&camera, None, None, view);
            let enc = self.renderer.encode_frame(&color_view, None, false, false);
            self.renderer.queue.submit(std::iter::once(enc.finish()));
            self.renderer.device.poll(wgpu::Maintain::Wait);
        }
        t0.elapsed().as_secs_f32() * 1000.0 / frames as f32
    }

    pub fn save_png(
        &mut self,
        eye: Vec3,
        angles: Angle,
        view: ViewParams,
        path: impl AsRef<Path>,
    ) -> Result<(), String> {
        let rgba = self.capture(eye, angles, view);
        image::save_buffer(
            path.as_ref(),
            &rgba,
            self.width,
            self.height,
            image::ColorType::Rgba8,
        )
        .map_err(|e| e.to_string())
    }
}

/// One-shot convenience: build a headless renderer for `map`, save one frame.
pub fn render_to_png(
    map: &LoadedMap,
    eye: Vec3,
    angles: Angle,
    width: u32,
    height: u32,
    path: impl AsRef<Path>,
) -> Result<(), String> {
    let mut off = Offscreen::new(map, width, height)?;
    off.save_png(eye, angles, ViewParams::default(), path)
}
