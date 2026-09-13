//! Pakfile material resolve → RGBA texture array layers for the renderer.

use std::collections::HashMap;

use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

use crate::pak::{normalize_path, PakFs};
use crate::stock::StockFs;
use crate::vmt;

/// Side length of the fallback / graybox texture-array layer. A real map picks
/// its own size in [`MaterialBank::into_atlas`].
pub const TEX_LAYER_SIZE: u32 = 256;

/// Largest layer side a map may pick. Most corpus VTFs are 512 or 1024; the
/// handful of 2048/4096 ones are downscaled.
pub const MAX_LAYER_SIZE: u32 = 1024;

/// GPU-ready albedo atlas: `layer_count` slices of `layer_size²` RGBA8, with
/// a full mip chain.
#[derive(Clone, Debug)]
pub struct MaterialAtlas {
    pub layer_size: u32,
    pub layer_count: u32,
    /// Mip level 0, packed layer-major: `[layer0 | layer1 | …]` each
    /// `layer_size² * 4` bytes.
    pub rgba: Vec<u8>,
    /// Mip levels 1.. (each half the previous side, down to 1×1), packed the
    /// same way. Without them a 1024² texture seen from 2000 units away is
    /// point-sampled and shimmers with every camera move.
    pub mips: Vec<Vec<u8>>,
    /// How many materials resolved to a real VTF (layer > 0).
    pub textured_count: u32,
    /// Unique material names without a real VTF (may use a generated placeholder).
    pub missing_count: u32,
    /// Sample of missing material names (capped) for diagnostics.
    pub missing_names: Vec<String>,
    /// Per layer: is this an `$additive` material? The renderer draws those in
    /// a second, blended pass (`dst += src`) after the opaque world.
    pub additive_layers: Vec<bool>,
    /// Per layer: is this a `$translucent` material? Drawn in a blended pass
    /// (`src.a` over the world) between the opaque world and the additive
    /// glows. Disjoint from `additive_layers` — a material declaring both is
    /// additive, because that is the blend Source ends up using.
    pub translucent_layers: Vec<bool>,
    /// Every resolved material name → its layer. "Which texture is that white
    /// wall?" is the question every texture triage starts with, and without
    /// this the atlas is an anonymous pile of images.
    pub layer_of: Vec<(String, u32)>,
}

impl MaterialAtlas {
    /// Single white layer — graybox / missing-texture fallback.
    pub fn solid_white() -> Self {
        let px = (TEX_LAYER_SIZE * TEX_LAYER_SIZE * 4) as usize;
        Self {
            layer_size: TEX_LAYER_SIZE,
            layer_count: 1,
            rgba: vec![255u8; px],
            mips: Vec::new(),
            textured_count: 0,
            missing_count: 0,
            missing_names: Vec::new(),
            additive_layers: vec![false],
            translucent_layers: vec![false],
            layer_of: Vec::new(),
        }
    }
}

/// Lazily loads albedos from pakfile (+ optional local stock) while meshing.
pub struct MaterialBank {
    pak: PakFs,
    stock: StockFs,
    /// material name (normalized) → layer index
    by_material: HashMap<String, u32>,
    /// resolved VTF path → layer index (cubemap patches often share one albedo)
    by_texture: HashMap<String, u32>,
    layers: Vec<RgbaImage>,
    /// Both parallel to `layers`.
    additive: Vec<bool>,
    translucent: Vec<bool>,
    textured_count: u32,
    missing_count: u32,
    missing_names: Vec<String>,
}

enum LoadedAlbedo {
    Real {
        image: DynamicImage,
        texture_path: String,
    },
    Placeholder(DynamicImage),
    /// A material that *is* a colour: `$color`/`$color2` with no basetexture we
    /// can load. Built directly at the right absolute brightness rather than
    /// tinted afterwards — see [`flat_color_layer`].
    FlatColor([f32; 3]),
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
            by_texture: HashMap::new(),
            layers: vec![white],
            additive: vec![false],
            translucent: vec![false],
            textured_count: 0,
            missing_count: 0,
            missing_names: Vec::new(),
        }
    }

    pub(crate) fn get_bytes(&self, path: &str) -> Option<Vec<u8>> {
        self.pak.get(path).or_else(|| self.stock.get(path))
    }

    /// Resolve an MDL texture using its ordered CD-material search directories.
    pub(crate) fn resolve_model_texture(&mut self, directories: &[String], texture: &str) -> u32 {
        for directory in directories {
            let candidate = material_key(&format!("{directory}/{texture}"));
            let vmt = format!("materials/{candidate}.vmt");
            let vtf = format!("materials/{candidate}.vtf");
            if self.get_bytes(&vmt).is_some() || self.get_bytes(&vtf).is_some() {
                return self.resolve(&candidate);
            }
        }
        self.resolve(&material_key(texture))
    }

    /// Resolve a BSP material name to a texture-array layer (0 = missing).
    pub fn resolve(&mut self, material_name: &str) -> u32 {
        let key = normalize_path(material_name);
        if let Some(&id) = self.by_material.get(&key) {
            return id;
        }
        // The shader alpha-tests every layer uniformly, so a layer's alpha must
        // mean "cutout" or nothing at all. Two materials can share one VTF while
        // disagreeing about that, hence the flag in the dedupe key.
        let get = |p: &str| self.get_bytes(p);
        let alpha_tested = vmt::is_alpha_tested(&get, &key);
        // `$color2`/`$color` is baked into the layer rather than carried per
        // vertex: it needs no new bind group, and the tint must be part of the
        // dedupe key or six differently-coloured neon strips sharing one white
        // VTF collapse into one white layer.
        let off = disabled();
        let tint = (!off.tint).then(|| vmt::resolve_tint(&get, &key)).flatten();
        let additive = !off.additive && vmt::is_additive(&get, &key);
        // A `$translucent` surface blends; it does not cut out. Additive wins
        // when a material declares both — `dst += src` is the blend Source
        // settles on, and the two passes are mutually exclusive here.
        let translucent = !off.translucent && !additive && vmt::is_translucent(&get, &key);
        let id = match self.load_albedo(&key, tint) {
            Some(LoadedAlbedo::Real {
                image,
                texture_path,
            }) => {
                let cache_key = format!(
                    "{texture_path}#{}{}{}#{}",
                    u8::from(alpha_tested),
                    u8::from(additive),
                    u8::from(translucent),
                    tint_key(tint)
                );
                if let Some(&id) = self.by_texture.get(&cache_key) {
                    id
                } else {
                    let id = self.layers.len() as u32;
                    let mut layer = cap_layer(image);
                    // An additive layer keeps its VTF alpha only when the
                    // material also asked for it (`$translucent`/`$alphatest`);
                    // the additive pass multiplies rgb by it, nothing else does.
                    if !alpha_tested {
                        force_opaque(&mut layer);
                    }
                    apply_tint(&mut layer, tint);
                    self.layers.push(layer);
                    self.additive.push(additive);
                    self.translucent.push(translucent);
                    self.by_texture.insert(cache_key, id);
                    self.textured_count += 1;
                    id
                }
            }
            Some(LoadedAlbedo::Placeholder(img)) => {
                self.note_missing(&key);
                let id = self.layers.len() as u32;
                let mut layer = cap_layer(img);
                force_opaque(&mut layer);
                apply_tint(&mut layer, tint);
                self.layers.push(layer);
                self.additive.push(additive);
                // Forced opaque above, so blending it would change nothing.
                self.translucent.push(false);
                id
            }
            Some(LoadedAlbedo::FlatColor(c)) => {
                self.note_missing(&key);
                let id = self.layers.len() as u32;
                self.layers.push(flat_color_layer(c));
                self.additive.push(additive);
                self.translucent.push(false);
                id
            }
            None => {
                self.note_missing(&key);
                0
            }
        };
        self.by_material.insert(key, id);
        id
    }

    /// The `$basetexture2` layer of a two-texture displacement material
    /// (`WorldVertexTransition`), or 0 when it has none. Untinted, opaque:
    /// the blend weight is per vertex, not in the texture.
    pub fn resolve_secondary(&mut self, material_name: &str) -> u32 {
        let key = normalize_path(material_name);
        let get = |p: &str| self.get_bytes(p);
        let Some(bt) = vmt::resolve_basetexture2(&get, &key) else {
            return 0;
        };
        let texture_path = normalize_path(&format!("materials/{bt}.vtf"));
        let cache_key = format!("{texture_path}#000#");
        if let Some(&id) = self.by_texture.get(&cache_key) {
            return id;
        }
        let Some(image) = self.decode_vtf(&texture_path) else {
            return 0;
        };
        let id = self.layers.len() as u32;
        let mut layer = cap_layer(image);
        force_opaque(&mut layer);
        self.layers.push(layer);
        self.additive.push(false);
        self.translucent.push(false);
        self.by_texture.insert(cache_key, id);
        id
    }

    fn note_missing(&mut self, material_name: &str) {
        self.missing_count += 1;
        if self.missing_names.len() < 64 {
            self.missing_names.push(material_name.to_string());
        }
    }

    fn load_albedo(&self, material_name: &str, tint: Option<[f32; 3]>) -> Option<LoadedAlbedo> {
        let get = |p: &str| self.get_bytes(p);
        if let Some(bt) = vmt::resolve_basetexture(&get, material_name) {
            let texture_path = normalize_path(&format!("materials/{bt}.vtf"));
            if let Some(image) = self.decode_vtf(&texture_path) {
                return Some(LoadedAlbedo::Real {
                    image,
                    texture_path,
                });
            }
        }
        let texture_path = normalize_path(&format!("materials/{material_name}.vtf"));
        if let Some(image) = self.decode_vtf(&texture_path) {
            return Some(LoadedAlbedo::Real {
                image,
                texture_path,
            });
        }
        let kind = vmt::see_through_kind(&get, material_name).or_else(|| {
            if material_name.contains("refract") {
                Some(vmt::SeeThrough::Refract)
            } else if material_name.contains("water") {
                Some(vmt::SeeThrough::Water)
            } else {
                None
            }
        });
        if let Some(kind) = kind {
            let rgb = match kind {
                vmt::SeeThrough::Water => image::Rgba([38, 82, 105, 255]),
                // Frosted white-blue; lit like everything else now, so it
                // can read light without clipping.
                vmt::SeeThrough::Refract => image::Rgba([196, 214, 222, 255]),
            };
            let px = RgbaImage::from_pixel(8, 8, rgb);
            return Some(LoadedAlbedo::Placeholder(DynamicImage::ImageRgba8(px)));
        }
        // A tinted material whose basetexture we do not have is almost always a
        // recoloured stock flat — `lights/white`, the texture every neon strip
        // in the corpus is built from. White here is not a guess about the
        // missing VTF so much as the only value that lets the tint speak; the
        // material is still counted missing.
        if let Some(c) = tint {
            return Some(LoadedAlbedo::FlatColor(c));
        }
        None
    }

    fn decode_vtf(&self, path: &str) -> Option<DynamicImage> {
        let bytes = self.get_bytes(path)?;
        decode_vtf_bytes(&bytes)
    }

    pub fn into_atlas(self) -> MaterialAtlas {
        let layer_count = self.layers.len() as u32;
        let layer_size = choose_layer_size(&self.layers);
        let mut levels: Vec<Vec<u8>> = Vec::new();
        let mut size = layer_size;
        while size >= 1 {
            levels.push(Vec::with_capacity((size * size * 4 * layer_count) as usize));
            if size == 1 {
                break;
            }
            size /= 2;
        }
        for layer in self.layers {
            let mut img = if layer.width() == layer_size && layer.height() == layer_size {
                layer
            } else {
                image::imageops::resize(&layer, layer_size, layer_size, FilterType::Triangle)
            };
            levels[0].extend_from_slice(img.as_raw());
            for level in levels.iter_mut().skip(1) {
                let half = (img.width() / 2).max(1);
                img = image::imageops::resize(&img, half, half, FilterType::Triangle);
                level.extend_from_slice(img.as_raw());
            }
        }
        let rgba = levels.remove(0);
        MaterialAtlas {
            layer_size,
            layer_count,
            rgba,
            mips: levels,
            textured_count: self.textured_count,
            missing_count: self.missing_count,
            missing_names: self.missing_names,
            additive_layers: self.additive,
            translucent_layers: self.translucent,
            layer_of: {
                let mut v: Vec<_> = self.by_material.into_iter().collect();
                v.sort();
                v
            },
        }
    }
}

fn material_key(path: &str) -> String {
    let key = normalize_path(path);
    let key = key.strip_prefix("materials/").unwrap_or(&key);
    key.strip_suffix(".vmt")
        .or_else(|| key.strip_suffix(".vtf"))
        .unwrap_or(key)
        .to_string()
}

/// Blank the alpha channel so the shader's cutout test can never fire on a
/// material that never asked for one.
fn force_opaque(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        px.0[3] = 255;
    }
}

/// A layer for a material that is nothing but a colour — `$color`/`$color2`
/// with no basetexture we can load. hourglass builds its sky, its hourglass
/// props and its tree trunks this way; so does demise's fake red sky.
///
/// The halving is the point. The fragment shader draws textured surfaces as
/// `albedo * light * 2`, a calibration against the LDR lightmap scale, and for
/// a real VTF that is right — its brightness is relative to every other
/// texture. A flat `$color` is not relative to anything: it *is* the surface
/// colour the author picked, so it has to survive the round trip. Skip the
/// halving and hourglass's `{220 220 220}` overcast sky clips to pure white.
fn flat_color_layer(linear: [f32; 3]) -> RgbaImage {
    let mut px = [255u8; 4];
    for (i, c) in linear.iter().enumerate() {
        px[i] = (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
    }
    RgbaImage::from_pixel(4, 4, image::Rgba([px[0], px[1], px[2], 255]))
}

/// A/B lever, same convention as `SURF_OSS_NO_PHY` / `SURF_OSS_NO_FIELDS`:
/// `SURF_OSS_NO_VMT_SHADING=tint|additive|all`. Kept separable because the two
/// land on overlapping maps and a corpus diff that cannot tell them apart says
/// nothing useful — nyx's cave darkens under `tint` (the mapper's own
/// `$color [0.2 0.2 0.2]` on the rock) and its glow rails change under
/// `additive`, and reading that as one number would have called a correct
/// change a regression.
#[derive(Clone, Copy, Default)]
struct Disabled {
    tint: bool,
    additive: bool,
    translucent: bool,
}

fn disabled() -> Disabled {
    let Some(v) = std::env::var_os("SURF_OSS_NO_VMT_SHADING") else {
        return Disabled::default();
    };
    let v = v.to_string_lossy().to_ascii_lowercase();
    let all = v.is_empty() || v == "1" || v == "all";
    Disabled {
        tint: all || v.contains("tint"),
        additive: all || v.contains("additive"),
        translucent: all || v.contains("translucent"),
    }
}

/// Stable dedupe-key fragment for a tint (quantised, so float noise cannot
/// split one colour into two layers).
fn tint_key(tint: Option<[f32; 3]>) -> String {
    match tint {
        None => "_".to_string(),
        Some(c) => format!(
            "{}:{}:{}",
            (c[0] * 1024.0) as i32,
            (c[1] * 1024.0) as i32,
            (c[2] * 1024.0) as i32
        ),
    }
}

/// Multiply a layer by a linear tint.
///
/// The atlas is uploaded as `Rgba8UnormSrgb`, so the bytes are gamma-encoded
/// and the multiply has to happen in linear space — doing it on the raw bytes
/// would make every tinted material read far too bright.
fn apply_tint(img: &mut RgbaImage, tint: Option<[f32; 3]>) {
    let Some(tint) = tint else { return };
    let mut lut = [[0u8; 256]; 3];
    for (c, factor) in tint.iter().enumerate() {
        for v in 0..256usize {
            let linear = srgb_to_linear(v as f32 / 255.0) * factor;
            lut[c][v] = (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0).round() as u8;
        }
    }
    for px in img.pixels_mut() {
        for c in 0..3 {
            px.0[c] = lut[c][px.0[c] as usize];
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Decoded VTF → RGBA8 no larger than `MAX_LAYER_SIZE` on a side. Layers are
/// kept at their own size until [`MaterialBank::into_atlas`] picks the array
/// size for the whole map.
fn cap_layer(img: DynamicImage) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w > MAX_LAYER_SIZE || h > MAX_LAYER_SIZE {
        let s = MAX_LAYER_SIZE;
        img.resize_exact(s, s, FilterType::Triangle).into_rgba8()
    } else {
        img.into_rgba8()
    }
}

/// Every layer of the array shares one size. 1024² RGBA with mips is 5.6 MB a
/// layer, so a map with many materials steps down to 512 rather than hold a
/// gigabyte of texture; `SURF_OSS_TEX_SIZE` overrides.
fn choose_layer_size(layers: &[RgbaImage]) -> u32 {
    if let Some(v) = std::env::var("SURF_OSS_TEX_SIZE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        if v.is_power_of_two() && (16..=4096).contains(&v) {
            return v;
        }
    }
    let largest = layers
        .iter()
        .map(|l| l.width().max(l.height()))
        .max()
        .unwrap_or(TEX_LAYER_SIZE)
        .next_power_of_two()
        .clamp(TEX_LAYER_SIZE, MAX_LAYER_SIZE);
    let mut size = largest;
    let budget: u64 = 640 << 20;
    while size > TEX_LAYER_SIZE {
        let bytes = layers.len() as u64 * (size as u64 * size as u64 * 4) * 4 / 3;
        if bytes <= budget {
            break;
        }
        size /= 2;
    }
    size
}

/// Sky faces: six layers at `SKY_LAYER_SIZE`. Skies ship at 1024–2048 a face
/// and fill the whole background, so they get their own, larger size.
fn resize_layer(img: DynamicImage) -> RgbaImage {
    img.resize_exact(SKY_LAYER_SIZE, SKY_LAYER_SIZE, FilterType::Triangle)
        .into_rgba8()
}

/// Side of each skybox face layer.
pub const SKY_LAYER_SIZE: u32 = 1024;

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
            layer_size: SKY_LAYER_SIZE,
            rgba: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }

    /// Load `materials/skybox/<skyname>{ft,bk,…}` from pakfile, then stock.
    pub fn from_pak_and_stock(pak: &PakFs, stock: &StockFs, skyname: &str) -> Self {
        let suffixes = ["ft", "bk", "lf", "rt", "up", "dn"];
        let mut layers = Vec::with_capacity(6);
        let fallback = RgbaImage::from_pixel(
            SKY_LAYER_SIZE,
            SKY_LAYER_SIZE,
            image::Rgba([140, 180, 220, 255]),
        );
        let mut any = false;
        for suf in suffixes {
            let mat = format!("skybox/{}{}", skyname, suf);
            let img = load_sky_face(pak, stock, &mat).map(resize_layer);
            if img.is_some() {
                any = true;
            }
            layers.push(img.unwrap_or_else(|| fallback.clone()));
        }
        if !any {
            return Self::none();
        }
        let mut rgba = Vec::with_capacity((SKY_LAYER_SIZE * SKY_LAYER_SIZE * 4 * 6) as usize);
        for layer in layers {
            rgba.extend_from_slice(layer.as_raw());
        }
        Self {
            layer_size: SKY_LAYER_SIZE,
            rgba,
        }
    }
}

fn load_sky_face(pak: &PakFs, stock: &StockFs, material_name: &str) -> Option<DynamicImage> {
    let get = |p: &str| pak.get(p).or_else(|| stock.get(p));
    if let Some(bt) = vmt::resolve_basetexture(&get, material_name) {
        if let Some(bytes) = get(&format!("materials/{bt}.vtf")) {
            if let Some(img) = decode_vtf_bytes(&bytes) {
                return Some(img);
            }
        }
    }
    let bytes = get(&format!("materials/{material_name}.vtf"))?;
    decode_vtf_bytes(&bytes)
}

/// Decode a VTF. The `vtf` crate covers DXT/RGB(A)/BGR(A); we add ABGR8888 / I8 /
/// A8 / ARGB8888 — common in community map pakfiles (frost bricks, 1×1 tints).
fn decode_vtf_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let file = vtf::from_bytes(bytes).ok()?;
    if let Ok(img) = file.highres_image.decode(0) {
        return Some(img);
    }
    let frame = file.highres_image.get_frame(0).ok()?;
    let w = file.highres_image.width as u32;
    let h = file.highres_image.height as u32;
    use vtf::ImageFormat;
    let rgba: Vec<u8> = match file.highres_image.format {
        ImageFormat::Abgr8888 => frame
            .chunks_exact(4)
            .flat_map(|px| {
                let (a, b, g, r) = (px[0], px[1], px[2], px[3]);
                [r, g, b, a]
            })
            .collect(),
        ImageFormat::Argb8888 => frame
            .chunks_exact(4)
            .flat_map(|px| {
                let (a, r, g, b) = (px[0], px[1], px[2], px[3]);
                [r, g, b, a]
            })
            .collect(),
        ImageFormat::I8 => frame.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        ImageFormat::A8 => frame.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
        ImageFormat::Ia88 => frame
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        _ => return None,
    };
    image::RgbaImage::from_raw(w, h, rgba).map(DynamicImage::ImageRgba8)
}
