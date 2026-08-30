//! Entity KV parsing: spawns, teleports, render-only brush models.

use std::collections::HashMap;

use surf_core::math::{Angle, Vec3};
use surf_core::{Aabb, Brush, Plane};
use vbsp::Bsp;

use crate::collision::{brush_from_bsp, collect_model_brushes};
use crate::leaves::LeafBrushRange;
use crate::MapError;

#[derive(Clone, Copy, Debug)]
pub struct NamedPoint {
    pub origin: Vec3,
    pub angles: Angle,
}

#[derive(Clone, Copy, Debug)]
struct NamedEntity {
    point: NamedPoint,
    /// Prefer `info_teleport_destination` / `info_target` over trigger origins.
    is_teleport_dest: bool,
    /// `model=*N` — a brush volume, so `point.origin` is its *centre*, not a
    /// place to stand. lovetunnel's `startzone` centre is 144u up and flush
    /// against a wall corner: spawning there wedges the hull startsolid.
    is_brush: bool,
}

#[derive(Clone, Debug)]
pub struct TeleportTrigger {
    pub brushes: Vec<Brush>,
    pub bounds: Aabb,
    pub dest_origin: Vec3,
    pub dest_angles: Angle,
}

/// Continuous `trigger_push` — applies as player basevelocity while touching.
#[derive(Clone, Debug)]
pub struct PushTrigger {
    pub brushes: Vec<Brush>,
    pub bounds: Aabb,
    /// Precomputed `speed * pushdir_forward` (u/s).
    pub velocity: Vec3,
}

/// `trigger_gravity` — multiplies MoveVars gravity while touching.
#[derive(Clone, Debug)]
pub struct GravityTrigger {
    pub brushes: Vec<Brush>,
    pub bounds: Aabb,
    pub scale: f32,
}

/// One `AddOutput` a trigger applies to the player on a touch edge.
///
/// Maps build boosters, anti-gravity zones and one-shot gates out of these —
/// there is no dedicated entity for any of them. tendies' whole boost set is
/// `AddOutput basevelocity`; lovetunnel's launch pads are `AddOutput gravity`.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldAction {
    /// `basevelocity X Y Z` — a pending impulse nothing re-asserts, so the sim
    /// folds it straight into velocity (`apply_base_velocity_momentum`).
    BaseVelocity(Vec3),
    /// `gravity N` — multiplies sv_gravity until another output sets it back.
    Gravity(f32),
    /// `targetname NAME` — renames the player. The only reason maps do this is
    /// to gate a trigger behind a `filter_activator_name`, which is how a boost
    /// is made to fire once per run.
    TargetName(String),
}

/// `filter_activator_name` — passes on the player's current `targetname`.
#[derive(Clone, Debug)]
pub struct NameFilter {
    pub name: String,
    pub negated: bool,
}

impl NameFilter {
    pub fn passes(&self, activator: &str) -> bool {
        (self.name == activator) != self.negated
    }
}

/// A trigger that runs `AddOutput` actions on the player when they enter or
/// leave it. Unlike push/gravity volumes this needs touch *edges*, so the
/// runtime carries state for it (`crate::fields::FieldState`).
#[derive(Clone, Debug)]
pub struct FieldTrigger {
    pub brushes: Vec<Brush>,
    pub bounds: Aabb,
    pub on_start: Vec<FieldAction>,
    pub on_end: Vec<FieldAction>,
    /// Resolved `filtername`; `None` means the trigger touches everyone.
    pub filter: Option<NameFilter>,
}

pub struct ParsedEntities {
    /// Gameplay start (stage start / most-targeted teleport dest — not lobby T/CT).
    pub spawn: NamedPoint,
    pub teleports: Vec<TeleportTrigger>,
    pub pushes: Vec<PushTrigger>,
    pub gravities: Vec<GravityTrigger>,
    pub fields: Vec<FieldTrigger>,
    /// Brush models to render in addition to world (func_illusionary / never-solid func_brush).
    /// `(model_index, entity origin)` — bmodel verts are local to origin.
    pub render_models: Vec<(usize, Vec3)>,
}

pub fn parse_entities(
    bsp: &Bsp,
    leaf_ranges: &[LeafBrushRange],
    world_bounds: Aabb,
) -> Result<ParsedEntities, MapError> {
    let mut player_spawns = Vec::new();
    // (model, target, start_disabled, origin, filtername)
    let mut teleports_raw: Vec<(usize, String, bool, Vec3, String)> = Vec::new();
    // (model, origin, velocity) — continuous only (skip Once-only flag 128).
    let mut pushes_raw: Vec<(usize, Vec3, Vec3)> = Vec::new();
    // (model, origin, gravity scale)
    let mut gravities_raw: Vec<(usize, Vec3, f32)> = Vec::new();
    let mut fields_raw: Vec<RawField> = Vec::new();
    // targetname -> filter_activator_name
    let mut name_filters: HashMap<String, NameFilter> = HashMap::new();
    let mut named: HashMap<String, NamedEntity> = HashMap::new();
    let mut render_models = Vec::new();
    let mut teleport_target_counts: HashMap<String, usize> = HashMap::new();
    let mut start_volumes: Vec<Aabb> = Vec::new();

    for ent in bsp.entities.iter() {
        let class = ent.prop("classname").unwrap_or("");
        let origin = ent
            .prop("origin")
            .and_then(parse_vec3)
            .unwrap_or(Vec3::ZERO);
        let angles = parse_angles(&ent);

        if let Some(name) = ent.prop("targetname") {
            let is_teleport_dest = matches!(
                class,
                "info_teleport_destination" | "info_target" | "info_landmark"
            );
            let point = NamedPoint { origin, angles };
            let model_index = ent.prop("model").and_then(parse_model_index);
            // A trigger named like a start marks the start *area*. Keep its
            // world box: the point entity the map actually teleports you to
            // sits inside it, whatever that entity is called.
            if name_is_start_like(name) {
                if let Some(m) = model_index.and_then(|i| bsp.models.get(i)) {
                    start_volumes.push(Aabb::from_mins_maxs(
                        Vec3::new(m.mins.x, m.mins.y, m.mins.z) + origin,
                        Vec3::new(m.maxs.x, m.maxs.y, m.maxs.z) + origin,
                    ));
                }
            }
            // Prefer a real teleport destination when several ents share a name.
            match named.get(name) {
                Some(prev) if prev.is_teleport_dest && !is_teleport_dest => {}
                _ => {
                    named.insert(
                        name.to_string(),
                        NamedEntity {
                            point,
                            is_teleport_dest,
                            is_brush: model_index.is_some(),
                        },
                    );
                }
            }
        }

        if class == "filter_activator_name" {
            if let (Some(name), Some(filtername)) =
                (ent.prop("targetname"), prop_ci(&ent, "filtername"))
            {
                // Hammer writes the *label* for the false case ("Allow entities
                // that match criteria"), so only a literal 1 negates.
                let negated = prop_ci(&ent, "Negated").unwrap_or("0").trim() == "1";
                name_filters.insert(
                    name.to_string(),
                    NameFilter {
                        name: filtername.to_string(),
                        negated,
                    },
                );
            }
        }

        // Any brush trigger can carry AddOutput boosts, so harvest outputs
        // before the per-class arms below (a trigger_push with outputs is both).
        if class.starts_with("trigger_") && !trigger_start_disabled(&ent) && touches_clients(&ent) {
            let (on_start, on_end) = parse_trigger_outputs(&ent);
            if !on_start.is_empty() || !on_end.is_empty() {
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 {
                        fields_raw.push(RawField {
                            model,
                            origin,
                            on_start,
                            on_end,
                            filtername: prop_ci(&ent, "filtername").unwrap_or("").to_string(),
                        });
                    }
                }
            }
        }

        match class {
            "info_player_terrorist"
            | "info_player_counterterrorist"
            | "info_player_start"
            | "info_player_logo" => {
                player_spawns.push((class, NamedPoint { origin, angles }));
            }
            "trigger_teleport" => {
                let start_disabled =
                    ent.prop("StartDisabled").map(|v| v == "1").unwrap_or(false);
                let target = ent.prop("target").unwrap_or("").to_string();
                // filter_activator_name (e.g. filter_fail) needs trigger_multiple
                // AddOutput targetname — skip until that path exists. Unfiltered
                // fail floors cover the main summit line.
                let filtername = ent.prop("filtername").unwrap_or("").to_string();
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 && !target.is_empty() {
                        *teleport_target_counts.entry(target.clone()).or_insert(0) += 1;
                        teleports_raw.push((
                            model,
                            target,
                            start_disabled,
                            origin,
                            filtername,
                        ));
                    }
                }
            }
            "trigger_push" => {
                let start_disabled =
                    ent.prop("StartDisabled").map(|v| v == "1").unwrap_or(false);
                if start_disabled {
                    continue;
                }
                let spawnflags: u32 = ent
                    .prop("spawnflags")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                // SF_TRIG_PUSH_ONCE = 128 — deferred (corpus maps use continuous).
                if spawnflags & 128 != 0 {
                    continue;
                }
                let speed: f32 = ent
                    .prop("speed")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                if speed == 0.0 {
                    continue;
                }
                let pushdir = parse_pushdir(&ent);
                let (fwd, _, _) = pushdir.vectors();
                let velocity = fwd * speed;
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 {
                        pushes_raw.push((model, origin, velocity));
                    }
                }
            }
            "trigger_gravity" => {
                let start_disabled =
                    ent.prop("StartDisabled").map(|v| v == "1").unwrap_or(false);
                if start_disabled {
                    continue;
                }
                let scale: f32 = ent
                    .prop("gravity")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1.0);
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 {
                        gravities_raw.push((model, origin, scale));
                    }
                }
            }
            "func_illusionary" => {
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 {
                        render_models.push((model, origin));
                    }
                }
            }
            "func_brush" => {
                // Solidity 1 = Never Solid (render only).
                let solidity = ent.prop("Solidity").unwrap_or("1");
                if solidity == "1" {
                    if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                        if model > 0 {
                            render_models.push((model, origin));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let spawn = pick_gameplay_spawn_debug(
        &named,
        &teleport_target_counts,
        &player_spawns,
        &start_volumes,
    )
    .ok_or(MapError::NoSpawn)?;

    let mut teleports = Vec::new();
    for (model_idx, target, start_disabled, origin, filtername) in teleports_raw {
        if start_disabled {
            continue;
        }
        if !filtername.is_empty() {
            continue;
        }
        let Some(dest) = named.get(&target) else {
            continue;
        };
        let Some((brushes, bounds)) =
            harvest_trigger_brushes(bsp, leaf_ranges, world_bounds, model_idx, origin)
        else {
            continue;
        };
        teleports.push(TeleportTrigger {
            brushes,
            bounds,
            dest_origin: dest.point.origin,
            dest_angles: dest.point.angles,
        });
    }

    let mut pushes = Vec::new();
    for (model_idx, origin, velocity) in pushes_raw {
        let Some((brushes, bounds)) =
            harvest_trigger_brushes(bsp, leaf_ranges, world_bounds, model_idx, origin)
        else {
            continue;
        };
        pushes.push(PushTrigger {
            brushes,
            bounds,
            velocity,
        });
    }

    let mut gravities = Vec::new();
    for (model_idx, origin, scale) in gravities_raw {
        let Some((brushes, bounds)) =
            harvest_trigger_brushes(bsp, leaf_ranges, world_bounds, model_idx, origin)
        else {
            continue;
        };
        gravities.push(GravityTrigger {
            brushes,
            bounds,
            scale,
        });
    }

    let mut fields = Vec::new();
    for RawField {
        model,
        origin,
        on_start,
        on_end,
        filtername,
    } in fields_raw
    {
        // An unresolvable filter means we cannot tell who the trigger touches;
        // firing it unconditionally would hand out boosts the map gates.
        let filter = if filtername.is_empty() {
            None
        } else {
            match name_filters.get(&filtername) {
                Some(f) => Some(f.clone()),
                None => continue,
            }
        };
        let Some((brushes, bounds)) =
            harvest_trigger_brushes(bsp, leaf_ranges, world_bounds, model, origin)
        else {
            continue;
        };
        fields.push(FieldTrigger {
            brushes,
            bounds,
            on_start,
            on_end,
            filter,
        });
    }

    Ok(ParsedEntities {
        spawn,
        teleports,
        pushes,
        gravities,
        fields,
        render_models,
    })
}

/// Case-insensitive key lookup.
///
/// vbsp's `prop` compares keys literally, and these BSPs store them lowercased
/// (`startdisabled`, `negated`) while the FGD spells them `StartDisabled`,
/// `Negated`. Anything reading a mixed-case key must go through this.
fn prop_ci<'a>(ent: &vbsp::RawEntity<'a>, key: &str) -> Option<&'a str> {
    ent.properties()
        .find_map(|(k, v)| k.eq_ignore_ascii_case(key).then_some(v))
}

/// `StartDisabled` — the trigger is off until an input enables it, which we
/// don't model, so treat it as absent.
fn trigger_start_disabled(ent: &vbsp::RawEntity<'_>) -> bool {
    prop_ci(ent, "StartDisabled").map(|v| v.trim() == "1").unwrap_or(false)
}

/// Spawnflag 1 is "Clients". A trigger that doesn't list it never touches the
/// player (physics-object-only triggers are common set dressing).
fn touches_clients(ent: &vbsp::RawEntity<'_>) -> bool {
    match prop_ci(ent, "spawnflags").and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(0) | None => true,
        Some(f) => f & 1 != 0,
    }
}

/// Collect `OnStartTouch` / `OnEndTouch` outputs we can act on.
///
/// Keys repeat (an entity may have several `OnStartTouch` lines), so this walks
/// every property rather than using `prop`, which returns only the first.
fn parse_trigger_outputs(ent: &vbsp::RawEntity<'_>) -> (Vec<FieldAction>, Vec<FieldAction>) {
    let mut on_start = Vec::new();
    let mut on_end = Vec::new();
    for (key, value) in ent.properties() {
        let dst = if key.eq_ignore_ascii_case("OnStartTouch") {
            &mut on_start
        } else if key.eq_ignore_ascii_case("OnEndTouch") {
            &mut on_end
        } else {
            continue;
        };
        if let Some(action) = parse_addoutput(value) {
            dst.push(action);
        }
    }
    (on_start, on_end)
}

/// Parse one output value into an action we can apply to the player.
///
/// Format is `<target>,<input>,<parameter>,<delay>,<refire>`, separated by
/// either `,` or the 0x1B escape newer compilers emit. We take only
/// `!activator,AddOutput,<key> <value>` with no delay — everything else
/// (sounds, doors, `SetDamageFilter`, relays) has no bearing on movement.
fn parse_addoutput(value: &str) -> Option<FieldAction> {
    let parts: Vec<&str> = value.split(['\u{1b}', ',']).map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    if !parts[0].eq_ignore_ascii_case("!activator") || !parts[1].eq_ignore_ascii_case("AddOutput") {
        return None;
    }
    // A delayed output would need a scheduler; none of the corpus uses one.
    if let Some(delay) = parts.get(3).and_then(|d| d.parse::<f32>().ok()) {
        if delay > 0.0 {
            return None;
        }
    }
    let param = parts[2];
    let (key, rest) = param.split_once(char::is_whitespace)?;
    let rest = rest.trim();
    if key.eq_ignore_ascii_case("basevelocity") {
        return parse_vec3(rest).map(FieldAction::BaseVelocity);
    }
    if key.eq_ignore_ascii_case("gravity") {
        return rest.parse().ok().map(FieldAction::Gravity);
    }
    if key.eq_ignore_ascii_case("targetname") {
        return Some(FieldAction::TargetName(rest.to_string()));
    }
    None
}

/// A trigger with usable outputs, before its brushes are harvested.
struct RawField {
    model: usize,
    origin: Vec3,
    on_start: Vec<FieldAction>,
    on_end: Vec<FieldAction>,
    filtername: String,
}

/// Harvest brush planes for a trigger bmodel, shifted by entity `origin`.
fn harvest_trigger_brushes(
    bsp: &Bsp,
    leaf_ranges: &[LeafBrushRange],
    world_bounds: Aabb,
    model_idx: usize,
    origin: Vec3,
) -> Option<(Vec<Brush>, Aabb)> {
    let model = bsp.models.get(model_idx)?;
    let indices = collect_model_brushes(bsp, leaf_ranges, model.head_node);
    let mut brushes = Vec::new();
    let mut bounds: Option<Aabb> = None;
    for bi in indices {
        // Trigger brushes lie as SOLID in the lump — take all in the model.
        // Bmodel planes are entity-local; shift into world space.
        if let Some(brush) = brush_from_bsp(bsp, bi, world_bounds) {
            let brush = translate_brush(brush, origin);
            bounds = Some(match bounds {
                None => brush.bounds,
                Some(b) => merge_aabb(b, brush.bounds),
            });
            brushes.push(brush);
        }
    }
    if brushes.is_empty() {
        let b = Aabb::from_mins_maxs(
            Vec3::new(model.mins.x, model.mins.y, model.mins.z) + origin,
            Vec3::new(model.maxs.x, model.maxs.y, model.maxs.z) + origin,
        );
        brushes.push(Brush::aabb(b.mins, b.maxs));
        bounds = Some(b);
    }
    Some((brushes, bounds.unwrap_or(world_bounds)))
}

/// `OSX_SURF_SPAWN_DEBUG=1` names the entity the picker chose, and every start
/// volume it recognised — the fastest way to explain a wrong spawn.
fn pick_gameplay_spawn_debug(
    named: &HashMap<String, NamedEntity>,
    teleport_target_counts: &HashMap<String, usize>,
    player_spawns: &[(&str, NamedPoint)],
    start_volumes: &[Aabb],
) -> Option<NamedPoint> {
    let pick = pick_gameplay_spawn(named, teleport_target_counts, player_spawns, start_volumes);
    if std::env::var_os("OSX_SURF_SPAWN_DEBUG").is_some() {
        for v in start_volumes {
            println!("SPAWN startvolume {:?}..{:?}", v.mins, v.maxs);
        }
        match pick {
            Some(pt) => {
                let name = named
                    .iter()
                    .find(|(_, e)| {
                        !e.is_brush
                            && e.point.origin.x == pt.origin.x
                            && e.point.origin.y == pt.origin.y
                            && e.point.origin.z == pt.origin.z
                    })
                    .map(|(n, _)| n.as_str())
                    .unwrap_or("<player spawn ent>");
                println!("SPAWN pick '{name}' at {:?}", pt.origin);
            }
            None => println!("SPAWN pick <none>"),
        }
    }
    pick
}

/// Surf maps put T/CT in a cosmetic lobby; stage start is usually an
/// `info_teleport_destination` (e.g. `td_mapstart`) that fail-teleports target.
fn pick_gameplay_spawn(
    named: &HashMap<String, NamedEntity>,
    teleport_target_counts: &HashMap<String, usize>,
    player_spawns: &[(&str, NamedPoint)],
    start_volumes: &[Aabb],
) -> Option<NamedPoint> {
    // A brush entity's origin is the centre of a volume, not a standing spot —
    // never spawn on one. (lovetunnel: `startzone`, 144u of air and flush with
    // a wall corner, which leaves the hull startsolid and unable to move.)
    let candidates = || named.iter().filter(|(_, ent)| !ent.is_brush);
    let score = |name: &str, ent: &NamedEntity| {
        score_spawn_name(name, ent.is_teleport_dest) + start_zone_bonus(ent, start_volumes)
    };
    // 1) Well-known stage-start names (case-insensitive exact).
    const PREFERRED: &[&str] = &[
        "td_mapstart",
        "mapstart",
        "map_start",
        "map_dest",
        "main_start",
        "main",
        "stage1",
        "stage_1",
        "s1_start",
        "s1start",
        "td_start",
        "tele_start",
        "start_tele_dest",
        "mapstart_tele_dest",
        "start",
    ];
    for want in PREFERRED {
        let mut hits: Vec<&NamedEntity> = candidates()
            .filter(|(n, _)| n.eq_ignore_ascii_case(want) && !is_bonus_or_later_stage(n))
            .map(|(_, ent)| ent)
            .collect();
        hits.sort_by_key(|e| !e.is_teleport_dest);
        if let Some(ent) = hits.first() {
            return Some(ent.point);
        }
    }

    // 2) Best-scored name among non-bonus ents (stable: name order).
    let mut scored: Vec<(i32, &str, &NamedEntity)> = candidates()
        .filter(|(n, _)| !is_bonus_or_later_stage(n))
        .map(|(n, ent)| (score(n, ent), n.as_str(), ent))
        .filter(|(score, _, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    if let Some((_, _, ent)) = scored.first() {
        return Some(ent.point);
    }

    // 3) Destination most targeted by trigger_teleport, preferring start-like names
    // over fail/reset pads (lovetunnel's `reset` is a common trap).
    let mut targets: Vec<(i32, usize, &str)> = teleport_target_counts
        .iter()
        .filter(|(n, _)| !is_bonus_or_later_stage(n))
        .filter_map(|(n, &count)| {
            let ent = named.get(n)?;
            if ent.is_brush {
                return None;
            }
            Some((score(n, ent), count, n.as_str()))
        })
        .collect();
    targets.sort_by(|a, b| {
        // Primary: teleport fan-in; secondary: start-like name score.
        b.1.cmp(&a.1)
            .then_with(|| b.0.cmp(&a.0))
            .then_with(|| a.2.cmp(b.2))
    });
    for (score, count, name) in &targets {
        if *count >= 3 || (player_spawns.is_empty() && *count >= 1) {
            // Reject pure fail/reset pads when a better-named target exists with
            // similar fan-in (within 2).
            if *score < 0 {
                let alt = targets.iter().find(|(s, c, _)| *s >= 40 && *c + 2 >= *count);
                if let Some((_, _, alt_name)) = alt {
                    if let Some(ent) = named.get(*alt_name) {
                        return Some(ent.point);
                    }
                }
                continue;
            }
            if let Some(ent) = named.get(*name) {
                return Some(ent.point);
            }
        }
    }
    // Last teleport-dest resort: highest-scored targeted name regardless of count.
    if let Some((_, _, name)) = targets.iter().find(|(s, _, _)| *s >= 40) {
        if let Some(ent) = named.get(*name) {
            return Some(ent.point);
        }
    }

    // 4) Fall back to player spawn entities — one inside the start zone first
    // (surf maps put T/CT in a cosmetic lobby far from the course).
    if let Some((_, pt)) = player_spawns
        .iter()
        .find(|(_, pt)| inside_any(pt.origin, start_volumes))
    {
        return Some(*pt);
    }
    for class in [
        "info_player_start",
        "info_player_terrorist",
        "info_player_counterterrorist",
        "info_player_logo",
    ] {
        if let Some((_, pt)) = player_spawns.iter().find(|(c, _)| *c == class) {
            return Some(*pt);
        }
    }
    player_spawns.first().map(|(_, pt)| *pt)
}

/// Does this targetname mark the start area? Used to recognise a *volume*
/// (`startzone`, `start_zone`, `s1_start`…), not to score a spawn point.
fn name_is_start_like(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !is_bonus_or_later_stage(&lower) && lower.contains("start")
}

/// Whatever the map calls the entity it teleports you to, if it stands inside
/// the start trigger it *is* the map start. Big enough to outweigh the
/// fail/reset name penalty (lovetunnel's only destination is named `reset`).
fn start_zone_bonus(ent: &NamedEntity, start_volumes: &[Aabb]) -> i32 {
    if ent.is_teleport_dest && inside_any(ent.point.origin, start_volumes) {
        120
    } else {
        0
    }
}

fn inside_any(p: Vec3, volumes: &[Aabb]) -> bool {
    volumes.iter().any(|v| {
        p.x >= v.mins.x
            && p.x <= v.maxs.x
            && p.y >= v.mins.y
            && p.y <= v.maxs.y
            && p.z >= v.mins.z
            && p.z <= v.maxs.z
    })
}

fn is_bonus_or_later_stage(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("bonus") {
        return true;
    }
    // stage2+ / s2_start… — keep stage1 / s1_start.
    for n in 2..=9 {
        if lower.contains(&format!("stage{n}"))
            || lower.contains(&format!("stage_{n}"))
            || lower.contains(&format!("s{n}_start"))
            || lower.contains(&format!("s{n}start"))
        {
            return true;
        }
    }
    false
}

/// Score a candidate spawn name. Higher is better; ≤0 means "not a start".
///
/// Important: `bonus1_start`.contains(`s1_start`) is true — never use raw
/// `contains("s1_start")` without rejecting bonus names first.
fn score_spawn_name(name: &str, is_teleport_dest: bool) -> i32 {
    let lower = name.to_ascii_lowercase();
    if is_bonus_or_later_stage(&lower) {
        return -1000;
    }

    let mut score = 0i32;
    if lower == "td_mapstart" || lower == "mapstart" || lower == "map_start" {
        score += 100;
    } else if lower.contains("mapstart") || lower.contains("map_start") || lower == "map_dest" {
        score += 90;
    } else if lower == "main" || lower == "main_start" || lower.contains("main_start") {
        score += 85;
    } else if is_s1_start_token(&lower) {
        score += 80;
    } else if lower.contains("stage1") || lower.contains("stage_1") {
        score += 70;
    } else if lower.contains("start_tele")
        || lower.contains("tele_start")
        || lower.ends_with("_start")
        || lower == "start"
    {
        score += 55;
    } else if lower.contains("start") {
        score += 35;
    }

    if lower.contains("fail") || lower.contains("reset") || lower.contains("wipe") {
        score -= 60;
    }
    if is_teleport_dest {
        score += 10;
    }
    score
}

fn is_s1_start_token(lower: &str) -> bool {
    lower == "s1_start"
        || lower == "s1start"
        || lower.starts_with("s1_start_")
        || lower.starts_with("s1start_")
        || lower.ends_with("_s1_start")
        || lower.ends_with("_s1start")
        || lower.contains("_s1_start_")
}

fn merge_aabb(a: Aabb, b: Aabb) -> Aabb {
    Aabb::from_mins_maxs(
        Vec3::new(
            a.mins.x.min(b.mins.x),
            a.mins.y.min(b.mins.y),
            a.mins.z.min(b.mins.z),
        ),
        Vec3::new(
            a.maxs.x.max(b.maxs.x),
            a.maxs.y.max(b.maxs.y),
            a.maxs.z.max(b.maxs.z),
        ),
    )
}

/// Source brush models store planes relative to the entity `origin`.
fn translate_brush(brush: Brush, origin: Vec3) -> Brush {
    if origin == Vec3::ZERO {
        return brush;
    }
    let planes = brush
        .planes
        .into_iter()
        .map(|p| Plane {
            normal: p.normal,
            dist: p.dist + p.normal.dot(origin),
        })
        .collect();
    Brush {
        planes,
        bounds: Aabb::from_mins_maxs(brush.bounds.mins + origin, brush.bounds.maxs + origin),
    }
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let mut it = s.split_whitespace();
    let x: f32 = it.next()?.parse().ok()?;
    let y: f32 = it.next()?.parse().ok()?;
    let z: f32 = it.next()?.parse().ok()?;
    Some(Vec3::new(x, y, z))
}

fn parse_angles(ent: &vbsp::RawEntity<'_>) -> Angle {
    if let Some(a) = ent.prop("angles").and_then(|s| {
        let mut it = s.split_whitespace();
        let pitch: f32 = it.next()?.parse().ok()?;
        let yaw: f32 = it.next()?.parse().ok()?;
        let roll: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        Some(Angle::new(pitch, yaw, roll))
    }) {
        return a;
    }
    if let Some(yaw) = ent.prop("angle").and_then(|s| s.parse::<f32>().ok()) {
        if yaw >= 0.0 {
            return Angle::new(0.0, yaw, 0.0);
        }
    }
    Angle::new(0.0, 0.0, 0.0)
}

/// `trigger_push.pushdir` is pitch/yaw/roll degrees (same layout as `angles`).
fn parse_pushdir(ent: &vbsp::RawEntity<'_>) -> Angle {
    if let Some(a) = ent.prop("pushdir").and_then(|s| {
        let mut it = s.split_whitespace();
        let pitch: f32 = it.next()?.parse().ok()?;
        let yaw: f32 = it.next()?.parse().ok()?;
        let roll: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        Some(Angle::new(pitch, yaw, roll))
    }) {
        return a;
    }
    Angle::ZERO
}

fn parse_model_index(s: &str) -> Option<usize> {
    let s = s.strip_prefix('*')?;
    s.parse().ok()
}
