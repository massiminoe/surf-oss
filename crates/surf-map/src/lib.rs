//! Load CS:S / CS:GO BSP maps into surf-core collision + a textured mesh.
//!
//! Collision uses world-model (model 0) brushes matching MASK_PLAYERSOLID
//! (including PLAYERCLIP). Trigger bmodels are never world-solid.
//! Materials: pakfile VMT/VTF, optional local CS:S stock (`OSX_SURF_GAME_DIR`).

mod collision;
mod disp;
mod entities;
mod leaves;
mod lightmap;
mod materials;
mod mesh;
mod models;
mod pak;
mod phy;
mod stock;
mod vmt;

use std::fmt;
use std::path::Path;

use surf_core::graybox::GrayboxMesh;
use surf_core::math::{Angle, Vec3};
use surf_core::{Aabb, Brush, World};

pub use entities::{GravityTrigger, NamedPoint, PushTrigger, TeleportTrigger};
pub use lightmap::LightmapAtlas;
pub use materials::{MaterialAtlas, SkyboxAtlas};
pub use models::PropInstance;

#[derive(Debug)]
pub enum MapError {
    Io(std::io::Error),
    Bsp(String),
    NoSpawn,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Io(e) => write!(f, "io: {e}"),
            MapError::Bsp(e) => write!(f, "bsp: {e}"),
            MapError::NoSpawn => write!(f, "map has no player spawn"),
        }
    }
}

impl std::error::Error for MapError {}

impl From<std::io::Error> for MapError {
    fn from(e: std::io::Error) -> Self {
        MapError::Io(e)
    }
}

/// Playable map extracted from a BSP.
#[derive(Clone, Debug)]
pub struct LoadedMap {
    pub name: String,
    pub world: World,
    pub mesh: GrayboxMesh,
    /// Pakfile albedos packed as a texture array (layer 0 = white / missing).
    pub materials: MaterialAtlas,
    pub lightmaps: LightmapAtlas,
    pub skybox: SkyboxAtlas,
    pub spawn_origin: Vec3,
    pub spawn_angles: Angle,
    pub teleports: Vec<TeleportTrigger>,
    pub pushes: Vec<PushTrigger>,
    pub gravities: Vec<GravityTrigger>,
    /// Index into `world.tris` where static-prop collision begins; everything
    /// before it is displacement. Lets tools attribute a snag to the right
    /// source — prop tris come from a render mesh, displacement tris do not.
    pub prop_tri_start: usize,
    /// Every static prop placed in the map, with the slice of prop collision it
    /// owns. Lets tools answer "what did I just hit?" with a model name.
    pub props: Vec<models::PropInstance>,
    /// Soft reset when the player falls below this Z (disp holes, voids).
    pub kill_z: f32,
    pub world_bounds: Aabb,
    /// `worldspawn.skyname` if present (2D skybox materials under `skybox/`).
    pub skyname: Option<String>,
}

impl LoadedMap {
    pub fn load_path(path: impl AsRef<Path>) -> Result<Self, MapError> {
        let path = path.as_ref();
        let data = std::fs::read(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("map")
            .to_string();
        Self::load_bytes(&data, name)
    }

    pub fn load_bytes(data: &[u8], name: String) -> Result<Self, MapError> {
        let bsp = vbsp::Bsp::read(data).map_err(|e| MapError::Bsp(e.to_string()))?;
        // vbsp sorts leaves by cluster, which breaks leaf indices used by the BSP
        // tree. Re-read lump 10 ourselves for brush harvesting.
        let leaf_ranges =
            leaves::parse_leaf_brush_ranges(data).map_err(|e| MapError::Bsp(e.to_string()))?;

        let world_model = bsp
            .models
            .first()
            .ok_or_else(|| MapError::Bsp("no models".into()))?;
        let world_bounds = Aabb::from_mins_maxs(
            Vec3::new(world_model.mins.x, world_model.mins.y, world_model.mins.z),
            Vec3::new(world_model.maxs.x, world_model.maxs.y, world_model.maxs.z),
        );

        let world_brush_indices =
            collision::collect_model_brushes(&bsp, &leaf_ranges, world_model.head_node);
        let brushes = collision::build_player_brushes(&bsp, &world_brush_indices, world_bounds);

        let pak = pak::PakFs::from_bsp(&bsp);
        let stock = stock::StockFs::from_env();
        let skyname = worldspawn_skyname(&bsp);
        let skybox = load_skybox(&pak, &stock, skyname.as_deref());

        let mut materials = materials::MaterialBank::new(pak, stock);
        let mut lightmaps = lightmap::LightmapBaker::from_bsp_bytes(data);
        let disps = disp::extract_displacements(&bsp, &mut materials, &mut lightmaps);
        let mut coll_tris = disps.collision;
        let prop_tri_start = coll_tris.len();
        // The map's own area partition tells us which props are 3D-skybox
        // backdrop; those are neither drawn nor collided.
        let leaf_areas = leaves::parse_leaf_areas(data).unwrap_or_default();
        let skybox_props = models::skybox_prop_mask(&bsp, &leaf_areas, skybox_area(&bsp, data));
        let (prop_tris, mut props) =
            models::extract_prop_collision(&bsp, &mut materials, &skybox_props);
        coll_tris.extend(prop_tris);
        let world = World::with_tris(brushes, coll_tris, disp::TRI_GRID_CELL);

        let ents = entities::parse_entities(&bsp, &leaf_ranges, world_bounds)?;
        let mut mesh = mesh::build_mesh(&bsp, &ents.render_models, &mut materials, &mut lightmaps);
        mesh.tris.extend(disps.render);
        let (_, render_bounds) =
            models::append_static_props(&bsp, &mut materials, &mut mesh, &skybox_props);
        for prop in &mut props {
            if let Some((range, bounds)) = render_bounds.get(prop.index) {
                prop.render_tris = range.clone();
                prop.render_bounds = *bounds;
            }
        }
        let materials = materials.into_atlas();
        let lightmaps = lightmaps.finish();
        // lm_uv was pixel-space while the atlas height grew; normalize now.
        lightmap::normalize_lm_uvs(&mut mesh, lightmaps.width, lightmaps.height);

        Ok(Self {
            name,
            world,
            mesh,
            materials,
            lightmaps,
            skybox,
            spawn_origin: ents.spawn.origin,
            spawn_angles: ents.spawn.angles,
            teleports: ents.teleports,
            pushes: ents.pushes,
            gravities: ents.gravities,
            prop_tri_start,
            props,
            kill_z: world_bounds.mins.z - 256.0,
            world_bounds,
            skyname,
        })
    }

    /// If the player hull overlaps a teleport trigger, return destination origin+angles.
    ///
    /// Uses the standing CS:S hull AABB (not a point probe). Thin horizontal
    /// stage/fail slabs (often ~2u thick) are easy to tunnel past with a
    /// center-point sample; hull overlap matches Source trigger touch.
    pub fn touch_teleport(&self, origin: Vec3) -> Option<(Vec3, Angle)> {
        let hull = surf_core::movement::Hull::css_stand();
        let mins = origin + hull.mins;
        let maxs = origin + hull.maxs;
        for tp in &self.teleports {
            let expanded = tp.bounds.expand(8.0);
            if !aabb_overlap(mins, maxs, expanded.mins, expanded.maxs) {
                continue;
            }
            for brush in &tp.brushes {
                if brush_intersects_aabb(brush, mins, maxs, 1.0) {
                    return Some((tp.dest_origin, tp.dest_angles));
                }
            }
        }
        None
    }

    /// Sum of continuous push velocities while the player is inside any push trigger.
    pub fn touch_push(&self, origin: Vec3) -> Vec3 {
        let probe = origin + Vec3::new(0.0, 0.0, 36.0);
        let mut sum = Vec3::ZERO;
        for push in &self.pushes {
            if !push.bounds.expand(64.0).contains_point(probe) {
                continue;
            }
            for brush in &push.brushes {
                if brush_contains_point_padded(brush, probe, 4.0) {
                    sum = sum + push.velocity;
                    break;
                }
            }
        }
        sum
    }

    /// Gravity scale from the first touching `trigger_gravity`, else 1.0.
    pub fn touch_gravity(&self, origin: Vec3) -> f32 {
        let probe = origin + Vec3::new(0.0, 0.0, 36.0);
        for g in &self.gravities {
            if !g.bounds.expand(64.0).contains_point(probe) {
                continue;
            }
            for brush in &g.brushes {
                if brush_contains_point_padded(brush, probe, 4.0) {
                    return g.scale;
                }
            }
        }
        1.0
    }
}

fn brush_contains_point_padded(brush: &Brush, p: Vec3, pad: f32) -> bool {
    for plane in &brush.planes {
        if plane.distance(p) > pad {
            return false;
        }
    }
    true
}

/// True when the AABB is not entirely outside any brush plane (intersects or inside).
/// Brush planes are outward-facing; inside ⇒ signed distance ≤ 0 on every plane.
fn brush_intersects_aabb(brush: &Brush, mins: Vec3, maxs: Vec3, pad: f32) -> bool {
    for plane in &brush.planes {
        let n = plane.normal;
        // AABB corner with the smallest signed distance (most "inside" along -n).
        let x = if n.x >= 0.0 { mins.x } else { maxs.x };
        let y = if n.y >= 0.0 { mins.y } else { maxs.y };
        let z = if n.z >= 0.0 { mins.z } else { maxs.z };
        if plane.distance(Vec3::new(x, y, z)) > pad {
            return false;
        }
    }
    true
}

fn aabb_overlap(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
    a_mins.x <= b_maxs.x
        && a_maxs.x >= b_mins.x
        && a_mins.y <= b_maxs.y
        && a_maxs.y >= b_mins.y
        && a_mins.z <= b_maxs.z
        && a_maxs.z >= b_mins.z
}

/// The BSP area holding the 3D skybox, found by asking which leaf the map's own
/// `sky_camera` sits in.
///
/// Source draws that area scaled-down around the sky camera as a backdrop. We
/// have no 3D-skybox pass, so drawing it as ordinary world geometry puts a
/// full-size forest tens of thousands of units below the map: on boreas that is
/// 640 of 1587 props and **51% of the entire draw mesh**, none of it ever
/// meant to be seen at that scale.
///
/// Returns `None` when the map has no sky camera or the lumps will not parse,
/// in which case nothing is excluded — an unreadable BSP must not silently
/// delete scenery.
fn skybox_area(bsp: &vbsp::Bsp, bsp_bytes: &[u8]) -> Option<u16> {
    let origin = bsp.entities.iter().find_map(|ent| {
        (ent.prop("classname") == Some("sky_camera")).then(|| ent.prop("origin"))?
    })?;
    let mut parts = origin
        .split_whitespace()
        .filter_map(|v| v.parse::<f32>().ok());
    let point = [parts.next()?, parts.next()?, parts.next()?];
    let leaf = leaves::leaf_index_at(bsp_bytes, point).ok()?;
    let areas = leaves::parse_leaf_areas(bsp_bytes).ok()?;
    areas.get(leaf).copied()
}

fn worldspawn_skyname(bsp: &vbsp::Bsp) -> Option<String> {
    for ent in bsp.entities.iter() {
        if ent.prop("classname") == Some("worldspawn") {
            return ent.prop("skyname").map(|s| s.to_string());
        }
    }
    None
}

/// Pak + stock for the map's skyname; if missing (joke/empty names, CS:GO-only
/// skies), fall back to a stock CS:S day sky so the void isn't clear-color.
fn load_skybox(pak: &pak::PakFs, stock: &stock::StockFs, skyname: Option<&str>) -> SkyboxAtlas {
    const STOCK_FALLBACK: &str = "sky_day01_01";
    if let Some(name) = skyname {
        let atlas = materials::SkyboxAtlas::from_pak_and_stock(pak, stock, name);
        if !atlas.is_empty() {
            return atlas;
        }
        if name != STOCK_FALLBACK {
            let fb = materials::SkyboxAtlas::from_pak_and_stock(pak, stock, STOCK_FALLBACK);
            if !fb.is_empty() {
                return fb;
            }
        }
        return atlas;
    }
    materials::SkyboxAtlas::from_pak_and_stock(pak, stock, STOCK_FALLBACK)
}

#[cfg(test)]
mod push_parse_tests {
    use super::LoadedMap;
    use std::path::PathBuf;

    fn maps_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps")
    }

    #[test]
    fn frost_nyx_overgrowth_parse_pushes() {
        let dir = maps_dir();
        let frost = LoadedMap::load_path(dir.join("surf_frost.bsp")).expect("frost");
        assert_eq!(frost.pushes.len(), 8, "frost pushes");
        assert!(frost.gravities.is_empty());

        let nyx = LoadedMap::load_path(dir.join("surf_nyx.bsp")).expect("nyx");
        assert_eq!(nyx.pushes.len(), 1, "nyx pushes");

        let og = LoadedMap::load_path(dir.join("surf_overgrowth.bsp")).expect("overgrowth");
        assert_eq!(og.pushes.len(), 11, "overgrowth pushes");
        assert_eq!(og.gravities.len(), 2, "overgrowth gravities");

        let summit = LoadedMap::load_path(dir.join("surf_summit.bsp")).expect("summit");
        assert!(summit.pushes.is_empty());
        assert!(summit.gravities.is_empty());
    }
}
