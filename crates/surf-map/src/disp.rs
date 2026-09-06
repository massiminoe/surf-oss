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
    /// For each collision tri, the index of the `ddispinfo_t` it came from.
    pub owner: Vec<u32>,
    /// Per displacement (BSP order): Hammer's displacement flags. vbsp stores
    /// them in `minTess` with the high bit set ("If the high bit is set - this
    /// is FLAGS!"); the low bits are `CCoreDispInfo`'s surface flags.
    pub flags: Vec<u32>,
    pub render: Vec<Tri>,
}

/// Hammer "No Physics Collision": VPhysics objects ignore the surface.
pub const DISP_NOPHYSICS_COLL: u32 = 0x2;
/// Hammer "No Hull Collision": player / NPC hull traces ignore the surface.
pub const DISP_NOHULL_COLL: u32 = 0x4;
/// Hammer "No Ray Collision": bullets and line traces ignore the surface.
pub const DISP_NORAY_COLL: u32 = 0x8;

/// The displacement's flag bits, or 0 when `minTess` is a real tessellation value.
pub fn disp_flags(disp: &vbsp::DisplacementInfo) -> u32 {
    let raw = disp.minimum_tesselation as u32;
    if raw & 0x8000_0000 != 0 {
        raw & 0x7fff_ffff
    } else {
        0
    }
}

pub fn extract_displacements(
    bsp: &Bsp,
    materials: &mut MaterialBank,
    lightmaps: &mut LightmapBaker,
) -> DispExtract {
    let mut collision = Vec::new();
    let mut owner = Vec::new();
    let mut render = Vec::new();
    let mask = mask_playersolid();
    let flags: Vec<u32> = bsp.displacements.iter().map(disp_flags).collect();

    for (disp_idx, disp) in bsp.displacements.iter().enumerate() {
        let contents = BrushFlags::from_bits_truncate(disp.contents as u32);
        if !contents.intersects(mask) {
            continue;
        }
        // Hammer's "No Hull Collision": Source's `CM_TraceToDispTree` skips
        // the displacement for every hull (non-ray) trace, which is what the
        // player is. It is still drawn.
        let hull_solid = flags[disp_idx] & DISP_NOHULL_COLL == 0 || ignore_nohull();
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
        let Some((verts, alphas)) = displaced_grid(bsp, disp, &corners) else {
            continue;
        };

        lightmaps.ensure_face(bsp, face_idx, face_ref);
        let tex_info = if face_ref.texture_info >= 0 {
            bsp.textures_info.get(face_ref.texture_info as usize)
        } else {
            None
        };
        let (tex_layer, tex2_layer, color, tex_h) = if let Some(tex) = tex_info {
            let tex_h = Handle::new(bsp, tex);
            let layer = materials.resolve(tex_h.name());
            // `WorldVertexTransition`: the displacement paints between two
            // textures with per-vertex alpha. Only meaningful with a real
            // first layer.
            let layer2 = if layer > 0 {
                materials.resolve_secondary(tex_h.name())
            } else {
                0
            };
            let color = face_color_fallback(tex_h.name());
            (layer, layer2, color, Some(tex_h))
        } else {
            (0, 0, [0.45, 0.50, 0.40], None)
        };

        let steps = 2usize.pow(disp.power as u32);
        let index = |x: usize, y: usize| y * (steps + 1) + x;
        for x in 0..steps {
            for y in 0..steps {
                let v00 = (verts[index(x, y)], alphas[index(x, y)]);
                let v10 = (verts[index(x + 1, y)], alphas[index(x + 1, y)]);
                let v01 = (verts[index(x, y + 1)], alphas[index(x, y + 1)]);
                let v11 = (verts[index(x + 1, y + 1)], alphas[index(x + 1, y + 1)]);
                // Source splits each quad on a checkerboard
                // (`CCoreDispInfo::GenerateCollisionSurface`): an odd
                // `row*width + col` — i.e. odd x+y, width being odd — runs the
                // diagonal from (x+1,y) to (x,y+1), an even one from (x,y) to
                // (x+1,y+1). Its collision tree is built from those same
                // triangles, so on a lumpy rock face the wrong diagonal moves
                // the surface by tens of units mid-cell.
                let anti = (x + y) % 2 == 1 || single_diagonal();
                let (t1, t2) = if anti {
                    ([v00, v10, v01], [v10, v11, v01])
                } else {
                    ([v00, v11, v01], [v00, v10, v11])
                };
                for [a, b, c] in [t1, t2] {
                    push_tri(
                        hull_solid,
                        &mut collision,
                        &mut owner,
                        disp_idx as u32,
                        &mut render,
                        bsp,
                        face_idx,
                        face_ref,
                        lightmaps,
                        a.0,
                        b.0,
                        c.0,
                        [a.1, b.1, c.1],
                        color,
                        tex_layer,
                        tex2_layer,
                        tex_h.as_ref(),
                    );
                }
            }
        }
    }

    DispExtract {
        collision,
        owner,
        flags,
        render,
    }
}

/// A/B lever: `MX_SURF_DISP_IGNORE_NOHULL=1` collides with displacements the
/// mapper flagged "No Hull Collision", as the loader did before 2026-09-02.
fn ignore_nohull() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var_os("MX_SURF_DISP_IGNORE_NOHULL").is_some())
}

/// A/B lever: `MX_SURF_DISP_SINGLE_DIAGONAL=1` restores the pre-2026-09-02
/// triangulation (every quad split on the same diagonal).
fn single_diagonal() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var_os("MX_SURF_DISP_SINGLE_DIAGONAL").is_some())
}

/// Match vbsp's subdivided_face + displacement offset (see Handle displacement).
fn displaced_grid(
    bsp: &Bsp,
    disp: &vbsp::DisplacementInfo,
    corners_in: &[Vector],
) -> Option<(Vec<Vec3>, Vec<f32>)> {
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
    let mut alphas = Vec::with_capacity(steps * steps);
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
            // Hammer paints blend alpha 0..255 per displacement vertex.
            alphas.push((dvert.alpha / 255.0).clamp(0.0, 1.0));
            vi += 1;
        }
    }
    Some((out, alphas))
}

fn push_tri(
    hull_solid: bool,
    collision: &mut Vec<CollisionTri>,
    owner: &mut Vec<u32>,
    disp_idx: u32,
    render: &mut Vec<Tri>,
    bsp: &Bsp,
    face_idx: usize,
    face: &Face,
    lightmaps: &LightmapBaker,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    alpha: [f32; 3],
    color: [f32; 3],
    tex: u32,
    tex2: u32,
    tex_h: Option<&Handle<'_, vbsp::TextureInfo>>,
) {
    let col = if hull_solid {
        let Some(col) = CollisionTri::from_points(a, b, c, TRI_THICKNESS) else {
            return;
        };
        Some(col)
    } else {
        None
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
        let n = (b - a).cross(c - a);
        let flat = (n.z / n.length().max(1e-6)).clamp(0.0, 1.0);
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
        tex2,
        alpha,
        light: [[1.0; 3]; 3],
    });
    if let Some(col) = col {
        collision.push(col);
        owner.push(disp_idx);
    }
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
