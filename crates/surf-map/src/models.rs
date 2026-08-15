//! Embedded Source static-prop models (`.mdl` + `.vvd` + `.dx90.vtx`).
//!
//! Cyberwave-style maps ship surf ramps as `solid=Physics` MDLs. We render those
//! and build TraceBox collision from the same mesh (`.phy` decode can replace this
//! later). Decorative props stay render-skip until full prop support.

use std::collections::HashMap;

use surf_core::graybox::{GrayboxMesh, Tri};
use surf_core::math::Vec3;
use surf_core::CollisionTri;
use vbsp::{Bsp, SolidType};
use vmdl::{Mdl, Model, Vtx, Vvd};

use crate::materials::MaterialBank;
use crate::pak::normalize_path;

/// Thin prism thickness for prop collision faces (matches displacements).
const PROP_TRI_THICKNESS: f32 = 2.0;

#[derive(Clone)]
struct LocalTri {
    positions: [Vec3; 3],
    uvs: [[f32; 2]; 3],
    texture: u32,
}

pub fn append_static_props(
    bsp: &Bsp,
    materials: &mut MaterialBank,
    mesh: &mut GrayboxMesh,
) -> usize {
    let mut cache: HashMap<u16, Option<Vec<LocalTri>>> = HashMap::new();
    let mut added = 0;

    for prop in &bsp.static_props.props.props {
        let model_type = prop.prop_type;
        let Some(model_name) = bsp.static_props.dict.name.get(model_type as usize) else {
            continue;
        };
        // Surf maps often ship their ramps as static MDLs among thousands of
        // decorative props. Render gameplay ramps now; full prop instancing is later.
        let name_l = model_name.as_str().to_ascii_lowercase();
        let is_ramp = name_l.contains("/ramps/")
            || (name_l.contains("ramp") && !name_l.contains("ramp_detail"));
        if !is_ramp {
            continue;
        }
        if !cache.contains_key(&model_type) {
            let decoded = decode_model(materials, model_name.as_str());
            cache.insert(model_type, decoded);
        }
        let Some(local_tris) = cache.get(&model_type).and_then(Option::as_ref) else {
            continue;
        };

        let origin = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
        for local in local_tris {
            let a = origin + rotate_source(local.positions[0], prop.angles);
            let b = origin + rotate_source(local.positions[1], prop.angles);
            let c = origin + rotate_source(local.positions[2], prop.angles);
            mesh.tris.push(Tri {
                a,
                b,
                c,
                color: [0.48, 0.54, 0.60],
                uv_a: local.uvs[0],
                uv_b: local.uvs[1],
                uv_c: local.uvs[2],
                // LightmapBaker reserves its first texel as white for unlit geometry.
                lm_a: [0.5, 0.5],
                lm_b: [0.5, 0.5],
                lm_c: [0.5, 0.5],
                tex: local.texture,
            });
            added += 1;
        }
    }
    added
}

/// Collision for `solid=Physics` static props (cyberwave / MDL ramps).
///
/// Uses the render mesh as a stand-in for `.phy` ledges — same transform as draw.
/// Only keeps upward-facing tris (surfable tops); full two-sided shells bury the
/// hull inside the mesh volume and trip Layer A burial gates.
pub fn extract_prop_collision(bsp: &Bsp, materials: &mut MaterialBank) -> Vec<CollisionTri> {
    let mut cache: HashMap<u16, Option<Vec<[Vec3; 3]>>> = HashMap::new();
    let mut out = Vec::new();

    for prop in &bsp.static_props.props.props {
        if !matches!(prop.solid, SolidType::Physics) {
            continue;
        }
        let model_type = prop.prop_type;
        let Some(model_name) = bsp.static_props.dict.name.get(model_type as usize) else {
            continue;
        };
        // Same gate as render: gameplay ramps only (skip light towers / portals).
        // Prefer `.../ramps/...` paths; also allow names with "ramp" but not
        // decorative `ramp_details` shells that fill volume and bury the hull.
        let name_l = model_name.as_str().to_ascii_lowercase();
        let is_ramp = name_l.contains("/ramps/")
            || (name_l.contains("ramp") && !name_l.contains("ramp_detail"));
        if !is_ramp {
            continue;
        }
        if !cache.contains_key(&model_type) {
            let decoded = decode_model(materials, model_name.as_str())
                .map(|tris| tris.into_iter().map(|t| t.positions).collect());
            cache.insert(model_type, decoded);
        }
        let Some(local_tris) = cache.get(&model_type).and_then(Option::as_ref) else {
            continue;
        };

        let origin = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
        for positions in local_tris {
            let a = origin + rotate_source(positions[0], prop.angles);
            let b = origin + rotate_source(positions[1], prop.angles);
            let c = origin + rotate_source(positions[2], prop.angles);
            let n = (b - a).cross(c - a);
            // Flip so the face normal points upward when the tri is a ramp top.
            let (a, b, c) = if n.z < 0.0 { (a, c, b) } else { (a, b, c) };
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len < 1e-5 {
                continue;
            }
            let nz = n.z / len;
            // Surfable / walkable tops only — skip walls, undersides, junk.
            if nz < 0.05 {
                continue;
            }
            if let Some(tri) = CollisionTri::from_points(a, b, c, PROP_TRI_THICKNESS) {
                out.push(tri);
            }
        }
    }
    out
}

fn decode_model(materials: &mut MaterialBank, model_path: &str) -> Option<Vec<LocalTri>> {
    let mdl_path = normalize_path(model_path);
    let stem = mdl_path.strip_suffix(".mdl")?;
    let mdl = Mdl::read(&materials.get_bytes(&mdl_path)?).ok()?;
    let vvd = Vvd::read(&materials.get_bytes(&format!("{stem}.vvd"))?).ok()?;
    let vtx = Vtx::read(&materials.get_bytes(&format!("{stem}.dx90.vtx"))?).ok()?;
    let model = Model::from_parts(mdl, vtx, vvd);

    let directories = model.texture_directories().to_vec();
    let skin = model.skin_tables().next();
    let mut out = Vec::new();

    for source_mesh in model.meshes() {
        let texture_name = skin
            .as_ref()
            .and_then(|table| table.texture(source_mesh.material_index()))
            .unwrap_or("");
        let texture = materials.resolve_model_texture(&directories, texture_name);
        let vertices: Vec<_> = source_mesh.vertices().collect();
        for triangle in vertices.chunks_exact(3) {
            let mut positions = [Vec3::ZERO; 3];
            let mut uvs = [[0.0; 2]; 3];
            for i in 0..3 {
                let position = model.apply_root_transform(triangle[i].position);
                positions[i] = Vec3::new(position.x, position.y, position.z);
                uvs[i] = triangle[i].texture_coordinates;
            }
            out.push(LocalTri {
                positions,
                uvs,
                texture,
            });
        }
    }
    Some(out)
}

fn rotate_source(v: Vec3, angles: vbsp::Angles) -> Vec3 {
    let (sp, cp) = angles.pitch.to_radians().sin_cos();
    let (sy, cy) = angles.yaw.to_radians().sin_cos();
    let (sr, cr) = angles.roll.to_radians().sin_cos();

    Vec3::new(
        v.x * (cp * cy) + v.y * (sr * sp * cy - cr * sy) + v.z * (cr * sp * cy + sr * sy),
        v.x * (cp * sy) + v.y * (sr * sp * sy + cr * cy) + v.z * (cr * sp * sy - sr * cy),
        v.x * -sp + v.y * (sr * cp) + v.z * (cr * cp),
    )
}
