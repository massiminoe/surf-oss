//! Triangle-mesh collision (displacements). Thin prism per triangle + sparse grid.

use crate::brush::{Aabb, Plane};
use crate::math::Vec3;
use std::collections::HashMap;

/// One displacement / mesh triangle as a thin solid prism for TraceBox.
#[derive(Clone, Debug)]
pub struct CollisionTri {
    pub planes: [Plane; 5], // face, back, edge0, edge1, edge2
    pub bounds: Aabb,
}

impl CollisionTri {
    /// Build a thin prism. `thickness` is how far the solid extends behind the face.
    pub fn from_points(a: Vec3, b: Vec3, c: Vec3, thickness: f32) -> Option<Self> {
        let mut n = (b - a).cross(c - a);
        let len = n.length();
        if len < 1e-5 {
            return None;
        }
        n = n * (1.0 / len);

        let face = Plane {
            normal: n,
            dist: n.dot(a),
        };
        let back = Plane {
            normal: -n,
            dist: -face.dist + thickness,
        };
        let e0 = edge_plane(a, b, c, n)?;
        let e1 = edge_plane(b, c, a, n)?;
        let e2 = edge_plane(c, a, b, n)?;

        let mut mins = a;
        let mut maxs = a;
        for p in [b, c] {
            mins.x = mins.x.min(p.x);
            mins.y = mins.y.min(p.y);
            mins.z = mins.z.min(p.z);
            maxs.x = maxs.x.max(p.x);
            maxs.y = maxs.y.max(p.y);
            maxs.z = maxs.z.max(p.z);
        }
        // Pad Z a bit for the slab thickness / hull.
        mins.z -= thickness;
        maxs.z += 1.0;

        Some(Self {
            planes: [face, back, e0, e1, e2],
            bounds: Aabb::from_mins_maxs(mins, maxs),
        })
    }
}

fn edge_plane(a: Vec3, b: Vec3, interior: Vec3, face_n: Vec3) -> Option<Plane> {
    let edge = b - a;
    let mut n = face_n.cross(edge);
    let len = n.length();
    if len < 1e-6 {
        return None;
    }
    n = n * (1.0 / len);
    let mut dist = n.dot(a);
    // Outward: interior must be behind (distance <= 0).
    if n.dot(interior) - dist > 0.0 {
        n = -n;
        dist = n.dot(a);
    }
    Some(Plane { normal: n, dist })
}

/// Sparse uniform grid over triangle indices.
#[derive(Clone, Debug, Default)]
pub struct TriGrid {
    pub cell_size: f32,
    pub cells: HashMap<(i16, i16, i16), Vec<u32>>,
}

impl TriGrid {
    pub fn build(tris: &[CollisionTri], cell_size: f32) -> Self {
        let mut cells: HashMap<(i16, i16, i16), Vec<u32>> = HashMap::new();
        for (i, tri) in tris.iter().enumerate() {
            let (x0, y0, z0) = cell_of(tri.bounds.mins, cell_size);
            let (x1, y1, z1) = cell_of(tri.bounds.maxs, cell_size);
            for z in z0..=z1 {
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        cells.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        Self { cell_size, cells }
    }

    /// Unique triangle indices whose cells overlap `bounds`.
    pub fn query(&self, bounds: Aabb, out: &mut Vec<u32>) {
        out.clear();
        if self.cells.is_empty() {
            return;
        }
        let (x0, y0, z0) = cell_of(bounds.mins, self.cell_size);
        let (x1, y1, z1) = cell_of(bounds.maxs, self.cell_size);
        for z in z0..=z1 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if let Some(list) = self.cells.get(&(x, y, z)) {
                        out.extend_from_slice(list);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

fn cell_of(p: Vec3, cell: f32) -> (i16, i16, i16) {
    (
        (p.x / cell).floor() as i16,
        (p.y / cell).floor() as i16,
        (p.z / cell).floor() as i16,
    )
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::brush::World;
    use crate::math::{Angle, Vec3};
    use crate::movement::{MoveVars, PlayerState, UserCmd};
    use crate::tick::tick;
    use crate::trace::trace_box;

    fn tri_up(a: Vec3, b: Vec3, c: Vec3) -> CollisionTri {
        let t = CollisionTri::from_points(a, b, c, 2.0).expect("tri");
        if t.planes[0].normal.z >= 0.05 {
            t
        } else {
            CollisionTri::from_points(a, c, b, 2.0).expect("flipped tri")
        }
    }

    /// Two coplanar 60° ramp tris sharing the X=0 edge (disp/prop mesh seam).
    fn seamed_ramp() -> World {
        let y_edge = 320.0;
        let z_rise = y_edge * 3.0_f32.sqrt();
        let v00 = Vec3::new(-200.0, 0.0, 0.0);
        let v10 = Vec3::new(200.0, 0.0, 0.0);
        let v01 = Vec3::new(-200.0, y_edge, z_rise);
        let v11 = Vec3::new(200.0, y_edge, z_rise);
        let mid0 = Vec3::new(0.0, 0.0, 0.0);
        let mid1 = Vec3::new(0.0, y_edge, z_rise);
        World::with_tris(
            vec![],
            vec![
                tri_up(v00, mid0, mid1),
                tri_up(v00, mid1, v01),
                tri_up(mid0, v10, v11),
                tri_up(mid0, v11, mid1),
            ],
            128.0,
        )
    }

    #[test]
    fn triangle_edge_planes_do_not_block_along_face_sweep() {
        // One isolated tri — a sideways sweep that only enters via an edge plane
        // must not report a wall hit (the bug that stopped nyx @ 2629→378 u/s).
        let y_edge = 320.0;
        let z_rise = y_edge * 3.0_f32.sqrt();
        let tri = tri_up(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(0.0, y_edge, z_rise),
        );
        // Confirm the prism actually has blocking edge planes before the skip.
        assert!(tri.planes.len() == 5);
        let world = World::with_tris(vec![], vec![tri], 128.0);
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        // Start left of the x=0 edge, on the face height, sweep +X into the edge.
        let y = 80.0;
        let z = y * 3.0_f32.sqrt() + 1.0;
        let start = Vec3::new(-40.0, y, z);
        let end = Vec3::new(40.0, y, z);
        let tr = trace_box(&world, start, end, mins, maxs);
        if let Some(h) = tr.hit {
            assert!(
                h.plane_index < 2,
                "edge plane must not block sweeps: plane={} n={:?}",
                h.plane_index,
                h.normal
            );
            assert!(
                h.normal.z > 0.4,
                "if we hit, it should be the ramp face, n={:?}",
                h.normal
            );
        }
    }

    #[test]
    fn glide_across_mesh_seam_keeps_speed() {
        let world = seamed_ramp();
        let vars = MoveVars::momentum_surf();
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let y = 120.0;
        let z_face = y * 3.0_f32.sqrt();
        let drop = trace_box(
            &world,
            Vec3::new(-80.0, y, z_face + 64.0),
            Vec3::new(-80.0, y, z_face - 64.0),
            mins,
            maxs,
        );
        assert!(drop.fraction < 1.0, "must find ramp");
        let mut p = PlayerState {
            origin: drop.endpos,
            velocity: Vec3::new(900.0, -80.0, 0.0),
            grounded: false,
            viewangles: Angle::new(0.0, 0.0, 0.0),
            ..PlayerState::default()
        };
        let cmd = UserCmd {
            side_move: -1.0,
            viewangles: Angle::new(0.0, 0.0, 0.0),
            ..UserCmd::default()
        };
        for _ in 0..8 {
            p = tick(&world, &p, &cmd, &vars);
        }
        let before = p.velocity.length_2d();
        assert!(before > 500.0, "pre-seam speed {before}");
        let mut min_spd = before;
        let mut crossed = false;
        for _ in 0..40 {
            p = tick(&world, &p, &cmd, &vars);
            min_spd = min_spd.min(p.velocity.length_2d());
            if p.origin.x > 40.0 {
                crossed = true;
                break;
            }
        }
        assert!(
            crossed,
            "cross seam; stuck {:?} spd={:.0}",
            p.origin,
            p.velocity.length_2d()
        );
        assert!(
            min_spd > before * 0.7,
            "mesh seam 1px wall: {before:.0}→min {min_spd:.0}"
        );
    }
}
