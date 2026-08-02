//! Source-unit math: right-handed, Z-up. Degrees for view angles (yaw/pitch/roll).

use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const X: Self = Self {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };
    pub const Y: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub const Z: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    #[inline]
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    #[inline]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[inline]
    pub fn length_2d(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[inline]
    pub fn length_2d_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// Normalize; returns ZERO if length is tiny.
    #[inline]
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len > 1e-6 {
            self * (1.0 / len)
        } else {
            Self::ZERO
        }
    }

    /// Normalize in place; returns previous length.
    #[inline]
    pub fn normalize_len(&mut self) -> f32 {
        let len = self.length();
        if len > 1e-6 {
            *self = *self * (1.0 / len);
        } else {
            *self = Self::ZERO;
        }
        len
    }

    #[inline]
    pub fn with_z(self, z: f32) -> Self {
        Self {
            x: self.x,
            y: self.y,
            z,
        }
    }

    #[inline]
    pub fn xy(self) -> Self {
        Self {
            x: self.x,
            y: self.y,
            z: 0.0,
        }
    }

    #[inline]
    pub fn abs(self) -> Self {
        Self {
            x: self.x.abs(),
            y: self.y.abs(),
            z: self.z.abs(),
        }
    }

    #[inline]
    pub fn is_nan(self) -> bool {
        self.x.is_nan() || self.y.is_nan() || self.z.is_nan()
    }

    #[inline]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self + (other - self) * t
    }
}

impl Add for Vec3 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec3 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }
}

impl MulAssign<f32> for Vec3 {
    #[inline]
    fn mul_assign(&mut self, s: f32) {
        *self = *self * s;
    }
}

impl Neg for Vec3 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

/// View angles in degrees (Source convention: pitch, yaw, roll).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Angle {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl Angle {
    pub const ZERO: Self = Self {
        pitch: 0.0,
        yaw: 0.0,
        roll: 0.0,
    };

    #[inline]
    pub const fn new(pitch: f32, yaw: f32, roll: f32) -> Self {
        Self { pitch, yaw, roll }
    }

    /// Build forward / right / up basis from view angles (degrees).
    pub fn vectors(self) -> (Vec3, Vec3, Vec3) {
        let pitch = self.pitch.to_radians();
        let yaw = self.yaw.to_radians();
        let roll = self.roll.to_radians();

        let sp = pitch.sin();
        let cp = pitch.cos();
        let sy = yaw.sin();
        let cy = yaw.cos();
        let sr = roll.sin();
        let cr = roll.cos();

        let forward = Vec3::new(cp * cy, cp * sy, -sp);
        let right = Vec3::new(
            -sr * sp * cy + cr * sy,
            -sr * sp * sy - cr * cy,
            -sr * cp,
        );
        // Source's AngleVectors "right" is actually left-handed relative to
        // forward×up in some docs; the SDK computes:
        //   right.x = (-1*sr*sp*cy + -1*cr*-sy)
        // which equals the formula above. Up:
        let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy + -sr * cy, cr * cp);
        (forward, right, up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_90_faces_plus_y() {
        let (f, _, _) = Angle::new(0.0, 90.0, 0.0).vectors();
        assert!((f.x).abs() < 1e-5);
        assert!((f.y - 1.0).abs() < 1e-5);
        assert!((f.z).abs() < 1e-5);
    }
}
