//! Displacement → collision tris + textured/lightmapped render tris.

use surf_core::graybox::Tri;
use surf_core::math::Vec3;
use surf_core::CollisionTri;
use vbsp::{BrushFlags, Bsp, Face, Handle, Vector};

use crate::lightmap::LightmapBaker;
use crate::materials::MaterialBank;

const TRI_THICKNESS: f32 = 2.0;
/// Grid cell size for tri broadphase (Source units).
pub const TRI_GRID_CELL: f32 = 256.0;

fn mask_playersolid() -> BrushFlags {
    BrushFlags::SOLID
        | BrushFlags::MOVEABLE
        | BrushFlags::PLAYERCLIP
        | BrushFlags::WINDOW
        | BrushFlags::MONSTER
        | BrushFlags::GRATE
}

pub struct DispExtract {
    pub collision: Vec<CollisionTri>,
    pub render: Vec<Tri>,
}

pub fn extract_displacements(
    bsp: &Bsp,
    materials: &mut MaterialBank,
    lightmaps: &mut LightmapBaker,
) -> DispExtract {
    let mut collision = Vec::new();
    let mut render = Vec::new();
    let mask = mask_playersolid();

    for disp in &bsp.displacements {
        let flags = BrushFlags::from_bits_truncate(disp.contents as u32);
        if !flags.intersects(mask) {
            continue;
        }
        let handle = Handle::new(bsp, disp);
        let Some(face_h) = handle.face() else {
            continue;
        };
        let face_idx = disp.map_face as usize;
        let Some(face_ref) = bsp.faces.get(face_idx) else {
            continue;
        };
        let corners = face_h.vertices().map(|v| v.position).collect::<Vec<_>>();
        if corners.len() != 4 {
            continue;
        }
        let Some(verts) = displaced_grid(bsp, disp, &corners) else {
            continue;
        };

        lightmaps.ensure_face(bsp, face_idx, face_ref);
        let tex_info = if face_ref.texture_info >= 0 {
            bsp.textures_info.get(face_ref.texture_info as usize)
        } else {
            None
        };
        let (tex_layer, color, tex_h) = if let Some(tex) = tex_info {
            let tex_h = Handle::new(bsp, tex);
            let layer = materials.resolve(tex_h.name());
            let color = face_color_fallback(tex_h.name());
            (layer, color, Some(tex_h))
        } else {
            (0, [0.45, 0.50, 0.40], None)
        };

        let steps = 2usize.pow(disp.power as u32);
        let index = |x: usize, y: usize| y * (steps + 1) + x;
        for x in 0..steps {
            for y in 0..steps {
                let v00 = verts[index(x, y)];
                let v10 = verts[index(x + 1, y)];
                let v01 = verts[index(x, y + 1)];
                let v11 = verts[index(x + 1, y + 1)];
                push_tri(
                    &mut collision,
                    &mut render,
                    bsp,
                    face_idx,
                    face_ref,
                    lightmaps,
                    v00,
                    v10,
                    v01,
                    color,
                    tex_layer,
                    tex_h.as_ref(),
                );
                push_tri(
                    &mut collision,
                    &mut render,
                    bsp,
                    face_idx,
                    face_ref,
                    lightmaps,
                    v10,
                    v11,
                    v01,
                    color,
                    tex_layer,
                    tex_h.as_ref(),
                );
            }
        }
    }

    DispExtract {
        collision,
        render,
    }
}

/// Match vbsp's subdivided_face + displacement offset (see Handle displacement).
fn displaced_grid(
    bsp: &Bsp,
    disp: &vbsp::DisplacementInfo,
    corners_in: &[Vector],
) -> Option<Vec<Vec3>> {
    let mut corners: [Vector; 4] = [
        corners_in[0],
        corners_in[1],
        corners_in[2],
        corners_in[3],
    ];
    let start = disp.start_position;
    let mut best_i = 0usize;
    let mut best_d = f32::INFINITY;
    for (i, c) in corners.iter().enumerate() {
        let d = (*c - start).length_squared();
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    corners.rotate_left(best_i);

    let steps = 2usize.pow(disp.power as u32) + 1;
    let step_scale = 1.0 / (steps as f32 - 1.0);
    let edge_intervals = [
        (corners[1] - corners[0]) * step_scale,
        (corners[2] - corners[3]) * step_scale,
    ];

    let vert_start = disp.displacement_vertex_start as usize;
    let vert_count = disp.vertex_count() as usize;
    if vert_start + vert_count > bsp.displacement_vertices.len() {
        return None;
    }

    let mut out = Vec::with_capacity(steps * steps);
    let mut vi = 0usize;
    for x in 0..steps {
        for y in 0..steps {
            let edge_positions = [
                corners[0] + edge_intervals[0] * x as f32,
                corners[3] + edge_intervals[1] * x as f32,
            ];
            let segment_interval = (edge_positions[1] - edge_positions[0]) * step_scale;
            let base = edge_positions[0] + segment_interval * y as f32;
            let dvert = &bsp.displacement_vertices[vert_start + vi];
            let offset = dvert.vector * dvert.distance;
            out.push(Vec3::new(
                base.x + offset.x,
                base.y + offset.y,
                base.z + offset.z,
            ));
            vi += 1;
        }
    }
    Some(out)
}

fn push_tri(
    collision: &mut Vec<CollisionTri>,
    render: &mut Vec<Tri>,
    bsp: &Bsp,
    face_idx: usize,
    face: &Face,
    lightmaps: &LightmapBaker,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    color: [f32; 3],
    tex: u32,
    tex_h: Option<&Handle<'_, vbsp::TextureInfo>>,
) {
    let Some(col) = CollisionTri::from_points(a, b, c, TRI_THICKNESS) else {
        return;
    };
    let pa = Vector {
        x: a.x,
        y: a.y,
        z: a.z,
    };
    let pb = Vector {
        x: b.x,
        y: b.y,
        z: b.z,
    };
    let pc = Vector {
        x: c.x,
        y: c.y,
        z: c.z,
    };
    let (uv_a, uv_b, uv_c) = if let Some(th) = tex_h {
        (th.uv(pa), th.uv(pb), th.uv(pc))
    } else {
        ([0.0, 0.0], [0.0, 0.0], [0.0, 0.0])
    };
    let lm_a = lightmaps.lm_uv(bsp, face_idx, face, pa);
    let lm_b = lightmaps.lm_uv(bsp, face_idx, face, pb);
    let lm_c = lightmaps.lm_uv(bsp, face_idx, face, pc);
    let mut col_rgb = color;
    if tex == 0 {
        let n = col.planes[0].normal;
        let flat = n.z.clamp(0.0, 1.0);
        col_rgb = [
            0.35 + 0.15 * (1.0 - flat),
            0.40 + 0.25 * flat,
            0.28 + 0.05 * flat,
        ];
    }
    render.push(Tri {
        a,
        b,
        c,
        color: col_rgb,
        uv_a,
        uv_b,
        uv_c,
        lm_a,
        lm_b,
        lm_c,
        tex,
    });
    collision.push(col);
}

fn face_color_fallback(name: &str) -> [f32; 3] {
    let h = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    [
        0.35 + ((h) & 0xff) as f32 / 255.0 * 0.4,
        0.35 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.4,
        0.35 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.4,
    ]
}
