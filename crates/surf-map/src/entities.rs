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
    let mut named: HashMap<String, NamedEntity> = HashMap::new();
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
            let is_teleport_dest = matches!(
                class,
                "info_teleport_destination" | "info_target" | "info_landmark"
            );
            let point = NamedPoint { origin, angles };
            // Prefer a real teleport destination when several ents share a name.
            match named.get(name) {
                Some(prev) if prev.is_teleport_dest && !is_teleport_dest => {}
                _ => {
                    named.insert(
                        name.to_string(),
                        NamedEntity {
                            point,
                            is_teleport_dest,
                        },
                    );
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
            dest_origin: dest.point.origin,
            dest_angles: dest.point.angles,
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
    named: &HashMap<String, NamedEntity>,
    teleport_target_counts: &HashMap<String, usize>,
    player_spawns: &[(&str, NamedPoint)],
) -> Option<NamedPoint> {
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
        let mut hits: Vec<&NamedEntity> = named
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(want) && !is_bonus_or_later_stage(n))
            .map(|(_, ent)| ent)
            .collect();
        hits.sort_by_key(|e| !e.is_teleport_dest);
        if let Some(ent) = hits.first() {
            return Some(ent.point);
        }
    }

    // 2) Best-scored name among non-bonus ents (stable: name order).
    let mut scored: Vec<(i32, &str, &NamedEntity)> = named
        .iter()
        .filter(|(n, _)| !is_bonus_or_later_stage(n))
        .map(|(n, ent)| (score_spawn_name(n, ent.is_teleport_dest), n.as_str(), ent))
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
            let score = score_spawn_name(n, ent.is_teleport_dest);
            Some((score, count, n.as_str()))
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

    // 4) Fall back to player spawn entities: start → T → CT → any.
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

fn parse_model_index(s: &str) -> Option<usize> {
    let s = s.strip_prefix('*')?;
    s.parse().ok()
}
