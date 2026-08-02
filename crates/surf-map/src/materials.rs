//! Pakfile material resolve → RGBA texture array layers for the renderer.

use std::collections::HashMap;

use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

use crate::pak::{normalize_path, PakFs};
use crate::stock::StockFs;
use crate::vmt;

/// Side length of each texture-array layer (power of two).
pub const TEX_LAYER_SIZE: u32 = 256;

/// GPU-ready albedo atlas: `layer_count` slices of `TEX_LAYER_SIZE²` RGBA8.
#[derive(Clone, Debug)]
pub struct MaterialAtlas {
    pub layer_size: u32,
    pub layer_count: u32,
    /// Packed as layer-major: `[layer0 | layer1 | …]` each `layer_size² * 4` bytes.
    pub rgba: Vec<u8>,
    /// How many materials resolved to a real VTF (layer > 0).
    pub textured_count: u32,
}

impl MaterialAtlas {
    /// Single white layer — graybox / missing-texture fallback.
    pub fn solid_white() -> Self {
        let px = (TEX_LAYER_SIZE * TEX_LAYER_SIZE * 4) as usize;
        Self {
            layer_size: TEX_LAYER_SIZE,
            layer_count: 1,
            rgba: vec![255u8; px],
            textured_count: 0,
        }
    }
}

/// Lazily loads albedos from pakfile (+ optional local stock) while meshing.
pub struct MaterialBank {
    pak: PakFs,
    stock: StockFs,
    /// material name (normalized) → layer index
    by_material: HashMap<String, u32>,
    layers: Vec<RgbaImage>,
    textured_count: u32,
}

impl MaterialBank {
    pub fn new(pak: PakFs, stock: StockFs) -> Self {
        let white = RgbaImage::from_pixel(
            TEX_LAYER_SIZE,
            TEX_LAYER_SIZE,
            image::Rgba([255, 255, 255, 255]),
        );
        Self {
            pak,
            stock,
            by_material: HashMap::new(),
            layers: vec![white],
            textured_count: 0,
        }
    }

    fn get_bytes(&self, path: &str) -> Option<Vec<u8>> {
        self.pak.get(path).or_else(|| self.stock.get(path))
    }

    /// Resolve a BSP material name to a texture-array layer (0 = missing).
    pub fn resolve(&mut self, material_name: &str) -> u32 {
        let key = normalize_path(material_name);
        if let Some(&id) = self.by_material.get(&key) {
            return id;
        }
        let id = match self.load_albedo(&key) {
            Some(img) => {
                let id = self.layers.len() as u32;
                self.layers.push(resize_layer(img));
                self.textured_count += 1;
                id
            }
            None => 0,
        };
        self.by_material.insert(key, id);
        id
    }

    fn load_albedo(&self, material_name: &str) -> Option<DynamicImage> {
        let get = |p: &str| self.get_bytes(p);
        if let Some(bt) = vmt::resolve_basetexture(&get, material_name) {
            if let Some(img) = self.decode_vtf(&format!("materials/{bt}.vtf")) {
                return Some(img);
            }
        }
        if let Some(img) = self.decode_vtf(&format!("materials/{material_name}.vtf")) {
            return Some(img);
        }
        None
    }

    fn decode_vtf(&self, path: &str) -> Option<DynamicImage> {
        let bytes = self.get_bytes(path)?;
        let vtf = vtf::from_bytes(&bytes).ok()?;
        vtf.highres_image.decode(0).ok()
    }

    pub fn into_atlas(self) -> MaterialAtlas {
        let layer_size = TEX_LAYER_SIZE;
        let layer_count = self.layers.len() as u32;
        let mut rgba =
            Vec::with_capacity((layer_size * layer_size * 4 * layer_count) as usize);
        for layer in self.layers {
            rgba.extend_from_slice(layer.as_raw());
        }
        MaterialAtlas {
            layer_size,
            layer_count,
            rgba,
            textured_count: self.textured_count,
        }
    }
}

fn resize_layer(img: DynamicImage) -> RgbaImage {
    img.resize_exact(TEX_LAYER_SIZE, TEX_LAYER_SIZE, FilterType::Triangle)
        .into_rgba8()
}

/// Six skybox faces: ft, bk, lf, rt, up, dn (Source order).
#[derive(Clone, Debug)]
pub struct SkyboxAtlas {
    pub layer_size: u32,
    /// 6 × layer_size² RGBA8, or empty if unavailable.
    pub rgba: Vec<u8>,
}

impl SkyboxAtlas {
    pub fn none() -> Self {
        Self {
            layer_size: TEX_LAYER_SIZE,
            rgba: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }

    /// Load `materials/skybox/<skyname>{ft,bk,…}` from the pakfile.
    pub fn from_pak(pak: &PakFs, skyname: &str) -> Self {
        let suffixes = ["ft", "bk", "lf", "rt", "up", "dn"];
        let mut layers = Vec::with_capacity(6);
        let fallback = RgbaImage::from_pixel(
            TEX_LAYER_SIZE,
            TEX_LAYER_SIZE,
            image::Rgba([140, 180, 220, 255]),
        );
        let mut any = false;
        for suf in suffixes {
            let mat = format!("skybox/{}{}", skyname, suf);
            let img = load_sky_face(pak, &mat).map(resize_layer);
            if img.is_some() {
                any = true;
            }
            layers.push(img.unwrap_or_else(|| fallback.clone()));
        }
        if !any {
            return Self::none();
        }
        let mut rgba = Vec::with_capacity((TEX_LAYER_SIZE * TEX_LAYER_SIZE * 4 * 6) as usize);
        for layer in layers {
            rgba.extend_from_slice(layer.as_raw());
        }
        Self {
            layer_size: TEX_LAYER_SIZE,
            rgba,
        }
    }
}

fn load_sky_face(pak: &PakFs, material_name: &str) -> Option<DynamicImage> {
    let get = |p: &str| pak.get(p);
    if let Some(bt) = vmt::resolve_basetexture(&get, material_name) {
        if let Some(img) = decode_vtf_path(pak, &format!("materials/{bt}.vtf")) {
            return Some(img);
        }
    }
    decode_vtf_path(pak, &format!("materials/{material_name}.vtf"))
}

fn decode_vtf_path(pak: &PakFs, path: &str) -> Option<DynamicImage> {
    let bytes = pak.get(path)?;
    let vtf = vtf::from_bytes(&bytes).ok()?;
    vtf.highres_image.decode(0).ok()
}
