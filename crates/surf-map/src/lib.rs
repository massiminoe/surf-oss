//! Load CS:S / CS:GO BSP maps into surf-core collision + a textured mesh.
//!
//! Collision uses world-model (model 0) brushes matching MASK_PLAYERSOLID
//! (including PLAYERCLIP). Trigger bmodels are never world-solid.
//! Materials: pakfile VMT/VTF, optional local CS:S stock (`SURF_OSS_GAME_DIR`).

mod ambient;
mod collision;
pub mod disp;
mod entities;
pub mod fields;
mod leaves;
mod lightmap;
mod materials;
mod mesh;
mod models;
mod pak;
mod phy;
pub mod stock;
mod vmt;
mod world_lights;

use std::fmt;
use std::path::Path;

use surf_core::graybox::GrayboxMesh;
use surf_core::math::{Angle, Vec3};
use surf_core::movement::PlayerState;
use surf_core::{Aabb, Brush, World};

pub use entities::{
    FieldAction, FieldTrigger, GravityTrigger, NameFilter, NamedPoint, PushTrigger,
    TeleportTrigger,
};
pub use fields::{FieldEffects, FieldState};
pub use lightmap::{LightmapAtlas, UNIT_LIGHT_BYTE};
pub use materials::{MaterialAtlas, SkyboxAtlas};
pub use models::{PropInstance, PropLighting};

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
    /// Triggers whose `AddOutput` outputs act on the player (boosters, gates).
    pub fields: Vec<FieldTrigger>,
    /// Index into `world.tris` where static-prop collision begins; everything
    /// before it is displacement. Lets tools attribute a snag to the right
    /// source — prop tris come from a render mesh, displacement tris do not.
    pub prop_tri_start: usize,
    /// For each displacement collision tri (`world.tris[..prop_tri_start]`),
    /// the index of the displacement it came from, so a hit can be attributed.
    pub disp_tri_owner: Vec<u32>,
    /// Per displacement (BSP order): Hammer's collision flags, see `disp::DISP_*`.
    pub disp_flags: Vec<u32>,
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
        let mut brushes = collision::build_player_brushes(&bsp, &world_brush_indices, world_bounds);
        let ents = entities::parse_entities(&bsp, &leaf_ranges, world_bounds)?;
        // Solid brush entities (func_brush) are part of the walkable world.
        brushes.extend(ents.solid_brushes.iter().cloned());

        let pak = pak::PakFs::from_bsp(&bsp);
        let stock = stock::StockFs::from_env();
        let skyname = worldspawn_skyname(&bsp);
        let skybox = load_skybox(&pak, &stock, skyname.as_deref());

        let mut materials = materials::MaterialBank::new(pak, stock);
        let mut lightmaps = lightmap::LightmapBaker::from_bsp_bytes(data);
        let disps = disp::extract_displacements(&bsp, &mut materials, &mut lightmaps);
        let mut coll_tris = disps.collision;
        let disp_tri_owner = disps.owner;
        let disp_flags = disps.flags;
        let prop_tri_start = coll_tris.len();
        // The map's own area partition tells us which props are 3D-skybox
        // backdrop; those are neither drawn nor collided.
        let leaf_areas = leaves::parse_leaf_areas(data).unwrap_or_default();
        let skybox_props = models::skybox_prop_mask(&bsp, &leaf_areas, skybox_area(&bsp, data));
        let (prop_tris, mut props) =
            models::extract_prop_collision(&bsp, &mut materials, &skybox_props);
        coll_tris.extend(prop_tris);
        let world = World::with_tris(brushes, coll_tris, disp::TRI_GRID_CELL);

        let mut mesh = mesh::build_mesh(&bsp, &ents.render_models, &mut materials, &mut lightmaps);
        mesh.tris.extend(disps.render);
        let (_, render_bounds) =
            models::append_static_props(&bsp, data, &mut materials, &mut mesh, &skybox_props);
        for prop in &mut props {
            if let Some((range, bounds, lighting)) = render_bounds.get(prop.index) {
                prop.render_tris = range.clone();
                prop.render_bounds = *bounds;
                prop.lighting = *lighting;
            }
        }
        let materials = materials.into_atlas();
        let lightmaps = lightmaps.finish();
        // lm_uv was pixel-space while the atlas height grew; normalize now.
        lightmap::normalize_lm_uvs(&mut mesh, lightmaps.width, lightmaps.height);

        // A spawn whose hull starts solid cannot move at all — every trace
        // returns fraction 0, so the player floats in place. Source refuses to
        // leave an entity wedged (`EntityPlacementTest`); so do we.
        let spawn_origin = unstick(&world, ents.spawn.origin);

        Ok(Self {
            name,
            world,
            mesh,
            materials,
            lightmaps,
            skybox,
            spawn_origin,
            spawn_angles: ents.spawn.angles,
            teleports: ents.teleports,
            pushes: ents.pushes,
            gravities: ents.gravities,
            fields: ents.fields,
            prop_tri_start,
            disp_tri_owner,
            disp_flags,
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

    /// Sum of continuous push velocities while the player touches a push volume.
    ///
    /// Hull overlap, like `touch_teleport` — Source tests trigger touch against
    /// the player's bounding box, and a point probe leaves a volume 16u early.
    /// On lovetunnel that is the difference between our 3500 u/s tunnel paying
    /// out on the tick the WR does and two ticks before it.
    pub fn touch_push(&self, origin: Vec3) -> Vec3 {
        let hull = surf_core::movement::Hull::css_stand();
        let mins = origin + hull.mins;
        let maxs = origin + hull.maxs;
        let mut sum = Vec3::ZERO;
        for push in &self.pushes {
            let expanded = push.bounds.expand(1.0);
            if !aabb_overlap(mins, maxs, expanded.mins, expanded.maxs) {
                continue;
            }
            if push
                .brushes
                .iter()
                .any(|b| brush_intersects_aabb(b, mins, maxs, fields::TOUCH_PAD))
            {
                sum = sum + push.velocity;
            }
        }
        sum
    }

    /// Run the map's touch-driven effects for the tick that just finished.
    ///
    /// Call this **after** `tick`, on the post-move origin. That ordering is not
    /// cosmetic: Source processes trigger touches at the end of a move, so a
    /// booster's payout lands in the same tick you leave the volume, and the
    /// basevelocity a volume asserts is carried by the *next* move. Evaluating
    /// before the move instead puts every boost one tick late — measurably, on
    /// tendies' start booster, against the recorded WR.
    ///
    /// It is the only place the three sources of basevelocity/gravity are
    /// combined, so the app, the resim harness and the audit tools cannot drift
    /// apart: continuous `trigger_push`, `trigger_gravity`, and the `AddOutput`
    /// boosters and launch pads driven by `FieldState`.
    ///
    /// `SURF_OSS_NO_FIELDS=1` disables the `AddOutput` half for A/B runs.
    pub fn apply_fields(&self, player: &mut PlayerState, state: &mut FieldState, dt: f32) {
        let eff = self.field_effects(player.origin, state);
        surf_core::apply_base_velocity_momentum(&mut player.velocity, eff.released, dt);
        player.basevelocity = eff.basevelocity;
        player.gravity_scale = eff.gravity_scale;
    }

    /// Same, for a tool stepping one tick from a *recorded* pose: advance the
    /// touch state and arm the carry, but drop the payout — the recording
    /// already has the boost in its velocity, so folding it again doubles it.
    pub fn arm_fields_from_recording(&self, player: &mut PlayerState, state: &mut FieldState) {
        let eff = self.field_effects(player.origin, state);
        player.basevelocity = eff.basevelocity;
        player.gravity_scale = eff.gravity_scale;
    }

    fn field_effects(&self, origin: Vec3, state: &mut FieldState) -> FieldEffects {
        let push = self.touch_push(origin);
        let fields: &[FieldTrigger] = if no_fields() { &[] } else { &self.fields };
        let mut eff = state.update(fields, origin, push);
        eff.gravity_scale *= self.touch_gravity(origin);
        eff
    }

    /// Replay touch edges along a recorded path without simulating anything.
    ///
    /// A resim that starts mid-run has to inherit the gates and renames the real
    /// run already tripped; starting from a blank `FieldState` would re-arm a
    /// one-shot booster and fire it a second time.
    pub fn field_state_at(&self, path: &[Vec3]) -> FieldState {
        let mut state = FieldState::new();
        let fields: &[FieldTrigger] = if no_fields() { &[] } else { &self.fields };
        for origin in path {
            state.update(fields, *origin, self.touch_push(*origin));
        }
        state
    }

    /// Gravity scale from the first touching `trigger_gravity`, else 1.0.
    pub fn touch_gravity(&self, origin: Vec3) -> f32 {
        let hull = surf_core::movement::Hull::css_stand();
        let mins = origin + hull.mins;
        let maxs = origin + hull.maxs;
        for g in &self.gravities {
            let expanded = g.bounds.expand(1.0);
            if !aabb_overlap(mins, maxs, expanded.mins, expanded.maxs) {
                continue;
            }
            if g.brushes
                .iter()
                .any(|b| brush_intersects_aabb(b, mins, maxs, fields::TOUCH_PAD))
            {
                return g.scale;
            }
        }
        1.0
    }
}

/// Nudge a spawn out of solid geometry, or return it unchanged when it is free.
///
/// The failure this guards against is total, not cosmetic: `trace_box` reports
/// `startsolid` for a hull that merely *touches* a face (matching Source's
/// `d1 > 0` test), every move then clips to fraction 0, and the player hangs in
/// the air unable to walk, fall or jump. lovetunnel hit this when the spawn
/// picker used a trigger volume's centre, which sat flush against a wall corner.
///
/// Search order is up first — a spawn is nearly always slightly *into* the floor
/// — then sideways, then down, at growing distance. Deterministic, and it gives
/// up rather than teleporting the player somewhere unrelated.
fn unstick(world: &World, origin: Vec3) -> Vec3 {
    let hull = surf_core::movement::Hull::css_stand();
    if !surf_core::trace::point_contents_box(world, origin, hull.mins, hull.maxs) {
        return origin;
    }
    // All 26 axis combinations, up-most first: a wedge is usually a corner, so
    // a single-axis nudge can free the floor while staying inside the wall.
    let mut dirs: Vec<Vec3> = Vec::with_capacity(26);
    for dz in [1, 0, -1] {
        for dx in [-1, 0, 1] {
            for dy in [-1, 0, 1] {
                if (dx, dy, dz) != (0, 0, 0) {
                    dirs.push(Vec3::new(dx as f32, dy as f32, dz as f32));
                }
            }
        }
    }
    // Fewest axes first within each Z tier, so the smallest useful move wins.
    dirs.sort_by_key(|d| d.x.abs() as i32 + d.y.abs() as i32);
    dirs.sort_by_key(|d| -(d.z as i32));
    for step in [1.0, 2.0, 4.0, 8.0, 16.0, 24.0, 32.0, 48.0, 64.0] {
        for dir in &dirs {
            let candidate = origin + *dir * step;
            if !surf_core::trace::point_contents_box(world, candidate, hull.mins, hull.maxs) {
                return candidate;
            }
        }
    }
    origin
}

/// `SURF_OSS_NO_FIELDS=1` — run without the map's `AddOutput` boosters, for
/// A/B against recordings made before they existed.
fn no_fields() -> bool {
    std::env::var_os("SURF_OSS_NO_FIELDS").is_some()
}

/// True when the AABB is not entirely outside any brush plane (intersects or inside).
/// Brush planes are outward-facing; inside ⇒ signed distance ≤ 0 on every plane.
pub(crate) fn brush_intersects_aabb(brush: &Brush, mins: Vec3, maxs: Vec3, pad: f32) -> bool {
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

pub(crate) fn aabb_overlap(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
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
    use super::{unstick, Brush, LoadedMap, Vec3, World};
    use std::path::PathBuf;

    fn maps_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps")
    }

    #[test]
    fn frost_and_overgrowth_parse_pushes() {
        let dir = maps_dir();
        let frost = LoadedMap::load_path(dir.join("surf_frost.bsp")).expect("frost");
        assert_eq!(frost.pushes.len(), 8, "frost pushes");
        assert!(frost.gravities.is_empty());

        let og = LoadedMap::load_path(dir.join("surf_overgrowth.bsp")).expect("overgrowth");
        assert_eq!(og.pushes.len(), 11, "overgrowth pushes");
        assert_eq!(og.gravities.len(), 2, "overgrowth gravities");

        let summit = LoadedMap::load_path(dir.join("surf_summit.bsp")).expect("summit");
        assert!(summit.pushes.is_empty());
        assert!(summit.gravities.is_empty());
    }

    /// A hull flush against a wall face is `startsolid` (Source's `d1 > 0` test
    /// treats touching as inside), which pins every trace at fraction 0.
    #[test]
    fn unstick_frees_a_hull_flush_against_a_wall() {
        // Floor slab plus a wall whose face is exactly at x = 0.
        let world = World::new(vec![
            Brush::aabb(Vec3::new(-512.0, -512.0, -16.0), Vec3::new(512.0, 512.0, 0.0)),
            Brush::aabb(Vec3::new(-512.0, -512.0, 0.0), Vec3::new(0.0, 512.0, 128.0)),
        ]);
        let hull = surf_core::movement::Hull::css_stand();
        // Origin at x = 16 puts the hull's -x face exactly on the wall.
        let flush = Vec3::new(16.0, 0.0, 0.0);
        assert!(
            surf_core::trace::point_contents_box(&world, flush, hull.mins, hull.maxs),
            "flush against the wall should read as startsolid"
        );

        let freed = unstick(&world, flush);
        assert_ne!(freed.x, flush.x, "expected a sideways nudge off the wall");
        assert!(
            !surf_core::trace::point_contents_box(&world, freed, hull.mins, hull.maxs),
            "unstick returned a still-solid spot {freed:?}"
        );
        assert!((freed - flush).length() <= 64.0, "nudged too far: {freed:?}");
    }

    /// A free spawn must be returned untouched — this runs on every map load.
    #[test]
    fn unstick_leaves_a_free_spawn_alone() {
        let world = World::new(vec![Brush::aabb(
            Vec3::new(-512.0, -512.0, -16.0),
            Vec3::new(512.0, 512.0, 0.0),
        )]);
        let p = Vec3::new(0.0, 0.0, 64.0);
        let out = unstick(&world, p);
        assert_eq!((out.x, out.y, out.z), (p.x, p.y, p.z));
    }
}
