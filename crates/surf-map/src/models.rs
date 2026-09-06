//! Embedded Source static-prop models (`.mdl` + `.vvd` + `.dx90.vtx`).
//!
//! Cyberwave-style maps ship surf ramps as `solid=Physics` MDLs. We render those
//! and build TraceBox collision from the same mesh (`.phy` decode can replace this
//! later). Decorative props stay render-skip until full prop support.

use std::collections::HashMap;

use surf_core::graybox::{GrayboxMesh, Tri};
use surf_core::math::Vec3;
use surf_core::{Aabb, CollisionTri};
use vbsp::{Bsp, SolidType};
use vmdl::{Mdl, Model, Vtx, Vvd};

use crate::materials::MaterialBank;
use crate::pak::normalize_path;

/// Thin prism thickness for prop collision faces (matches displacements).
const PROP_TRI_THICKNESS: f32 = 2.0;

/// Minimum face-normal Z kept when a prop has no `.phy` and we fall back to its
/// render mesh. A render mesh is a closed two-sided shell, so keeping the
/// undersides buries the hull; a `.phy` is the real collision surface and is
/// kept whole.
const RENDER_FALLBACK_MIN_NZ: f32 = 0.05;

/// One placed static prop, with the slice of `world.tris` its collision
/// occupies. Lets a trace hit be reported as "you hit `rock_c2` at (x,y,z)"
/// instead of a bare triangle index — the difference between a number and a
/// thing you can go look at in the map.
#[derive(Clone, Debug)]
pub struct PropInstance {
    /// Index into the BSP's static-prop lump — the join key back to the map file.
    pub index: usize,
    pub model: String,
    pub origin: Vec3,
    pub angles: [f32; 3],
    /// True when this prop contributed collision (`solid=Physics` and decodable).
    pub solid: bool,
    /// True when the prop lives in the map's 3D-skybox area, so it is neither
    /// drawn nor collided. Kept in the census rather than dropped, so tools can
    /// still answer "what is that?" about backdrop scenery.
    pub skybox: bool,
    /// True when its collision came from a `.phy` hull rather than the render mesh.
    pub from_phy: bool,
    /// Range into the prop-collision region, i.e. offsets from `prop_tri_start`.
    pub tris: std::ops::Range<usize>,
    /// World-space bounds of the collision triangles above.
    pub bounds: Aabb,
    /// Range into the map's draw mesh holding this prop's triangles.
    pub render_tris: std::ops::Range<usize>,
    /// World-space bounds of the prop as *drawn*. Often much larger than
    /// `bounds`: a `.phy` hull is a simplified shell, and a non-solid prop has
    /// no collision at all yet still occupies the view.
    pub render_bounds: Aabb,
}

/// Component-wise bounds accumulator. `surf-core`'s `Vec3` deliberately has no
/// `min`/`max`/`splat`; this stays local rather than widening the physics API.
#[derive(Clone, Copy)]
struct Bounds {
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Bounds {
    fn new() -> Self {
        Self {
            lo: [f32::MAX; 3],
            hi: [f32::MIN; 3],
        }
    }
    fn add(&mut self, p: Vec3) {
        for (i, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            self.lo[i] = self.lo[i].min(v);
            self.hi[i] = self.hi[i].max(v);
        }
    }
    fn finish(self, fallback: Vec3) -> Aabb {
        if self.lo[0] > self.hi[0] {
            return Aabb::from_mins_maxs(fallback, fallback);
        }
        Aabb::from_mins_maxs(
            Vec3::new(self.lo[0], self.lo[1], self.lo[2]),
            Vec3::new(self.hi[0], self.hi[1], self.hi[2]),
        )
    }
}

#[derive(Clone)]
struct LocalTri {
    positions: [Vec3; 3],
    uvs: [[f32; 2]; 3],
    texture: u32,
}

/// Appends every decodable static prop to the draw mesh, and reports the world
/// bounds each one occupies. The bounds are what answers "which prop is that,
/// and is it sticking through the ramp?" — a question about what is *drawn*,
/// which is not the same geometry as what is collided.
/// Marks props the compiler placed in the 3D-skybox area, which we do not draw
/// or collide: they are backdrop scenery meant to be rendered at 1/scale around
/// the sky camera, not objects in the map.
///
/// A prop spans one or more leaves; it counts as skybox only when *every* leaf
/// it touches is in that area, so anything straddling the boundary is kept.
pub fn skybox_prop_mask(bsp: &Bsp, leaf_areas: &[u16], sky_area: Option<u16>) -> Vec<bool> {
    let props = &bsp.static_props.props.props;
    // A/B escape hatch, matching MX_SURF_NO_PHY: proves a change in the world
    // came from this and not from something else that moved at the same time.
    if std::env::var_os("MX_SURF_NO_SKYBOX_CULL").is_some() {
        return vec![false; props.len()];
    }
    let Some(sky_area) = sky_area else {
        return vec![false; props.len()];
    };
    let leaf_refs = &bsp.static_props.leaf.leaves;
    let mask: Vec<bool> = props
        .iter()
        .map(|prop| {
            let first = prop.first_leaf as usize;
            let end = first + prop.leaf_count as usize;
            let Some(refs) = leaf_refs.get(first..end) else {
                return false;
            };
            !refs.is_empty()
                && refs
                    .iter()
                    .all(|&leaf| leaf_areas.get(leaf as usize).copied() == Some(sky_area))
        })
        .collect();
    if std::env::var_os("MX_SURF_PROP_DEBUG").is_some() {
        let mut solid = 0;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (i, prop) in props.iter().enumerate() {
            if !mask[i] {
                continue;
            }
            if matches!(prop.solid, SolidType::Physics) {
                solid += 1;
                eprintln!(
                    "  SKYCULL-SOLID {} at ({:.0},{:.0},{:.0})",
                    bsp.static_props
                        .dict
                        .name
                        .get(prop.prop_type as usize)
                        .map(|n| n.as_str())
                        .unwrap_or("?"),
                    prop.origin.x,
                    prop.origin.y,
                    prop.origin.z
                );
            }
            lo = lo.min(prop.origin.z);
            hi = hi.max(prop.origin.z);
        }
        eprintln!(
            "  SKYCULL area={sky_area}: {} of {} props culled ({solid} solid), z {lo:.0}..{hi:.0}",
            mask.iter().filter(|m| **m).count(),
            props.len()
        );
    }
    mask
}

pub fn append_static_props(
    bsp: &Bsp,
    materials: &mut MaterialBank,
    mesh: &mut GrayboxMesh,
    skybox: &[bool],
) -> (usize, Vec<(std::ops::Range<usize>, Aabb)>) {
    let mut cache: HashMap<u16, Option<Vec<LocalTri>>> = HashMap::new();
    let mut added = 0;
    let mut render_bounds: Vec<(std::ops::Range<usize>, Aabb)> =
        vec![
            (0..0, Aabb::from_mins_maxs(Vec3::ZERO, Vec3::ZERO));
            bsp.static_props.props.props.len()
        ];

    for (prop_index, prop) in bsp.static_props.props.props.iter().enumerate() {
        if skybox.get(prop_index).copied().unwrap_or(false) {
            continue;
        }
        let model_type = prop.prop_type;
        let Some(model_name) = bsp.static_props.dict.name.get(model_type as usize) else {
            continue;
        };
        if !render_prop(model_name.as_str()) {
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
        let mut bounds = Bounds::new();
        let draw_start = mesh.tris.len();
        for local in local_tris {
            let a = origin + rotate_source(local.positions[0], prop.angles);
            let b = origin + rotate_source(local.positions[1], prop.angles);
            let c = origin + rotate_source(local.positions[2], prop.angles);
            bounds.add(a);
            bounds.add(b);
            bounds.add(c);
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
        render_bounds[prop_index] = (draw_start..mesh.tris.len(), bounds.finish(origin));
    }
    (added, render_bounds)
}

/// Collision for `solid=Physics` static props (cyberwave / MDL ramps).
///
/// Uses the render mesh as a stand-in for `.phy` ledges — same transform as draw.
/// Only keeps upward-facing tris (surfable tops); full two-sided shells bury the
/// hull inside the mesh volume and trip Layer A burial gates.
pub fn extract_prop_collision(
    bsp: &Bsp,
    materials: &mut MaterialBank,
    skybox: &[bool],
) -> (Vec<CollisionTri>, Vec<PropInstance>) {
    let mut cache: HashMap<u16, Option<PropCollision>> = HashMap::new();
    let mut out = Vec::new();
    let mut census: Vec<PropInstance> = Vec::new();
    // MX_SURF_PROP_DEBUG=1 reports, per model, how much of its render mesh
    // survives the upward-facing filter — the filter that decides whether an
    // MDL ramp is a surface or a set of holes.
    let debug = std::env::var_os("MX_SURF_PROP_DEBUG").is_some();
    let mut stats: HashMap<String, (usize, usize, usize, f32, bool)> = HashMap::new();

    for (prop_index, prop) in bsp.static_props.props.props.iter().enumerate() {
        let model_type = prop.prop_type;
        let Some(model_name) = bsp.static_props.dict.name.get(model_type as usize) else {
            continue;
        };
        let origin = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
        let angles = [prop.angles.pitch, prop.angles.yaw, prop.angles.roll];
        // Every prop is censused, solid or not: "which rock is that?" is a
        // question about what is drawn, and the answer is often a prop we chose
        // not to collide.
        if skybox.get(prop_index).copied().unwrap_or(false) {
            census.push(PropInstance {
                index: prop_index,
                model: model_name.as_str().to_string(),
                origin,
                angles,
                solid: false,
                skybox: true,
                from_phy: false,
                tris: out.len()..out.len(),
                render_tris: 0..0,
                bounds: Aabb::from_mins_maxs(origin, origin),
                render_bounds: Aabb::from_mins_maxs(origin, origin),
            });
            continue;
        }
        let declared_solid =
            matches!(prop.solid, SolidType::Physics) && collide_prop(model_name.as_str());
        if !cache.contains_key(&model_type) {
            cache.insert(
                model_type,
                decode_prop_collision(materials, model_name.as_str()),
            );
        }
        let Some(collision) = cache.get(&model_type).and_then(Option::as_ref) else {
            census.push(PropInstance {
                index: prop_index,
                model: model_name.as_str().to_string(),
                origin,
                angles,
                solid: false,
                skybox: false,
                from_phy: false,
                tris: out.len()..out.len(),
                bounds: Aabb::from_mins_maxs(origin, origin),
                render_tris: 0..0,
                render_bounds: Aabb::from_mins_maxs(origin, origin),
            });
            continue;
        };
        // Source builds a static prop's collision from the model's `.phy`
        // vcollide and from nothing else — it never traces a render mesh. A
        // model that ships no `.phy` therefore has no collision at all, whatever
        // the lump's solid type says, and mappers leave `prop_static` on its
        // default "Use VPhysics" freely because in-engine that costs nothing.
        // botanica is the case in point: 971 props marked `solid=Physics`, not
        // one of them with a `.phy`, including the grass, flowers and ivy — and
        // the render-mesh fallback turned them into 1.44M triangles of invisible
        // fence across every stage-end portal.
        let solid = declared_solid && (collision.from_phy || render_mesh_collision());
        if !solid {
            let mut bounds = Bounds::new();
            for positions in &collision.tris {
                for p in positions {
                    bounds.add(origin + rotate_source(*p, prop.angles));
                }
            }
            let bounds = bounds.finish(origin);
            census.push(PropInstance {
                index: prop_index,
                model: model_name.as_str().to_string(),
                origin,
                angles,
                solid: false,
                skybox: false,
                from_phy: collision.from_phy,
                tris: out.len()..out.len(),
                bounds,
                render_tris: 0..0,
                render_bounds: Aabb::from_mins_maxs(origin, origin),
            });
            continue;
        }
        let local_tris = &collision.tris;
        let from_phy = collision.from_phy;
        // A/B lever shared with `phy::decode_collision`: the old behaviour.
        let phy_raw = std::env::var_os("MX_SURF_PHY_RAW").is_some();

        let inst_start = out.len();
        let entry = stats
            .entry(model_name.as_str().to_string())
            .or_insert((0, 0, 0, 1.0f32, false));
        entry.0 += 1;
        entry.4 = materials
            .get_bytes(&format!(
                "{}.phy",
                normalize_path(model_name.as_str())
                    .strip_suffix(".mdl")
                    .unwrap_or("")
            ))
            .is_some();
        for positions in local_tris {
            let a = origin + rotate_source(positions[0], prop.angles);
            let b = origin + rotate_source(positions[1], prop.angles);
            let c = origin + rotate_source(positions[2], prop.angles);
            let n = (b - a).cross(c - a);
            // A `.phy` triangle already faces out of its convex piece (see
            // `phy::surface_of`); flipping it by z would turn a seam cap or an
            // underside into a wall facing oncoming traffic. The render-mesh
            // fallback has no reliable winding, so there we still flip so the
            // face normal points upward when the tri is a ramp top.
            let (a, b, c) = if (!from_phy || phy_raw) && n.z < 0.0 { (a, c, b) } else { (a, b, c) };
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len < 1e-5 {
                continue;
            }
            let nz = n.z / len;
            entry.1 += 1;
            entry.3 = entry.3.min(nz);
            // A `.phy` IS the collision surface — keep all of it, walls included
            // (boreas' start deck is a floor plate plus a 776u starting cage).
            // The render-mesh fallback is a closed two-sided shell instead, so
            // there we still keep only upward faces or the hull ends up buried.
            if !from_phy && nz < RENDER_FALLBACK_MIN_NZ {
                continue;
            }
            entry.2 += 1;
            if let Some(tri) = CollisionTri::from_points(a, b, c, PROP_TRI_THICKNESS) {
                out.push(tri);
            }
        }
        let mut bounds = Bounds::new();
        for tri in &out[inst_start..] {
            bounds.add(tri.bounds.mins);
            bounds.add(tri.bounds.maxs);
        }
        let bounds = bounds.finish(origin);
        census.push(PropInstance {
            index: prop_index,
            model: model_name.as_str().to_string(),
            origin,
            angles,
            solid: out.len() > inst_start,
            skybox: false,
            from_phy,
            tris: inst_start..out.len(),
            bounds,
            render_tris: 0..0,
            render_bounds: Aabb::from_mins_maxs(origin, origin),
        });
        if debug {
            eprintln!(
                "  PROPRANGE {}..{} {} at ({:.0},{:.0},{:.0}) ang=({:.0},{:.0},{:.0})",
                inst_start,
                out.len(),
                model_name.as_str(),
                origin.x,
                origin.y,
                origin.z,
                prop.angles.pitch,
                prop.angles.yaw,
                prop.angles.roll
            );
        }
    }
    if debug {
        eprintln!("-- solid prop collision (render-mesh stand-in for .phy) --");
        let mut rows: Vec<_> = stats.into_iter().collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, (instances, faces, kept, min_nz, has_phy)) in rows {
            let per = if instances > 0 { faces / instances } else { 0 };
            eprintln!(
                "  {name}: x{instances} mesh_tris/inst={per} kept={kept} dropped={} min_nz={min_nz:.3} phy={has_phy}",
                faces - kept
            );
        }
    }
    (out, census)
}

/// Static props the mapper marked `solid=Physics` are world geometry as far as
/// the player is concerned — surf decks, ledges, ramp shells. The old gate only
/// admitted paths containing "ramp", which dropped e.g. boreas'
/// `details/dek01.mdl` **start platform** (the player spawned over open terrain
/// and fell 235u onto a slope). Solidity is the mapper's own signal; trust it.
///
/// The exclusions below are decorative shells that are marked solid but whose
/// render mesh is a closed volume — using it as collision buries the hull.
fn collide_prop(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.contains("ramp_detail")
}

/// Render every prop we can decode. Skipping non-ramps left maps looking like
/// bare terrain (boreas: 1587 props, all invisible).
///
/// `MX_SURF_HIDE_PROPS` takes a comma-separated list of path substrings and
/// drops those from the draw mesh only — collision is untouched. It exists so a
/// "is that thing supposed to be there?" question can be answered by rendering
/// the same view twice, which is cheaper than arguing about a screenshot.
fn render_prop(name: &str) -> bool {
    let Some(hide) = std::env::var_os("MX_SURF_HIDE_PROPS") else {
        return true;
    };
    let hide = hide.to_string_lossy().to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    !hide
        .split(',')
        .map(str::trim)
        .any(|pat| !pat.is_empty() && name.contains(pat))
}

/// A/B lever restoring the pre-2026-09-06 behaviour: collide `solid=Physics`
/// props that ship no `.phy` against their render mesh. Source never does this;
/// the switch exists so a corpus A/B can be run against the old geometry.
fn render_mesh_collision() -> bool {
    std::env::var_os("MX_SURF_PROP_RENDER_COLLISION").is_some()
}

/// Collision triangles for one prop model, in model space.
struct PropCollision {
    tris: Vec<[Vec3; 3]>,
    /// True when these came from the model's `.phy` collision hull rather than
    /// its render mesh. Decides whether the upward-facing filter applies.
    from_phy: bool,
}

/// Prefer the model's real `.phy` collision hull; fall back to the render mesh.
fn decode_prop_collision(materials: &mut MaterialBank, model_path: &str) -> Option<PropCollision> {
    let normalized = normalize_path(model_path);
    // A/B escape hatch while the .phy path is being evaluated.
    let allow_phy = std::env::var_os("MX_SURF_NO_PHY").is_none();
    if let Some(stem) = normalized.strip_suffix(".mdl").filter(|_| allow_phy) {
        if let Some(bytes) = materials.get_bytes(&format!("{stem}.phy")) {
            if let Some(tris) = crate::phy::decode_collision(&bytes) {
                if std::env::var_os("MX_SURF_PROP_DEBUG").is_some() {
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for t in &tris {
                        for p in t {
                            for (i, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                                lo[i] = lo[i].min(v);
                                hi[i] = hi[i].max(v);
                            }
                        }
                    }
                    eprintln!(
                        "  PHYAABB {model_path}: tris={} x[{:.1},{:.1}] y[{:.1},{:.1}] z[{:.1},{:.1}]",
                        tris.len(), lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
                    );
                }
                return Some(PropCollision {
                    tris,
                    from_phy: true,
                });
            }
        }
    }
    let tris = decode_model(materials, model_path)?
        .into_iter()
        .map(|t| t.positions)
        .collect();
    Some(PropCollision {
        tris,
        from_phy: false,
    })
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
    if std::env::var_os("MX_SURF_PROP_DEBUG").is_some() {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for t in &out {
            for p in &t.positions {
                for (i, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(v);
                    hi[i] = hi[i].max(v);
                }
            }
        }
        eprintln!(
            "  RENDERAABB {model_path}: tris={} x[{:.1},{:.1}] y[{:.1},{:.1}] z[{:.1},{:.1}]",
            out.len(),
            lo[0],
            hi[0],
            lo[1],
            hi[1],
            lo[2],
            hi[2]
        );
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
