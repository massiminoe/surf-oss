//! First-person camera. Source Z-up → wgpu Y-up at the view-matrix boundary.

use surf_core::math::{Angle, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
}

pub struct Camera {
    pub eye: Vec3,
    pub angles: Angle,
    pub aspect: f32,
    pub fov_y_deg: f32,
    pub z_near: f32,
    pub z_far: f32,
}

impl Camera {
    pub fn new(eye: Vec3, angles: Angle, aspect: f32) -> Self {
        Self {
            eye,
            angles,
            aspect,
            fov_y_deg: 90.0,
            z_near: 1.0,
            // Surf maps span ±16k+ units; graybox is tiny so this is fine for both.
            z_far: 100_000.0,
        }
    }

    pub fn uniform(&self) -> CameraUniform {
        CameraUniform {
            view_proj: view_proj(self, false),
        }
    }

    /// Rotation-only view (eye at origin) for the skybox cube.
    pub fn sky_uniform(&self) -> CameraUniform {
        CameraUniform {
            view_proj: view_proj(self, true),
        }
    }
}

/// Source (x,y,z Z-up) → view space (x,y,z Y-up): (x, z, -y) then look along +Z in clip.
fn source_to_yup(v: Vec3) -> [f32; 3] {
    [v.x, v.z, -v.y]
}

fn view_proj(cam: &Camera, sky: bool) -> [[f32; 4]; 4] {
    let (forward, _right, _up) = cam.angles.vectors();
    let eye = if sky {
        [0.0, 0.0, 0.0]
    } else {
        source_to_yup(cam.eye)
    };
    let target = if sky {
        source_to_yup(forward)
    } else {
        source_to_yup(cam.eye + forward)
    };
    let up = source_to_yup(Vec3::Z); // Source up → Y-up space

    let view = look_at_rh(eye, target, up);
    // Reversed-Z: near maps to depth 1, far to depth 0. Float32 has its finest
    // spacing near 0, which is where the *far* geometry now lands, so precision
    // becomes near-uniform across the range instead of collapsing with distance.
    // With the old forward mapping and z_near=1 / z_far=100000, one float step
    // was ~5.8 world units at 10k out and ~26 at 20k — enough for boreas' props
    // to punch through terrain. Depth compare is Greater and the buffer clears
    // to 0.0 to match (see pipeline.rs / skybox.rs / ghost.rs / trail.rs).
    let proj = perspective_rh(
        cam.fov_y_deg.to_radians(),
        cam.aspect,
        cam.z_far,
        cam.z_near,
    );
    mat4_mul(proj, view)
}

fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

fn perspective_rh(fovy: f32, aspect: f32, znear: f32, zfar: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fovy * 0.5).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = zfar / (znear - zfar);
    m[2][3] = -1.0;
    m[3][2] = (zfar * znear) / (znear - zfar);
    m
}

fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = a[0][j] * b[i][0] + a[1][j] * b[i][1] + a[2][j] * b[i][2] + a[3][j] * b[i][3];
        }
    }
    r
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}
