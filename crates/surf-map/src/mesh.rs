//! Textured + lightmapped face mesh from world + render-only brush models.

use surf_core::graybox::{GrayboxMesh, Tri};
use surf_core::math::Vec3;
use vbsp::{Bsp, Handle, TextureFlags};

use crate::lightmap::LightmapBaker;
use crate::materials::MaterialBank;

pub fn build_mesh(
    bsp: &Bsp,
    extra_models: &[(usize, Vec3)],
    materials: &mut MaterialBank,
    lightmaps: &mut LightmapBaker,
) -> GrayboxMesh {
    let mut tris = Vec::new();

    // World model first (origin = 0).
    append_model_faces(bsp, 0, Vec3::ZERO, materials, lightmaps, &mut tris);

    let mut seen = vec![0usize];
    for &(model_idx, origin) in extra_models {
        if model_idx == 0 || seen.contains(&model_idx) {
            continue;
        }
        seen.push(model_idx);
        append_model_faces(bsp, model_idx, origin, materials, lightmaps, &mut tris);
    }

    GrayboxMesh { tris }
}

fn append_model_faces(
    bsp: &Bsp,
    model_idx: usize,
    origin: Vec3,
    materials: &mut MaterialBank,
    lightmaps: &mut LightmapBaker,
    tris: &mut Vec<Tri>,
) {
    let Some(model) = bsp.models.get(model_idx) else {
        return;
    };
    let start = model.first_face as usize;
    let end = start + model.face_count as usize;
    for face_idx in start..end {
        let Some(face) = bsp.faces.get(face_idx) else {
            continue;
        };
        if face.texture_info < 0 {
            continue;
        }
        if face.displacement_info >= 0 {
            continue;
        }
        let Some(tex) = bsp.textures_info.get(face.texture_info as usize) else {
            continue;
        };
        let skip = TextureFlags::NODRAW
            | TextureFlags::SKY
            | TextureFlags::SKY2D
            | TextureFlags::TRIGGER
            | TextureFlags::HINT
            | TextureFlags::SKIP;
        if tex.flags.intersects(skip) {
            continue;
        }

        lightmaps.ensure_face(bsp, face_idx, face);
        let tex_h = Handle::new(bsp, tex);
        let tex_layer = materials.resolve(tex_h.name());
        let color = face_color(bsp, face.texture_info as usize);
        let handle = Handle::new(bsp, face);
        for tri in handle.triangulate() {
            let a = Vec3::new(tri[0].x, tri[0].y, tri[0].z) + origin;
            let b = Vec3::new(tri[1].x, tri[1].y, tri[1].z) + origin;
            let c = Vec3::new(tri[2].x, tri[2].y, tri[2].z) + origin;
            // Lightmap/UV sampling still uses local face verts.
            tris.push(Tri {
                a,
                b,
                c,
                color,
                uv_a: tex_h.uv(tri[0]),
                uv_b: tex_h.uv(tri[1]),
                uv_c: tex_h.uv(tri[2]),
                lm_a: lightmaps.lm_uv(bsp, face_idx, face, tri[0]),
                lm_b: lightmaps.lm_uv(bsp, face_idx, face, tri[1]),
                lm_c: lightmaps.lm_uv(bsp, face_idx, face, tri[2]),
                tex: tex_layer,
            });
        }
    }
}

fn face_color(bsp: &Bsp, texinfo_idx: usize) -> [f32; 3] {
    let Some(tex) = bsp.textures_info.get(texinfo_idx) else {
        return [0.55, 0.55, 0.58];
    };
    let data_idx = tex.texture_data_index as usize;
    let Some(data) = bsp.textures_data.get(data_idx) else {
        return [0.55, 0.55, 0.58];
    };
    let r = data.reflectivity;
    let mut col = [r.x, r.y, r.z];
    let lum = col[0] * 0.3 + col[1] * 0.6 + col[2] * 0.1;
    if lum < 0.05 {
        let h = data_idx
            .wrapping_mul(2654435761)
            .wrapping_add(texinfo_idx);
        col = [
            0.35 + ((h) & 0xff) as f32 / 255.0 * 0.45,
            0.35 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.45,
            0.35 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.45,
        ];
    } else {
        for c in &mut col {
            *c = (*c * 0.7 + 0.15).clamp(0.05, 0.95);
        }
    }
    col
}
