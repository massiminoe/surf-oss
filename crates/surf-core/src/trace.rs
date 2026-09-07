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
/// How far below a triangle's face a hull may sit and still have a side-plane
/// entry read as contact with the face (a seam crossing) rather than with the
/// prism's side (a lip or end wall).
const SEAM_LEVEL_TOLERANCE: f32 = 1.0;

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
    /// When the hull starts straddling a mesh surface: that surface's face
    /// normal, i.e. the way out. A sweep that starts inside has no impact
    /// plane of its own, and the move loop needs *some* direction to push
    /// the hull along or it stays put for good.
    pub stuck_normal: Option<Vec3>,
}

impl TraceResult {
    fn all_solid(start: Vec3, stuck_normal: Option<Vec3>) -> Self {
        Self {
            startsolid: true,
            allsolid: true,
            fraction: 0.0,
            endpos: start,
            hit: None,
            stuck_normal,
        }
    }

    fn free(end: Vec3) -> Self {
        Self {
            startsolid: false,
            allsolid: false,
            fraction: 1.0,
            endpos: end,
            hit: None,
            stuck_normal: None,
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
            stuck_normal: None,
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
            stuck_normal: None,
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
pub fn trace_box(world: &World, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> TraceResult {
    let mut best = TraceResult::free(end);
    let mut any_startsolid = false;
    let mut stuck_normal: Option<Vec3> = None;

    for (brush_index, brush) in world.brushes.iter().enumerate() {
        if !aabb_overlaps_brush_bounds(start, end, mins, maxs, brush) {
            continue;
        }
        let tr = trace_planes(&brush.planes, brush_index, start, end, mins, maxs);
        if tr.startsolid {
            any_startsolid = true;
            if tr.allsolid {
                return TraceResult::all_solid(start, stuck_normal);
            }
            // Started inside but the sweep leaves: Source lets the hull out
            // rather than clipping it to a zero move (the brush sets
            // `startsolid` and is otherwise ignored). Clamping to fraction 0
            // here is what froze a hull in place with its speed intact.
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
            // Source's displacement sweep is one-sided: a box moving away from
            // the face never registers (`SweepAABBTriIntersect` bails when the
            // delta runs along the normal). That is also what lets a hull that
            // ended up behind — or half-way through — a surface leave through
            // it instead of being held; the overlap test below must not see
            // it either. An unswept test has no direction and counts it.
            if (end - start).dot(tri.normal) > DIST_EPSILON {
                continue;
            }
            // "Inside" a triangle means straddling its face within its outline
            // — the bare-triangle plane set, which is the overlap test Source's
            // displacement code applies to an unswept box. The 2u slab below
            // is only for clipping; a hull that is merely *behind* a surface
            // is not stuck in it, and can leave through it (below).
            let (thin, thin_n) = tri.planes_with(0.0);
            let overlap = trace_planes(&thin[..thin_n], solid_index, start, start, mins, maxs);
            if overlap.startsolid {
                any_startsolid = true;
                if stuck_normal.is_none() {
                    stuck_normal = Some(tri.normal);
                }
                let end_in = thin[..thin_n]
                    .iter()
                    .all(|p| expand_plane(*p, mins, maxs).distance(end) <= DIST_EPSILON);
                if end_in {
                    return TraceResult::all_solid(start, stuck_normal);
                }
                continue;
            }
            let (planes, count) = tri.planes();
            // The prism's plane set is bevelled (see `CollisionTri`), so the
            // expanded-plane sweep is exact for the hull and whichever plane
            // sets the entry fraction is a real contact — an edge or axial
            // plane included, exactly as Source reports for displacements. The
            // old five-plane prism was fatter than the true swept volume near
            // its edges, and the workaround (discard any edge entry) let a
            // sweep that also crossed the face plane pass straight through it:
            // the hull then woke up inside the ramp and froze with its speed
            // intact (cyberwave s2 ramp 2, 1800 u/s at (766,-10190,-10007)).
            let mut tr = trace_planes(&planes[..count], solid_index, start, end, mins, maxs);
            if tr.startsolid {
                // Behind the face but not straddling it: not a contact.
                continue;
            }
            if let Some(h) = tr.hit.as_mut() {
                if h.plane_index == 1 {
                    // Entered through the back: the hull is behind the surface
                    // and heading out. Source's one-sided sweep lets it pass.
                    continue;
                }
                if h.plane_index >= 2 {
                    // Entered through an edge, axial or bevel plane. The
                    // fraction is the true first contact with this prism. If
                    // the hull is at or above the face's level, the surface it
                    // meets is the face: a hull sliding across a seam between
                    // two triangles crosses the seam's side planes a hair
                    // before it re-touches the (coplanar) face, and a hull
                    // landing on a face near its edge touches that face, not a
                    // vertical wall through the edge. That is what VPhysics
                    // reports for a box on a convex piece, and it is what stops
                    // cyberwave tick 1211 (2651 -> 960 u/s against a seam
                    // bevel) from being a wall. A hull well *below* the face
                    // level is hitting the prism's side for real — the ramp's
                    // end wall is the neighbouring triangle's edge too, and
                    // reporting the top face there left a hull pressed against
                    // the wall with nothing to clip its speed.
                    let face = expand_plane(tri.face(), mins, maxs);
                    if face.distance(start) > -SEAM_LEVEL_TOLERANCE {
                        h.normal = face.normal;
                        h.dist = face.dist;
                    }
                }
            }
            if tr.fraction < best.fraction {
                best = tr;
            }
        }
    }

    if any_startsolid {
        best.startsolid = true;
        best.stuck_normal = stuck_normal;
    }

    best
}

/// `trace_planes` for diagnostics — lets tools ask what a single solid says
/// about a sweep, independent of the broadphase and the edge-plane filter.
pub fn trace_planes_pub(
    planes: &[Plane],
    solid_index: usize,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
) -> TraceResult {
    trace_planes(planes, solid_index, start, end, mins, maxs)
}

/// Point contents test for the player hull at `origin` (unswept).
pub fn point_contents_box(world: &World, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
    let tr = trace_box(world, origin, origin, mins, maxs);
    tr.startsolid || tr.allsolid
}

/// Is the hull at `origin` stuck in something, given that it is moving with
/// `velocity`? Like [`point_contents_box`], except that a mesh surface the
/// hull is passing *out through* (moving along its normal) does not count —
/// the sweep is one-sided and lets the hull leave, so the "will I be stuck"
/// re-test the move loop runs on a full-length sweep must agree, or a hull
/// that is half-way out of a ramp plate stops there for good. Brushes are
/// closed solids and always count.
pub fn hull_is_stuck(world: &World, origin: Vec3, velocity: Vec3, mins: Vec3, maxs: Vec3) -> bool {
    for brush in &world.brushes {
        if !aabb_overlaps_brush_bounds(origin, origin, mins, maxs, brush) {
            continue;
        }
        if trace_planes(&brush.planes, 0, origin, origin, mins, maxs).startsolid {
            return true;
        }
    }
    if world.tris.is_empty() {
        return false;
    }
    let pb = path_bounds(origin, origin, mins, maxs);
    let mut ids = Vec::new();
    world.tri_grid.query(pb, &mut ids);
    for ti in ids {
        let tri = &world.tris[ti as usize];
        if velocity.dot(tri.normal) > 0.0 {
            continue;
        }
        let (thin, n) = tri.planes_with(0.0);
        if trace_planes(&thin[..n], 0, origin, origin, mins, maxs).startsolid {
            return true;
        }
    }
    false
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
