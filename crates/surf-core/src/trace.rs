//! Swept AABB vs convex brush planes (Quake / Source lineage).
//!
//! Contract for movement:
//! - `fraction` in [0, 1]; impact at `start + (end-start)*fraction` pulled back by
//!   [`DIST_EPSILON`] along the sweep so the hull stops short of the surface.
//! - `startsolid` / `allsolid` match Source semantics.
//! - Displacement triangles are thin prisms tested the same way (grid broadphase).

use crate::brush::{Aabb, Brush, Plane, World};
use crate::math::Vec3;

/// Source `DIST_EPSILON` — traces stop this far short of surfaces.
pub const DIST_EPSILON: f32 = 0.03125;

#[derive(Clone, Copy, Debug)]
pub struct TraceHit {
    pub normal: Vec3,
    pub dist: f32,
    pub brush_index: usize,
    pub plane_index: usize,
}

#[derive(Clone, Debug)]
pub struct TraceResult {
    pub startsolid: bool,
    pub allsolid: bool,
    pub fraction: f32,
    pub endpos: Vec3,
    pub hit: Option<TraceHit>,
}

impl TraceResult {
    fn free(end: Vec3) -> Self {
        Self {
            startsolid: false,
            allsolid: false,
            fraction: 1.0,
            endpos: end,
            hit: None,
        }
    }
}

/// Expand a plane outward so an AABB (mins/maxs relative to origin) becomes a point.
fn expand_plane(plane: Plane, mins: Vec3, maxs: Vec3) -> Plane {
    let mut offset = 0.0;
    // For each axis, pick the support point of the AABB in the -normal direction
    // (hull "into" the plane), then shift the plane out by that support.
    offset += if plane.normal.x > 0.0 {
        -plane.normal.x * mins.x
    } else {
        -plane.normal.x * maxs.x
    };
    offset += if plane.normal.y > 0.0 {
        -plane.normal.y * mins.y
    } else {
        -plane.normal.y * maxs.y
    };
    offset += if plane.normal.z > 0.0 {
        -plane.normal.z * mins.z
    } else {
        -plane.normal.z * maxs.z
    };
    Plane {
        normal: plane.normal,
        dist: plane.dist + offset,
    }
}

fn aabb_overlaps_brush_bounds(
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    brush: &Brush,
) -> bool {
    let path_mins = Vec3::new(
        start.x.min(end.x) + mins.x,
        start.y.min(end.y) + mins.y,
        start.z.min(end.z) + mins.z,
    );
    let path_maxs = Vec3::new(
        start.x.max(end.x) + maxs.x,
        start.y.max(end.y) + maxs.y,
        start.z.max(end.z) + maxs.z,
    );
    let b = brush.bounds;
    path_maxs.x >= b.mins.x
        && path_mins.x <= b.maxs.x
        && path_maxs.y >= b.mins.y
        && path_mins.y <= b.maxs.y
        && path_maxs.z >= b.mins.z
        && path_mins.z <= b.maxs.z
}

/// Trace against a convex plane set (brush or thin triangle prism).
fn trace_planes(
    planes: &[Plane],
    solid_index: usize,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
) -> TraceResult {
    let mut enter_frac = -1.0f32;
    let mut leave_frac = 1.0f32;
    let mut startsolid = true;
    let mut hit_plane: Option<(Plane, usize)> = None;

    for (plane_index, plane) in planes.iter().enumerate() {
        let ep = expand_plane(*plane, mins, maxs);
        let d1 = ep.distance(start);
        let d2 = ep.distance(end);

        if d1 > 0.0 {
            startsolid = false;
        }

        // Both in front: miss this solid entirely.
        if d1 > 0.0 && d2 > 0.0 {
            return TraceResult::free(end);
        }
        // Both behind: this plane doesn't clip the segment.
        if d1 <= 0.0 && d2 <= 0.0 {
            continue;
        }

        if d1 > d2 {
            // Crossing in (entering the half-space behind the plane).
            let mut f = (d1 - DIST_EPSILON) / (d1 - d2);
            if f < 0.0 {
                f = 0.0;
            }
            if f > enter_frac {
                enter_frac = f;
                hit_plane = Some((ep, plane_index));
            }
        } else {
            // Crossing out.
            let mut f = (d1 + DIST_EPSILON) / (d1 - d2);
            if f > 1.0 {
                f = 1.0;
            }
            if f < leave_frac {
                leave_frac = f;
            }
        }
    }

    if startsolid {
        let end_in = planes.iter().all(|p| {
            let ep = expand_plane(*p, mins, maxs);
            ep.distance(end) <= DIST_EPSILON
        });
        return TraceResult {
            startsolid: true,
            allsolid: end_in,
            fraction: 0.0,
            endpos: start,
            hit: None,
        };
    }

    if enter_frac < leave_frac && enter_frac >= 0.0 && enter_frac < 1.0 {
        let fraction = enter_frac.clamp(0.0, 1.0);
        let (plane, plane_index) = hit_plane.expect("enter_frac set implies hit plane");
        let endpos = start + (end - start) * fraction;
        return TraceResult {
            startsolid: false,
            allsolid: false,
            fraction,
            endpos,
            hit: Some(TraceHit {
                normal: plane.normal,
                dist: plane.dist,
                brush_index: solid_index,
                plane_index,
            }),
        };
    }

    TraceResult::free(end)
}

fn path_bounds(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Aabb {
    Aabb::from_mins_maxs(
        Vec3::new(
            start.x.min(end.x) + mins.x,
            start.y.min(end.y) + mins.y,
            start.z.min(end.z) + mins.z,
        ),
        Vec3::new(
            start.x.max(end.x) + maxs.x,
            start.y.max(end.y) + maxs.y,
            start.z.max(end.z) + maxs.z,
        ),
    )
}

/// Swept AABB from `start`→`end` (origin = feet). `mins`/`maxs` are hull extents
/// relative to origin (CS:S stand: mins=(-16,-16,0), maxs=(16,16,62)).
pub fn trace_box(
    world: &World,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
) -> TraceResult {
    let mut best = TraceResult::free(end);
    let mut any_startsolid = false;

    for (brush_index, brush) in world.brushes.iter().enumerate() {
        if !aabb_overlaps_brush_bounds(start, end, mins, maxs, brush) {
            continue;
        }
        let tr = trace_planes(&brush.planes, brush_index, start, end, mins, maxs);
        if tr.startsolid {
            any_startsolid = true;
            if tr.allsolid {
                return TraceResult {
                    startsolid: true,
                    allsolid: true,
                    fraction: 0.0,
                    endpos: start,
                    hit: None,
                };
            }
            if tr.fraction < best.fraction {
                best = tr;
            }
            continue;
        }
        if tr.fraction < best.fraction {
            best = tr;
        }
    }

    if !world.tris.is_empty() {
        let pb = path_bounds(start, end, mins, maxs);
        let mut ids = Vec::new();
        world.tri_grid.query(pb, &mut ids);
        let brush_base = world.brushes.len();
        for ti in ids {
            let tri = &world.tris[ti as usize];
            if pb.maxs.x < tri.bounds.mins.x
                || pb.mins.x > tri.bounds.maxs.x
                || pb.maxs.y < tri.bounds.mins.y
                || pb.mins.y > tri.bounds.maxs.y
                || pb.maxs.z < tri.bounds.mins.z
                || pb.mins.z > tri.bounds.maxs.z
            {
                continue;
            }
            let solid_index = brush_base + ti as usize;
            let tr = trace_planes(&tri.planes, solid_index, start, end, mins, maxs);
            // Thin-prism edge planes are walls at every triangle seam. Surfing into
            // them is the classic "1px wall" snag (nyx @ ghost tick 979: edge hit
            // dropped 2629→378 u/s). Skip edge hits — neighboring face planes still
            // provide the surfable surface; startsolid still uses the full prism.
            if tr
                .hit
                .as_ref()
                .is_some_and(|h| h.plane_index >= 2)
            {
                continue;
            }
            if tr.startsolid {
                any_startsolid = true;
                if tr.allsolid {
                    return TraceResult {
                        startsolid: true,
                        allsolid: true,
                        fraction: 0.0,
                        endpos: start,
                        hit: None,
                    };
                }
                if tr.fraction < best.fraction {
                    best = tr;
                }
                continue;
            }
            if tr.fraction < best.fraction {
                best = tr;
            }
        }
    }

    if any_startsolid && best.hit.is_none() && best.fraction >= 1.0 {
        best.startsolid = true;
        best.fraction = 0.0;
        best.endpos = start;
    } else if any_startsolid {
        best.startsolid = true;
    }

    best
}

/// Point contents test for the player hull at `origin` (unswept).
pub fn point_contents_box(world: &World, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
    let tr = trace_box(world, origin, origin, mins, maxs);
    tr.startsolid || tr.allsolid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::World;

    fn floor_world() -> World {
        // Floor slab z=-16..0, large XY.
        World::new(vec![Brush::aabb(
            Vec3::new(-1024.0, -1024.0, -16.0),
            Vec3::new(1024.0, 1024.0, 0.0),
        )])
    }

    #[test]
    fn drops_onto_floor() {
        let world = floor_world();
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let start = Vec3::new(0.0, 0.0, 100.0);
        let end = Vec3::new(0.0, 0.0, -50.0);
        let tr = trace_box(&world, start, end, mins, maxs);
        assert!(tr.fraction < 1.0);
        assert!(!tr.allsolid);
        // Feet should stop just above z=0.
        assert!(tr.endpos.z > -0.1 && tr.endpos.z < 1.0);
        let hit = tr.hit.expect("should hit floor");
        assert!(hit.normal.z > 0.9);
    }

    #[test]
    fn misses_open_air() {
        let world = floor_world();
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let tr = trace_box(
            &world,
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(10.0, 0.0, 100.0),
            mins,
            maxs,
        );
        assert_eq!(tr.fraction, 1.0);
        assert!(tr.hit.is_none());
    }

    #[test]
    fn drops_onto_triangle_floor() {
        use crate::mesh_collide::CollisionTri;
        let tri = CollisionTri::from_points(
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, -64.0, 0.0),
            Vec3::new(0.0, 64.0, 0.0),
            2.0,
        )
        .expect("tri");
        let world = World::with_tris(vec![], vec![tri], 256.0);
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let tr = trace_box(
            &world,
            Vec3::new(0.0, 0.0, 80.0),
            Vec3::new(0.0, 0.0, -40.0),
            mins,
            maxs,
        );
        assert!(tr.fraction < 1.0, "frac={}", tr.fraction);
        assert!(tr.endpos.z > -0.5 && tr.endpos.z < 2.0, "z={}", tr.endpos.z);
        let hit = tr.hit.expect("hit");
        assert!(hit.normal.z > 0.9, "n.z={}", hit.normal.z);
    }
}
