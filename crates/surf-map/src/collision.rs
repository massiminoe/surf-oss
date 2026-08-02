//! World / trigger brush extraction.

use std::collections::HashSet;

use surf_core::brush::{Aabb, Brush, Plane};
use surf_core::math::Vec3;
use vbsp::{BrushFlags, Bsp};

use crate::leaves::LeafBrushRange;

/// MASK_PLAYERSOLID from bspflags.h (SDK 2013).
fn mask_playersolid() -> BrushFlags {
    BrushFlags::SOLID
        | BrushFlags::MOVEABLE
        | BrushFlags::PLAYERCLIP
        | BrushFlags::WINDOW
        | BrushFlags::MONSTER
        | BrushFlags::GRATE
}

/// Collect unique brush indices under a model's BSP head node.
pub fn collect_model_brushes(
    bsp: &Bsp,
    leaf_ranges: &[LeafBrushRange],
    head_node: i32,
) -> HashSet<usize> {
    let mut out = HashSet::new();
    let mut stack = vec![head_node];
    while let Some(node_idx) = stack.pop() {
        if node_idx < 0 {
            let leaf_i = (!node_idx) as usize;
            let Some(range) = leaf_ranges.get(leaf_i) else {
                continue;
            };
            let start = range.first as usize;
            let end = start + range.count as usize;
            for lb in bsp.leaf_brushes.get(start..end).into_iter().flatten() {
                out.insert(lb.brush as usize);
            }
            continue;
        }
        let Some(node) = bsp.nodes.get(node_idx as usize) else {
            continue;
        };
        stack.push(node.children[0]);
        stack.push(node.children[1]);
    }
    out
}

pub fn build_player_brushes(
    bsp: &Bsp,
    brush_indices: &HashSet<usize>,
    fallback_bounds: Aabb,
) -> Vec<Brush> {
    let mut brushes = Vec::new();
    for &bi in brush_indices {
        let Some(raw) = bsp.brushes.get(bi) else {
            continue;
        };
        if !raw.flags.intersects(mask_playersolid()) {
            continue;
        }
        // Skip pure areaportals even if somehow flagged.
        if raw.flags.contains(BrushFlags::AREAPORTAL)
            && !raw.flags.intersects(
                BrushFlags::SOLID
                    | BrushFlags::PLAYERCLIP
                    | BrushFlags::WINDOW
                    | BrushFlags::GRATE,
            )
        {
            continue;
        }
        if let Some(brush) = brush_from_bsp(bsp, bi, fallback_bounds) {
            brushes.push(brush);
        }
    }
    brushes
}

pub fn brush_from_bsp(bsp: &Bsp, brush_index: usize, fallback_bounds: Aabb) -> Option<Brush> {
    let raw = bsp.brushes.get(brush_index)?;
    let start = raw.brush_side as usize;
    let end = start + raw.num_brush_sides as usize;
    let mut planes = Vec::with_capacity(raw.num_brush_sides as usize);
    for side in bsp.brush_sides.get(start..end)? {
        let plane = bsp.planes.get(side.plane as usize)?;
        planes.push(Plane {
            normal: Vec3::new(plane.normal.x, plane.normal.y, plane.normal.z),
            dist: plane.dist,
        });
    }
    if planes.is_empty() {
        return None;
    }
    let bounds = bounds_from_axial_planes(&planes).unwrap_or(fallback_bounds);
    Some(Brush { planes, bounds })
}

/// VBSP emits axial bevels on every brush — recover AABB from those.
fn bounds_from_axial_planes(planes: &[Plane]) -> Option<Aabb> {
    let mut mins = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut maxs = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut got = [false; 6];

    for p in planes {
        let n = p.normal;
        if (n.x + 1.0).abs() < 0.01 && n.y.abs() < 0.01 && n.z.abs() < 0.01 {
            // n = (-1,0,0), inside: -x <= dist → x >= -dist
            mins.x = mins.x.max(-p.dist);
            got[0] = true;
        } else if (n.x - 1.0).abs() < 0.01 && n.y.abs() < 0.01 && n.z.abs() < 0.01 {
            maxs.x = maxs.x.min(p.dist);
            got[1] = true;
        } else if n.x.abs() < 0.01 && (n.y + 1.0).abs() < 0.01 && n.z.abs() < 0.01 {
            mins.y = mins.y.max(-p.dist);
            got[2] = true;
        } else if n.x.abs() < 0.01 && (n.y - 1.0).abs() < 0.01 && n.z.abs() < 0.01 {
            maxs.y = maxs.y.min(p.dist);
            got[3] = true;
        } else if n.x.abs() < 0.01 && n.y.abs() < 0.01 && (n.z + 1.0).abs() < 0.01 {
            mins.z = mins.z.max(-p.dist);
            got[4] = true;
        } else if n.x.abs() < 0.01 && n.y.abs() < 0.01 && (n.z - 1.0).abs() < 0.01 {
            maxs.z = maxs.z.min(p.dist);
            got[5] = true;
        }
    }

    if !got.iter().all(|&g| g)
        || !mins.x.is_finite()
        || !maxs.x.is_finite()
        || mins.x > maxs.x
        || mins.y > maxs.y
        || mins.z > maxs.z
    {
        return None;
    }
    Some(Aabb::from_mins_maxs(mins, maxs))
}
