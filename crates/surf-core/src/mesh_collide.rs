//! Triangle-mesh collision (displacements). Thin prism per triangle + sparse grid.

use crate::brush::{Aabb, Plane};
use crate::math::Vec3;
use std::collections::HashMap;

/// Upper bound on the planes [`CollisionTri::planes`] emits: face, back, three
/// edge planes, six axial planes, nine edge×axis bevels in both orientations.
pub const TRI_MAX_PLANES: usize = 29;

/// One displacement / mesh triangle as a thin solid prism for TraceBox.
///
/// Only the vertices are stored; the plane set is rebuilt per query. It is the
/// *bevelled* set — the same planes vbsp adds to a brush so that an expanded-
/// plane sweep is exact for an axis-aligned hull, and the same axes Source's
/// own displacement sweep tests (`CDispCollTree::SweepAABBTriIntersect`: the
/// triangle's AABB planes and its edge×axis planes). Without them the expanded
/// prism is fatter than the true hull-swept volume near its edges, and a hull
/// sliding *over* a triangle's boundary edge registers as entering it: the old
/// code hid that by discarding edge entries wholesale, which let a sweep that
/// also crossed the face plane pass straight through the surface.
#[derive(Clone, Debug)]
pub struct CollisionTri {
    pub verts: [Vec3; 3],
    /// Unit face normal (front side; the solid extends `thickness` behind it).
    pub normal: Vec3,
    pub thickness: f32,
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
        // Degenerate slivers have no usable edge planes.
        edge_plane(a, b, c, n)?;
        edge_plane(b, c, a, n)?;
        edge_plane(c, a, b, n)?;

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
            verts: [a, b, c],
            normal: n,
            thickness,
            bounds: Aabb::from_mins_maxs(mins, maxs),
        })
    }

    /// The front face as a plane.
    pub fn face(&self) -> Plane {
        Plane {
            normal: self.normal,
            dist: self.normal.dot(self.verts[0]),
        }
    }

    /// The bevelled plane set. Index 0 is the face, 1 the back, 2..5 the edge
    /// planes; everything after is a bevel (axial planes first).
    pub fn planes(&self) -> ([Plane; TRI_MAX_PLANES], usize) {
        self.planes_with(self.thickness)
    }

    /// Same set with an explicit slab thickness. Zero gives the planes of the
    /// bare triangle: a hull is "inside" them exactly when it straddles the
    /// face within the triangle's outline — the overlap test Source's
    /// displacement code uses for unswept boxes.
    pub fn planes_with(&self, thickness: f32) -> ([Plane; TRI_MAX_PLANES], usize) {
        let [a, b, c] = self.verts;
        let n = self.normal;
        let face = self.face();
        let mut out = [Plane { normal: Vec3::ZERO, dist: 0.0 }; TRI_MAX_PLANES];
        let mut count = 0usize;
        // No near-parallel merging: two planes a fraction of a degree apart
        // through a point 10k units from the origin differ by units in `dist`,
        // and giving one the other's distance moved a real surface by 2.4u
        // (cyberwave s2 ramp 2's end cap). Exact duplicates are harmless.
        let push = |out: &mut [Plane; TRI_MAX_PLANES], count: &mut usize, p: Plane| {
            out[*count] = p;
            *count += 1;
        };
        push(&mut out, &mut count, face);
        push(
            &mut out,
            &mut count,
            Plane {
                normal: -n,
                dist: -face.dist + thickness,
            },
        );
        // Edge planes are validated in `from_points`.
        push(&mut out, &mut count, edge_plane(a, b, c, n).unwrap());
        push(&mut out, &mut count, edge_plane(b, c, a, n).unwrap());
        push(&mut out, &mut count, edge_plane(c, a, b, n).unwrap());

        let back = [a - n * thickness, b - n * thickness, c - n * thickness];
        let all = [a, b, c, back[0], back[1], back[2]];
        let support = |normal: Vec3| -> f32 {
            all.iter().map(|v| normal.dot(*v)).fold(f32::MIN, f32::max)
        };
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        for axis in axes {
            for sign in [1.0f32, -1.0] {
                let normal = axis * sign;
                push(&mut out, &mut count, Plane { normal, dist: support(normal) });
            }
        }
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let edge = q - p;
            for axis in axes {
                let mut bevel = edge.cross(axis);
                let len = bevel.length();
                if len < 1e-6 {
                    continue;
                }
                bevel = bevel * (1.0 / len);
                // Both orientations are support planes of the prism and both
                // can be genuine faces of its hull (it is a thin slab), so add
                // both — exactly what vbsp's bevelling does for a brush.
                for normal in [bevel, -bevel] {
                    push(&mut out, &mut count, Plane { normal, dist: support(normal) });
                }
            }
        }
        (out, count)
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
        if t.normal.z >= 0.05 {
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
    fn gliding_over_a_boundary_edge_above_the_face_is_free() {
        // One isolated tri. A hull resting just above its face and sweeping
        // parallel to it across the x=0 boundary edge never touches the
        // prism: with the bevelled plane set the face plane alone rejects it,
        // which is what keeps a coplanar mesh seam from being a "1px wall".
        // (Below the face level the boundary edge *is* a real contact, and
        // Source reports it too — see `SweepAABBTriIntersect`'s axis planes.)
        let y_edge = 320.0;
        let z_rise = y_edge * 3.0_f32.sqrt();
        let tri = tri_up(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(0.0, y_edge, z_rise),
        );
        let world = World::with_tris(vec![], vec![tri.clone()], 128.0);
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        // Put the hull's support point 0.5u in front of the face plane.
        let n = tri.normal;
        let support = Vec3::new(
            if n.x > 0.0 { mins.x } else { maxs.x },
            if n.y > 0.0 { mins.y } else { maxs.y },
            if n.z > 0.0 { mins.z } else { maxs.z },
        );
        let y = 80.0;
        let mut origin = Vec3::new(-40.0, y, y * 3.0_f32.sqrt());
        let d = n.dot(origin + support) - tri.face().dist;
        origin += n * (0.5 - d);
        let along = Vec3::new(1.0, 0.0, 0.0);
        let tr = trace_box(&world, origin, origin + along * 80.0, mins, maxs);
        assert_eq!(tr.fraction, 1.0, "hit {:?}", tr.hit);
        assert!(!tr.startsolid);

        // Same sweep with the support point 0.5u *behind* the face is a real
        // contact at the boundary edge — stopped there, but the surface it
        // meets is reported as the face (a box on a convex piece, VPhysics
        // style), never a vertical wall through the edge.
        let deep = origin - n * 1.0;
        let tr = trace_box(&world, deep, deep + along * 80.0, mins, maxs);
        assert!(tr.fraction < 1.0, "should touch the edge");
        let h = tr.hit.expect("hit");
        assert!(h.plane_index >= 2, "entered through a side plane: {:?}", h);
        assert!(h.normal.dot(n) > 0.999, "face normal expected, got {:?}", h);
    }

    #[test]
    fn an_edge_entry_that_also_crosses_the_face_stops_at_the_surface() {
        // Come in from beside the tri, descending, so the sweep crosses the
        // face plane *and* the x=0 boundary. The old prism dropped the whole
        // tri because the entry plane was an edge, and the hull sailed through
        // the face and woke up inside — the cyberwave freeze.
        let y_edge = 320.0;
        let z_rise = y_edge * 3.0_f32.sqrt();
        let tri = tri_up(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(400.0, 0.0, 0.0),
            Vec3::new(0.0, y_edge, z_rise),
        );
        let world = World::with_tris(vec![], vec![tri], 128.0);
        let mins = Vec3::new(-16.0, -16.0, 0.0);
        let maxs = Vec3::new(16.0, 16.0, 62.0);
        let y = 100.0;
        let z_face = y * 3.0_f32.sqrt();
        let start = Vec3::new(-60.0, y, z_face + 40.0);
        let end = Vec3::new(60.0, y, z_face - 60.0);
        let tr = trace_box(&world, start, end, mins, maxs);
        assert!(tr.fraction < 1.0, "must be stopped, got {:?}", tr);
        assert!(!tr.startsolid);
        assert!(
            !crate::trace::point_contents_box(&world, tr.endpos, mins, maxs),
            "sweep ended inside the surface at {:?}",
            tr.endpos
        );
    }

    #[test]
    fn a_hull_straddling_a_face_is_pushed_out_not_frozen() {
        // Start embedded half a unit into the ramp face with speed along it.
        // Stock Source zeroes velocity here; the ramp fix nudges out along the
        // face and keeps going. Either way the hull must not sit still with
        // its speed intact, tick after tick.
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
        let n = drop.hit.expect("ramp").normal;
        let embedded = drop.endpos - n * 0.5;
        assert!(crate::trace::point_contents_box(&world, embedded, mins, maxs));
        let along = Vec3::new(900.0, 0.0, 0.0);
        let mut p = PlayerState {
            origin: embedded,
            velocity: along,
            grounded: false,
            ..PlayerState::default()
        };
        let cmd = UserCmd::default();
        let mut moved = 0.0;
        for _ in 0..3 {
            let next = tick(&world, &p, &cmd, &vars);
            moved += (next.origin - p.origin).length();
            p = next;
        }
        assert!(moved > 20.0, "moved only {moved:.2}u in 3 ticks; vel {:?}", p.velocity);
        assert!(
            !crate::trace::point_contents_box(&world, p.origin, mins, maxs),
            "still inside at {:?}",
            p.origin
        );
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
