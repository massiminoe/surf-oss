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

#[derive(Clone, Debug)]
pub struct TeleportTrigger {
    pub brushes: Vec<Brush>,
    pub bounds: Aabb,
    pub dest_origin: Vec3,
    pub dest_angles: Angle,
}

pub struct ParsedEntities {
    /// Gameplay start (stage start / most-targeted teleport dest — not lobby T/CT).
    pub spawn: NamedPoint,
    pub teleports: Vec<TeleportTrigger>,
    /// Model indices to render in addition to world (func_illusionary / solid func_brush).
    pub render_models: Vec<usize>,
}

pub fn parse_entities(
    bsp: &Bsp,
    leaf_ranges: &[LeafBrushRange],
    world_bounds: Aabb,
) -> Result<ParsedEntities, MapError> {
    let mut player_spawns = Vec::new();
    // (model, target, start_disabled, origin, filtername)
    let mut teleports_raw: Vec<(usize, String, bool, Vec3, String)> = Vec::new();
    let mut named: HashMap<String, NamedPoint> = HashMap::new();
    let mut render_models = Vec::new();
    let mut teleport_target_counts: HashMap<String, usize> = HashMap::new();

    for ent in bsp.entities.iter() {
        let class = ent.prop("classname").unwrap_or("");
        let origin = ent
            .prop("origin")
            .and_then(parse_vec3)
            .unwrap_or(Vec3::ZERO);
        let angles = parse_angles(&ent);

        if let Some(name) = ent.prop("targetname") {
            named.insert(name.to_string(), NamedPoint { origin, angles });
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
            "func_illusionary" => {
                if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                    if model > 0 {
                        render_models.push(model);
                    }
                }
            }
            "func_brush" => {
                // Solidity 1 = Never Solid (render only).
                let solidity = ent.prop("Solidity").unwrap_or("1");
                if solidity == "1" {
                    if let Some(model) = ent.prop("model").and_then(parse_model_index) {
                        if model > 0 {
                            render_models.push(model);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let spawn = pick_gameplay_spawn(&named, &teleport_target_counts, &player_spawns)
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
        let Some(model) = bsp.models.get(model_idx) else {
            continue;
        };
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
        teleports.push(TeleportTrigger {
            brushes,
            bounds: bounds.unwrap_or(world_bounds),
            dest_origin: dest.origin,
            dest_angles: dest.angles,
        });
    }

    Ok(ParsedEntities {
        spawn,
        teleports,
        render_models,
    })
}

/// Surf maps put T/CT in a cosmetic lobby; stage start is usually an
/// `info_teleport_destination` (e.g. `td_mapstart`) that fail-teleports target.
fn pick_gameplay_spawn(
    named: &HashMap<String, NamedPoint>,
    teleport_target_counts: &HashMap<String, usize>,
    player_spawns: &[(&str, NamedPoint)],
) -> Option<NamedPoint> {
    // 1) Well-known stage-start names (case-insensitive).
    const PREFERRED: &[&str] = &[
        "td_mapstart",
        "mapstart",
        "main_start",
        "stage1",
        "stage_1",
        "s1_start",
        "s1start",
        "td_start",
        "start",
    ];
    for want in PREFERRED {
        if let Some((_, pt)) = named
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(want))
        {
            return Some(*pt);
        }
    }
    // Names containing "mapstart" / "stage1".
    for (name, pt) in named {
        let lower = name.to_ascii_lowercase();
        if lower.contains("mapstart") || lower.contains("stage1") || lower.contains("s1_start")
        {
            return Some(*pt);
        }
    }

    // 2) Destination most targeted by trigger_teleport (fail teleporters → stage start).
    if let Some((name, _)) = teleport_target_counts
        .iter()
        .max_by_key(|(_, c)| *c)
    {
        if let Some(pt) = named.get(name) {
            // Ignore obscure single-use targets when counts are tiny and we have player spawns.
            if teleport_target_counts[name] >= 3 || player_spawns.is_empty() {
                return Some(*pt);
            }
        }
    }

    // 3) Fall back to player spawn entities: start → T → CT → any.
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

fn parse_model_index(s: &str) -> Option<usize> {
    let s = s.strip_prefix('*')?;
    s.parse().ok()
}
