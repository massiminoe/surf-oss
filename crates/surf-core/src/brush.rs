//! Convex brushes as outward-facing plane sets (Source/Quake style).

use crate::math::Vec3;
use crate::mesh_collide::{CollisionTri, TriGrid};

/// Plane: points with `normal · p - dist > 0` are outside (in front).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
}

impl Plane {
    pub fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Self {
        let normal = (c - b).cross(a - b).normalize();
        let dist = normal.dot(a);
        Self { normal, dist }
    }

    pub fn from_normal_point(normal: Vec3, point: Vec3) -> Self {
        let normal = normal.normalize();
        Self {
            normal,
            dist: normal.dot(point),
        }
    }

    #[inline]
    pub fn distance(self, p: Vec3) -> f32 {
        self.normal.dot(p) - self.dist
    }
}

/// Axis-aligned extents of a convex brush (for broadphase / mesh helpers).
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl Aabb {
    pub fn from_mins_maxs(mins: Vec3, maxs: Vec3) -> Self {
        Self { mins, maxs }
    }

    pub fn contains_point(self, p: Vec3) -> bool {
        p.x >= self.mins.x
            && p.x <= self.maxs.x
            && p.y >= self.mins.y
            && p.y <= self.maxs.y
            && p.z >= self.mins.z
            && p.z <= self.maxs.z
    }

    pub fn expand(self, amount: f32) -> Self {
        Self {
            mins: self.mins - Vec3::new(amount, amount, amount),
            maxs: self.maxs + Vec3::new(amount, amount, amount),
        }
    }
}

/// Convex solid defined by outward planes. Collision only (render mesh is separate).
#[derive(Clone, Debug)]
pub struct Brush {
    pub planes: Vec<Plane>,
    pub bounds: Aabb,
}

impl Brush {
    /// Axis-aligned box brush (six planes).
    pub fn aabb(mins: Vec3, maxs: Vec3) -> Self {
        let planes = vec![
            Plane {
                normal: Vec3::new(-1.0, 0.0, 0.0),
                dist: -mins.x,
            },
            Plane {
                normal: Vec3::new(1.0, 0.0, 0.0),
                dist: maxs.x,
            },
            Plane {
                normal: Vec3::new(0.0, -1.0, 0.0),
                dist: -mins.y,
            },
            Plane {
                normal: Vec3::new(0.0, 1.0, 0.0),
                dist: maxs.y,
            },
            Plane {
                normal: Vec3::new(0.0, 0.0, -1.0),
                dist: -mins.z,
            },
            Plane {
                normal: Vec3::new(0.0, 0.0, 1.0),
                dist: maxs.z,
            },
        ];
        Self {
            planes,
            bounds: Aabb::from_mins_maxs(mins, maxs),
        }
    }

    /// Point-classified: true if strictly inside or on surface (within epsilon).
    pub fn contains_point(&self, p: Vec3, epsilon: f32) -> bool {
        for plane in &self.planes {
            if plane.distance(p) > epsilon {
                return false;
            }
        }
        true
    }
}

/// Static collision world: convex brushes + optional triangle mesh (displacements).
#[derive(Clone, Debug, Default)]
pub struct World {
    pub brushes: Vec<Brush>,
    pub tris: Vec<CollisionTri>,
    pub tri_grid: TriGrid,
}

impl World {
    pub fn new(brushes: Vec<Brush>) -> Self {
        Self {
            brushes,
            tris: Vec::new(),
            tri_grid: TriGrid::default(),
        }
    }

    pub fn with_tris(brushes: Vec<Brush>, tris: Vec<CollisionTri>, cell_size: f32) -> Self {
        let tri_grid = TriGrid::build(&tris, cell_size);
        Self {
            brushes,
            tris,
            tri_grid,
        }
    }

    pub fn empty() -> Self {
        Self {
            brushes: Vec::new(),
            tris: Vec::new(),
            tri_grid: TriGrid::default(),
        }
    }
}
