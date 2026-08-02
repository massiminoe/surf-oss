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
mod pak;
mod stock;
mod vmt;

use std::fmt;
use std::path::Path;

use surf_core::graybox::GrayboxMesh;
use surf_core::math::{Angle, Vec3};
use surf_core::{Aabb, Brush, World};

pub use entities::{NamedPoint, TeleportTrigger};
pub use lightmap::LightmapAtlas;
pub use materials::{MaterialAtlas, SkyboxAtlas};

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
        let skyname = worldspawn_skyname(&bsp);
        let skybox = skyname
            .as_deref()
            .map(|s| materials::SkyboxAtlas::from_pak(&pak, s))
            .unwrap_or_else(materials::SkyboxAtlas::none);

        let stock = stock::StockFs::from_env();
        let mut materials = materials::MaterialBank::new(pak, stock);
        let mut lightmaps = lightmap::LightmapBaker::from_bsp_bytes(data);
        let disps = disp::extract_displacements(&bsp, &mut materials, &mut lightmaps);
        let world = World::with_tris(brushes, disps.collision, disp::TRI_GRID_CELL);

        let ents = entities::parse_entities(&bsp, &leaf_ranges, world_bounds)?;
        let mut mesh = mesh::build_mesh(&bsp, &ents.render_models, &mut materials, &mut lightmaps);
        mesh.tris.extend(disps.render);
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
            kill_z: world_bounds.mins.z - 256.0,
            world_bounds,
            skyname,
        })
    }

    /// If the player is inside a teleport trigger, return destination origin+angles.
    pub fn touch_teleport(&self, origin: Vec3) -> Option<(Vec3, Angle)> {
        // Probe near hull center so floor-aligned triggers still fire.
        let probe = origin + Vec3::new(0.0, 0.0, 36.0);
        for tp in &self.teleports {
            if !tp.bounds.expand(64.0).contains_point(probe) {
                continue;
            }
            for brush in &tp.brushes {
                if brush_contains_point_padded(brush, probe, 4.0) {
                    return Some((tp.dest_origin, tp.dest_angles));
                }
            }
        }
        None
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

fn worldspawn_skyname(bsp: &vbsp::Bsp) -> Option<String> {
    for ent in bsp.entities.iter() {
        if ent.prop("classname") == Some("worldspawn") {
            return ent.prop("skyname").map(|s| s.to_string());
        }
    }
    None
}
