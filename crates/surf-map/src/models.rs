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
    /// Where the prop's per-vertex light came from.
    pub lighting: PropLighting,
}

/// How a drawn static prop is lit. Source never lightmaps a prop: vrad bakes
/// per-vertex light into `sp_<index>.vhv` in the pakfile when the map was
/// compiled with `-StaticPropLighting`, and otherwise the engine lights the
/// model from its leaf's ambient cube (plus local lights we do not model).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropLighting {
    /// Not drawn (skybox, undecodable) or lit at unity.
    Unlit,
    /// Per-vertex light from the map's `.vhv`.
    Baked,
    /// Ambient cube of the leaf at the prop origin, per vertex normal.
    Ambient,
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
    normals: [Vec3; 3],
    uvs: [[f32; 2]; 3],
    /// Hardware vertex index (strip-group upload order) — the `.vhv` join key.
    vert_ids: [u32; 3],
    texture: u32,
}

/// A decoded model: its draw triangles plus the vertex count the `.vhv` has to
/// match before its colours can be trusted.
struct DecodedModel {
    tris: Vec<LocalTri>,
    vertex_count: usize,
    /// Hardware (strip-group order) vertex count of LOD 0 — the `.vhv` join.
    /// MDL `illumposition`, model space: where Source samples ambient light.
    illumination_position: Vec3,
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
    // A/B escape hatch, matching SURF_OSS_NO_PHY: proves a change in the world
    // came from this and not from something else that moved at the same time.
    if std::env::var_os("SURF_OSS_NO_SKYBOX_CULL").is_some() {
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
    if std::env::var_os("SURF_OSS_PROP_DEBUG").is_some() {
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
    bsp_bytes: &[u8],
    materials: &mut MaterialBank,
    mesh: &mut GrayboxMesh,
    skybox: &[bool],
) -> (usize, Vec<(std::ops::Range<usize>, Aabb, PropLighting)>) {
    let mut cache: HashMap<u16, Option<DecodedModel>> = HashMap::new();
    let mut added = 0;
    let mut render_bounds: Vec<(std::ops::Range<usize>, Aabb, PropLighting)> = vec![
        (
            0..0,
            Aabb::from_mins_maxs(Vec3::ZERO, Vec3::ZERO),
            PropLighting::Unlit
        );
        bsp.static_props.props.props.len()
    ];
    let ambient = if std::env::var_os("SURF_OSS_NO_PROP_LIGHT").is_some() {
        None
    } else {
        crate::ambient::AmbientCubes::parse(bsp_bytes)
    };
    let baked_allowed = std::env::var_os("SURF_OSS_NO_PROP_LIGHT").is_none()
        && std::env::var_os("SURF_OSS_NO_VHV").is_none();
    let world_lights = if ambient.is_some() {
        crate::world_lights::WorldLights::parse(bsp_bytes)
    } else {
        crate::world_lights::WorldLights::default()
    };
    let sun = world_lights.sun();
    // The lightmap baker reserves its first texel as "L = 1" for geometry
    // whose light is not in the atlas; a prop's light rides per vertex.
    let unit_lm = [0.5, 0.5];

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
        let Some(decoded) = cache.get(&model_type).and_then(Option::as_ref) else {
            continue;
        };

        let origin = Vec3::new(prop.origin.x, prop.origin.y, prop.origin.z);
        let baked = if baked_allowed {
            materials
                .get_bytes(&format!("sp_{prop_index}.vhv"))
                .and_then(|b| decode_vhv(&b, decoded.vertex_count))
        } else {
            None
        };
        let cube = if baked.is_none() {
            ambient.as_ref().and_then(|a| {
                // Source samples the cube at the model's illumination position.
                // A prop whose origin is sunk into the ground (rocks, corals)
                // lands in a solid leaf, whose cube is all zeros; walk upward
                // until a lit leaf answers.
                let illum = origin + rotate_source(decoded.illumination_position, prop.angles);
                [0.0, 8.0, 32.0, 96.0, 256.0]
                    .into_iter()
                    .filter_map(|dz| a.cube_at(bsp_bytes, [illum.x, illum.y, illum.z + dz]))
                    .find(|c| c.iter().flatten().any(|v| *v > 0.0))
                    .or_else(|| a.average())
            })
        } else {
            None
        };
        let locals = if cube.is_some() {
            world_lights.local_lights_at(origin)
        } else {
            Vec::new()
        };
        // Direct sun on an ambient-lit prop, gated by whether this cube can
        // see the sky in the sun's direction (see `parse_sun`).
        let sun_here = match (&cube, &sun) {
            (Some(cube), Some(sun)) if sun.visible_from(cube) => Some(sun),
            _ => None,
        };
        let lighting = if baked.is_some() {
            PropLighting::Baked
        } else if cube.is_some() {
            PropLighting::Ambient
        } else {
            PropLighting::Unlit
        };
        let mut bounds = Bounds::new();
        let draw_start = mesh.tris.len();
        for local in &decoded.tris {
            let a = origin + rotate_source(local.positions[0], prop.angles);
            let b = origin + rotate_source(local.positions[1], prop.angles);
            let c = origin + rotate_source(local.positions[2], prop.angles);
            bounds.add(a);
            bounds.add(b);
            bounds.add(c);
            let mut light = [[1.0f32; 3]; 3];
            if let Some(colors) = &baked {
                for (i, id) in local.vert_ids.iter().enumerate() {
                    light[i] = colors[*id as usize];
                }
            } else if let Some(cube) = &cube {
                for (i, n) in local.normals.iter().enumerate() {
                    let wn = rotate_source(*n, prop.angles);
                    light[i] = crate::ambient::eval(cube, [wn.x, wn.y, wn.z]);
                    if let Some(sun) = sun_here {
                        let lambert = wn.dot(sun.to_sun).max(0.0);
                        for c in 0..3 {
                            light[i][c] += sun.color[c] * lambert;
                        }
                    }
                    let p = origin + rotate_source(local.positions[i], prop.angles);
                    for wl in &locals {
                        let lambert = wn.dot(wl.direction_from(p)).max(0.0);
                        if lambert <= 0.0 {
                            continue;
                        }
                        let e = wl.irradiance_at(p);
                        for c in 0..3 {
                            light[i][c] += e[c] * lambert;
                        }
                    }
                }
            }
            mesh.tris.push(Tri {
                a,
                b,
                c,
                color: [0.48, 0.54, 0.60],
                uv_a: local.uvs[0],
                uv_b: local.uvs[1],
                uv_c: local.uvs[2],
                lm_a: unit_lm,
                lm_b: unit_lm,
                lm_c: unit_lm,
                tex: local.texture,
                tex2: 0,
                alpha: [0.0; 3],
                light,
            });
            added += 1;
        }
        render_bounds[prop_index] = (draw_start..mesh.tris.len(), bounds.finish(origin), lighting);
        if std::env::var_os("SURF_OSS_PROP_DEBUG").is_some() && prop_index % 25 == 0 {
            let n = (mesh.tris.len() - draw_start).max(1) as f32 * 3.0;
            let mean = mesh.tris[draw_start..]
                .iter()
                .flat_map(|t| t.light.iter())
                .fold([0.0f32; 3], |a, l| [a[0] + l[0] / n, a[1] + l[1] / n, a[2] + l[2] / n]);
            eprintln!(
                "  PROPLIGHT #{prop_index} {model_name} {lighting:?} mean=({:.3},{:.3},{:.3}) cube+z={:?} sun={}",
                mean[0],
                mean[1],
                mean[2],
                cube.map(|c| c[4]),
                sun_here.is_some()
            );
        }
    }
    (added, render_bounds)
}

/// vrad's per-vertex static-prop lighting (`sp_<index>.vhv`, "hardware
/// verts"): a 40-byte header (version 2, checksum, vertex flags, vertex size,
/// vertex count, mesh count), then 28-byte mesh headers (lod, vertex count,
/// byte offset), then 4-byte BGRA colours. The header's vertex count spans
/// every LOD; only the LOD-0 mesh headers are read, and vertex `i` of their
/// concatenation is LOD-0 VVD vertex `i`, which is what `LocalTri::vert_ids`
/// holds (vmdl resolves the VVD fixups to the LOD-0 list).
///
/// Colours are stored the way the LDR lightmap page is — gamma-space, halved
/// for overbright — so a byte `b` means linear `(2b/255)^2.2`. Calibrated
/// against summit, where the bytes top out at 125 on sunlit rocks (the
/// lightmap's lit luxels reach L ≈ 1) — see the module doc on `lightmap.rs`.
/// Byte order was settled the same way: read as RGB the torches lit blue and
/// the leaf cubes around them are orange; as BGR they agree.
fn decode_vhv(bytes: &[u8], expected_vertices: usize) -> Option<Vec<[f32; 3]>> {
    if bytes.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let version = u32_at(0);
    let vertex_size = u32_at(12) as usize;
    let mesh_count = u32_at(20) as usize;
    if version != 2 || vertex_size != 4 || expected_vertices == 0 || mesh_count > 4096 {
        return None;
    }
    let vertex_count = expected_vertices;
    let mut out = Vec::with_capacity(vertex_count);
    let decode = |b: u8| -> f32 { (2.0 * b as f32 / 255.0).powf(2.2) };
    for m in 0..mesh_count {
        let h = 40 + m * 28;
        if h + 12 > bytes.len() {
            return None;
        }
        let lod = u32_at(h);
        let count = u32_at(h + 4) as usize;
        let offset = u32_at(h + 8) as usize;
        if lod != 0 {
            continue;
        }
        if offset + count * 4 > bytes.len() {
            return None;
        }
        for i in 0..count {
            let px = &bytes[offset + i * 4..offset + i * 4 + 4];
            out.push([decode(px[2]), decode(px[1]), decode(px[0])]);
        }
        if out.len() >= vertex_count {
            break;
        }
    }
    // An empty file (header count 0) is vrad saying "no vertex lighting for
    // this one"; the ambient path covers it silently.
    if std::env::var_os("SURF_OSS_PROP_DEBUG").is_some() && out.len() < vertex_count && mesh_count > 0 {
        eprintln!(
            "  VHV lod0 colours {} < needed {} (header count {}, meshes {})",
            out.len(),
            vertex_count,
            u32_at(16),
            mesh_count
        );
    }
    (out.len() >= vertex_count).then_some(out)
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
    // SURF_OSS_PROP_DEBUG=1 reports, per model, how much of its render mesh
    // survives the upward-facing filter — the filter that decides whether an
    // MDL ramp is a surface or a set of holes.
    let debug = std::env::var_os("SURF_OSS_PROP_DEBUG").is_some();
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
            lighting: PropLighting::Unlit,
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
            lighting: PropLighting::Unlit,
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
            lighting: PropLighting::Unlit,
            });
            continue;
        }
        let local_tris = &collision.tris;
        let from_phy = collision.from_phy;
        // A/B lever shared with `phy::decode_collision`: the old behaviour.
        let phy_raw = std::env::var_os("SURF_OSS_PHY_RAW").is_some();

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
            lighting: PropLighting::Unlit,
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
/// `SURF_OSS_HIDE_PROPS` takes a comma-separated list of path substrings and
/// drops those from the draw mesh only — collision is untouched. It exists so a
/// "is that thing supposed to be there?" question can be answered by rendering
/// the same view twice, which is cheaper than arguing about a screenshot.
fn render_prop(name: &str) -> bool {
    let Some(hide) = std::env::var_os("SURF_OSS_HIDE_PROPS") else {
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
    std::env::var_os("SURF_OSS_PROP_RENDER_COLLISION").is_some()
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
    let allow_phy = std::env::var_os("SURF_OSS_NO_PHY").is_none();
    if let Some(stem) = normalized.strip_suffix(".mdl").filter(|_| allow_phy) {
        if let Some(bytes) = materials.get_bytes(&format!("{stem}.phy")) {
            if let Some(tris) = crate::phy::decode_collision(&bytes) {
                if std::env::var_os("SURF_OSS_PROP_DEBUG").is_some() {
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
        .tris
        .into_iter()
        .map(|t| t.positions)
        .collect();
    Some(PropCollision {
        tris,
        from_phy: false,
    })
}

fn decode_model(materials: &mut MaterialBank, model_path: &str) -> Option<DecodedModel> {
    let mdl_path = normalize_path(model_path);
    let stem = mdl_path.strip_suffix(".mdl")?;
    let mdl = Mdl::read(&materials.get_bytes(&mdl_path)?).ok()?;
    let illum = mdl.header.illumination_position;
    let illumination_position = Vec3::new(illum.x, illum.y, illum.z);
    let vvd = Vvd::read(&materials.get_bytes(&format!("{stem}.vvd"))?).ok()?;
    let vtx = Vtx::read(&materials.get_bytes(&format!("{stem}.dx90.vtx"))?).ok()?;
    // Meshes in the order vmdl (and the engine) pair them: MDL meshes zipped
    // with the VTX LOD-0 meshes, body part by body part.
    let mdl_meshes: Vec<(&vmdl::mdl::Mesh, i32)> = mdl
        .body_parts
        .iter()
        .flat_map(|part| part.models.iter())
        .flat_map(|model| model.meshes.iter().map(move |m| (m, model.vertex_offset)))
        .collect();
    let vtx_meshes: Vec<&vmdl::vtx::Mesh> = vtx
        .body_parts
        .iter()
        .flat_map(|part| part.models.iter())
        .flat_map(|model| model.lods.first())
        .flat_map(|lod| lod.meshes.iter())
        .collect();
    let model = Model::from_parts(mdl.clone(), vtx.clone(), vvd);

    let directories = model.texture_directories().to_vec();
    let skin = model.skin_tables().next();
    let all = model.vertices();
    let mut out = Vec::new();
    // Hardware vertex index: the order the strip groups upload vertices in,
    // strip group after strip group, mesh after mesh — the order vrad writes
    // `.vhv` colours in. Not the VVD index: a multi-LOD model's VVD ids run
    // past its LOD-0 count.
    let mut hw_base = 0u32;

    for ((mdl_mesh, model_vertex_offset), vtx_mesh) in mdl_meshes.into_iter().zip(vtx_meshes) {
        let texture_name = skin
            .as_ref()
            .and_then(|table| table.texture(mdl_mesh.material))
            .unwrap_or("");
        let texture = materials.resolve_model_texture(&directories, texture_name);
        let vvd_offset = (mdl_mesh.vertex_offset + model_vertex_offset).max(0) as usize;
        for group in &vtx_mesh.strip_groups {
            let ids: Vec<(u32, usize)> = group
                .strips
                .iter()
                .flat_map(|strip| strip.indices())
                .filter_map(|i| {
                    let sg = *group.indices.get(i)? as usize;
                    let vvd_id = group.vertices.get(sg)?.original_mesh_vertex_id as usize + vvd_offset;
                    Some((hw_base + sg as u32, vvd_id))
                })
                .collect();
            for triangle in ids.chunks_exact(3) {
                let mut positions = [Vec3::ZERO; 3];
                let mut normals = [Vec3::ZERO; 3];
                let mut uvs = [[0.0; 2]; 3];
                let mut vert_ids = [0u32; 3];
                for i in 0..3 {
                    let (hw, vvd_id) = triangle[i];
                    let Some(v) = all.get(vvd_id) else {
                        return None;
                    };
                    let position = model.apply_root_transform(v.position);
                    positions[i] = Vec3::new(position.x, position.y, position.z);
                    // The root transform is a rotation (plus nothing for a
                    // static prop); moving the normal through it and removing
                    // the origin's image keeps it a direction.
                    let n = model.apply_root_transform(v.normal);
                    let o = model.apply_root_transform(vmdl::Vector {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    });
                    normals[i] = Vec3::new(n.x - o.x, n.y - o.y, n.z - o.z);
                    uvs[i] = v.texture_coordinates;
                    vert_ids[i] = hw;
                }
                out.push(LocalTri {
                    positions,
                    normals,
                    uvs,
                    vert_ids,
                    texture,
                });
            }
            hw_base += group.vertices.len() as u32;
        }
    }
    let vertex_count = hw_base as usize;
    if std::env::var_os("SURF_OSS_PROP_DEBUG").is_some() {
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
    Some(DecodedModel {
        tris: out,
        vertex_count,
        illumination_position,
    })
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
