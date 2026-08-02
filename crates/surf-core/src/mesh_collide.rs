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
