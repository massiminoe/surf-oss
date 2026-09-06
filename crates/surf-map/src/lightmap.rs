//! LDR/HDR lightmap lump → atlas + per-face luxel UVs.
//!
//! **Brightness convention (Source LDR).** A luxel is `ColorRGBExp32`:
//! `L = rgb * 2^exp / 255`, linear, with `L = 1` a fully lit surface and up to
//! ~4 near lights. The engine stores `L / 2` in the lightmap page and its
//! shaders multiply the sample by `OVERBRIGHT = 2`, so the pixel is
//! `albedo * L`. We keep exactly that: the atlas is an sRGB texture holding
//! `L / 2` (clamped at 1, i.e. `L ≤ 2` headroom) and the world shader doubles
//! it. The reserved texel for faces with no luxels — and for static props,
//! whose light is per-vertex — therefore stores `0.5`, meaning `L = 1`, not
//! white.
//!
//! `lm_uv` returns **atlas pixel** coordinates while packing (height may grow).
//! Call [`normalize_lm_uvs`] after [`LightmapBaker::finish`] so UVs are 0..1
//! against the final atlas size.

use std::collections::HashMap;

use surf_core::graybox::GrayboxMesh;
use vbsp::{Bsp, Face, TextureFlags, TextureInfo, Vector};

/// GPU-ready lightmap atlas (RGBA8, linear-ish decoded luxels).
#[derive(Clone, Debug)]
pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub face_count: u32,
}

impl LightmapAtlas {
    pub fn white_1x1() -> Self {
        Self {
            width: 1,
            height: 1,
            rgba: vec![UNIT_LIGHT_BYTE, UNIT_LIGHT_BYTE, UNIT_LIGHT_BYTE, 255],
            face_count: 0,
        }
    }

    pub fn is_stub(&self) -> bool {
        self.face_count == 0
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

/// sRGB byte encoding `L / 2` for `L = 1`: `srgb(0.5) * 255`. Every atlas
/// texel that means "lit at unity" — the stub atlas, the reserved texel for
/// luxel-less faces, the texel static props point at — stores this.
pub const UNIT_LIGHT_BYTE: u8 = 188;

/// Builds an atlas while meshing; maps `bsp.faces` index → atlas rect.
pub struct LightmapBaker {
    samples: Vec<u8>,
    /// face index → packed rect (inner luxel region, no border)
    rects: HashMap<u32, Rect>,
    atlas_w: u32,
    cursor_x: u32,
    cursor_y: u32,
    shelf_h: u32,
    atlas_h: u32,
    /// Growing RGBA buffer, width = atlas_w, height grows.
    rgba: Vec<u8>,
    /// Pixel coords for faces with no luxels (reserved white 2×2 when lighting exists).
    missing_px: [f32; 2],
}

impl LightmapBaker {
    pub fn from_bsp_bytes(bsp_bytes: &[u8]) -> Self {
        let samples = read_lighting_lump(bsp_bytes).unwrap_or_default();
        let mut baker = Self {
            samples,
            rects: HashMap::new(),
            atlas_w: 2048,
            cursor_x: 0,
            cursor_y: 0,
            shelf_h: 0,
            atlas_h: 0,
            rgba: Vec::new(),
            missing_px: [0.0, 0.0],
        };
        // Reserve a unit-light 2×2 so faces without luxels (and static props,
        // whose light is per-vertex) don't sample a dark luxel at (0,0).
        if !baker.samples.is_empty() {
            let (x, y) = baker.alloc(2, 2);
            for dy in 0..2 {
                for dx in 0..2 {
                    baker.put_px(
                        x + dx,
                        y + dy,
                        [UNIT_LIGHT_BYTE, UNIT_LIGHT_BYTE, UNIT_LIGHT_BYTE, 255],
                    );
                }
            }
            baker.missing_px = [x as f32 + 0.5, y as f32 + 0.5];
        }
        baker
    }

    pub fn ensure_face(&mut self, bsp: &Bsp, face_idx: usize, face: &Face) {
        if self.rects.contains_key(&(face_idx as u32)) {
            return;
        }
        if face.light_offset < 0 || self.samples.is_empty() {
            return;
        }
        let w = (face.light_map_texture_size[0] + 1).max(1) as u32;
        let h = (face.light_map_texture_size[1] + 1).max(1) as u32;
        if w > 512 || h > 512 {
            return;
        }
        let bump = bsp
            .textures_info
            .get(face.texture_info as usize)
            .map(|t| t.flags.contains(TextureFlags::BUMPLIGHT))
            .unwrap_or(false);
        let page = (w * h) as usize;
        let byte_off = face.light_offset as usize;
        // Style 0, bump page 0.
        let need = page * 4;
        if byte_off + need > self.samples.len() {
            return;
        }
        // If bump, pages are sequential; we still take the first page at light_offset.
        let _ = bump;

        let pad_w = w + 2;
        let pad_h = h + 2;
        let (x, y) = self.alloc(pad_w, pad_h);
        let inner_x = x + 1;
        let inner_y = y + 1;

        // Decode luxels into atlas (with 1px border clone).
        for ly in 0..h {
            for lx in 0..w {
                let si = byte_off + ((ly * w + lx) as usize) * 4;
                let rgba = decode_rgbexp(
                    self.samples[si],
                    self.samples[si + 1],
                    self.samples[si + 2],
                    self.samples[si + 3] as i8,
                );
                self.put_px(inner_x + lx, inner_y + ly, rgba);
            }
        }
        // Borders: clone edges for bilinear.
        for lx in 0..w {
            let top = self.get_px(inner_x + lx, inner_y);
            let bot = self.get_px(inner_x + lx, inner_y + h - 1);
            self.put_px(inner_x + lx, y, top);
            self.put_px(inner_x + lx, inner_y + h, bot);
        }
        for ly in 0..h {
            let left = self.get_px(inner_x, inner_y + ly);
            let right = self.get_px(inner_x + w - 1, inner_y + ly);
            self.put_px(x, inner_y + ly, left);
            self.put_px(inner_x + w, inner_y + ly, right);
        }
        // Corners.
        self.put_px(x, y, self.get_px(inner_x, inner_y));
        self.put_px(inner_x + w, y, self.get_px(inner_x + w - 1, inner_y));
        self.put_px(x, inner_y + h, self.get_px(inner_x, inner_y + h - 1));
        self.put_px(
            inner_x + w,
            inner_y + h,
            self.get_px(inner_x + w - 1, inner_y + h - 1),
        );

        self.rects.insert(
            face_idx as u32,
            Rect {
                x: inner_x as u16,
                y: inner_y as u16,
                w: w as u16,
                h: h as u16,
            },
        );
    }

    /// Atlas **pixel** coordinates (not yet divided by atlas size).
    pub fn lm_uv(&self, bsp: &Bsp, face_idx: usize, face: &Face, p: Vector) -> [f32; 2] {
        let Some(rect) = self.rects.get(&(face_idx as u32)) else {
            return self.missing_px;
        };
        let Some(tex) = bsp.textures_info.get(face.texture_info as usize) else {
            return self.missing_px;
        };
        let (s, t) = luxel_st(tex, face, p);
        let u = rect.x as f32 + s * rect.w as f32;
        let v = rect.y as f32 + t * rect.h as f32;
        [u, v]
    }

    pub fn finish(mut self) -> LightmapAtlas {
        if self.rects.is_empty() {
            return LightmapAtlas::white_1x1();
        }
        // Ensure buffer covers full height.
        let need = (self.atlas_w * self.atlas_h * 4) as usize;
        if self.rgba.len() < need {
            self.rgba.resize(need, 255);
        }
        LightmapAtlas {
            width: self.atlas_w,
            height: self.atlas_h.max(1),
            rgba: self.rgba,
            face_count: self.rects.len() as u32,
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.cursor_x + w > self.atlas_w {
            self.cursor_x = 0;
            self.cursor_y += self.shelf_h;
            self.shelf_h = 0;
        }
        if self.cursor_y + h > self.atlas_h {
            let new_h = (self.cursor_y + h).next_power_of_two().max(64);
            self.rgba
                .resize((self.atlas_w * new_h * 4) as usize, 255);
            self.atlas_h = new_h;
        }
        let x = self.cursor_x;
        let y = self.cursor_y;
        self.cursor_x += w;
        self.shelf_h = self.shelf_h.max(h);
        (x, y)
    }

    fn put_px(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.atlas_w || y >= self.atlas_h {
            return;
        }
        let i = ((y * self.atlas_w + x) * 4) as usize;
        self.rgba[i..i + 4].copy_from_slice(&rgba);
    }

    fn get_px(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.atlas_w || y >= self.atlas_h {
            return [255, 255, 255, 255];
        }
        let i = ((y * self.atlas_w + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Convert mesh lightmap UVs from atlas pixels → 0..1.
pub fn normalize_lm_uvs(mesh: &mut GrayboxMesh, atlas_w: u32, atlas_h: u32) {
    let aw = atlas_w.max(1) as f32;
    let ah = atlas_h.max(1) as f32;
    for tri in &mut mesh.tris {
        for uv in [&mut tri.lm_a, &mut tri.lm_b, &mut tri.lm_c] {
            uv[0] /= aw;
            uv[1] /= ah;
        }
    }
}

fn luxel_st(tex: &TextureInfo, face: &Face, p: Vector) -> (f32, f32) {
    let u = tex.light_map_scale[0] * p.x
        + tex.light_map_scale[1] * p.y
        + tex.light_map_scale[2] * p.z
        + tex.light_map_scale[3];
    let v = tex.light_map_transform[0] * p.x
        + tex.light_map_transform[1] * p.y
        + tex.light_map_transform[2] * p.z
        + tex.light_map_transform[3];
    let w = (face.light_map_texture_size[0] + 1).max(1) as f32;
    let h = (face.light_map_texture_size[1] + 1).max(1) as f32;
    let s = (u - face.light_map_texture_min[0] as f32) / w;
    let t = (v - face.light_map_texture_min[1] as f32) / h;
    (s, t)
}

/// `ColorRGBExp32` luxel → linear `L` per channel (see the module doc).
pub fn luxel_to_linear(r: u8, g: u8, b: u8, exp: i8) -> [f32; 3] {
    let scale = 2f32.powi(exp as i32) / 255.0;
    [r as f32 * scale, g as f32 * scale, b as f32 * scale]
}

/// Linear `L` → the atlas byte: `srgb(L / 2)`, clamped. sRGB encoding keeps
/// the shadow end of an 8-bit page from banding; the texture is declared
/// `Rgba8UnormSrgb` so the sampler hands the shader `L / 2` back in linear.
pub fn encode_light_byte(l: f32) -> u8 {
    let v = (l * 0.5).clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn decode_rgbexp(r: u8, g: u8, b: u8, exp: i8) -> [u8; 4] {
    let l = luxel_to_linear(r, g, b, exp);
    [
        encode_light_byte(l[0]),
        encode_light_byte(l[1]),
        encode_light_byte(l[2]),
        255,
    ]
}

/// The LDR lighting lump (8), else HDR (53) — void ships only HDR. Read
/// through the LZMA-aware reader: six corpus maps (aquaflow, boreas,
/// botanica, demise, lovetunnel, tendies) compress it, and reading the raw
/// bytes silently produced no lightmap at all on every one of them.
fn read_lighting_lump(bsp_bytes: &[u8]) -> Result<Vec<u8>, String> {
    for lump in [LUMP_LIGHTING, LUMP_LIGHTING_HDR] {
        match crate::leaves::read_lump(bsp_bytes, lump) {
            Ok((data, _)) if !data.is_empty() => return Ok(data),
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Vec::new())
}

const LUMP_LIGHTING: usize = 8;
const LUMP_LIGHTING_HDR: usize = 53;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_divides_by_final_atlas_size() {
        use surf_core::math::Vec3;
        let mut mesh = GrayboxMesh {
            tris: vec![surf_core::graybox::Tri {
                a: Vec3::ZERO,
                b: Vec3::ZERO,
                c: Vec3::ZERO,
                color: [1.0, 1.0, 1.0],
                uv_a: [0.0, 0.0],
                uv_b: [0.0, 0.0],
                uv_c: [0.0, 0.0],
                lm_a: [512.0, 256.0],
                lm_b: [1024.0, 512.0],
                lm_c: [0.0, 0.0],
                tex: 0,
                tex2: 0,
                alpha: [0.0; 3],
                light: [[1.0; 3]; 3],
            }],
        };
        normalize_lm_uvs(&mut mesh, 2048, 1024);
        assert!((mesh.tris[0].lm_a[0] - 0.25).abs() < 1e-5);
        assert!((mesh.tris[0].lm_a[1] - 0.25).abs() < 1e-5);
        assert!((mesh.tris[0].lm_b[0] - 0.5).abs() < 1e-5);
        assert!((mesh.tris[0].lm_b[1] - 0.5).abs() < 1e-5);
    }
}
