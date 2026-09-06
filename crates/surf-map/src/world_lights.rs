//! `LUMP_WORLDLIGHTS` — the lights vrad compiled from the map's entities, in
//! the same linear units the lightmap decodes to (summit's `light_environment
//! 251 205 121 250` is stored as `(0.95, 0.61, 0.19)`, exactly
//! `(rgb/255)^2.2 * 250/255`). Source lights a static prop that has no baked
//! per-vertex light from these: the sun as an unshadowed directional term,
//! and the nearest few point / spot / surface lights with their compiled
//! attenuation — the classic "props don't receive shadows" look of maps
//! built without `-StaticPropLighting`.

use surf_core::math::Vec3;

use crate::leaves;

const LUMP_WORLDLIGHTS: usize = 15;
const LUMP_WORLDLIGHTS_HDR: usize = 54;
const RECORD_SIZE: usize = 88;

const EMIT_SURFACE: i32 = 0;
const EMIT_POINT: i32 = 1;
const EMIT_SPOTLIGHT: i32 = 2;
const EMIT_SKYLIGHT: i32 = 3;
const EMIT_SKYAMBIENT: i32 = 5;

/// How many local lights a prop takes, brightest at its origin first.
pub const MAX_LOCAL_LIGHTS: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct WorldLight {
    pub origin: Vec3,
    pub intensity: [f32; 3],
    pub normal: Vec3,
    pub kind: i32,
    pub stopdot: f32,
    pub stopdot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant: f32,
    pub linear: f32,
    pub quadratic: f32,
}

#[derive(Clone, Debug, Default)]
pub struct WorldLights {
    pub lights: Vec<WorldLight>,
}

/// The sun, from the lump's `emit_skylight` record.
#[derive(Clone, Copy, Debug)]
pub struct Sun {
    /// Unit vector from a surface toward the sun.
    pub to_sun: Vec3,
    /// Linear RGB at normal incidence.
    pub color: [f32; 3],
    /// Luma of the `emit_skyambient` record, for the visibility gate.
    pub sky_luma: f32,
}

impl Sun {
    /// Whether a prop lit by `cube` is in the open. The engine occludes the
    /// sun on a model with a trace; we have no sky-brush test, so ask the
    /// cube: a sample that sees sky in the sun's direction reads about the
    /// skylight level on that face (summit's upward faces sit at exactly
    /// `_ambient`), one in a cave or under a deck reads far below it.
    pub fn visible_from(&self, cube: &crate::ambient::Cube) -> bool {
        let toward = crate::ambient::eval(cube, [self.to_sun.x, self.to_sun.y, self.to_sun.z]);
        self.sky_luma <= 0.0 || luma(toward) >= 0.3 * self.sky_luma
    }
}

fn luma(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.6 * c[1] + 0.1 * c[2]
}

impl WorldLights {
    /// LDR lump when present, else HDR.
    pub fn parse(bsp_bytes: &[u8]) -> Self {
        let raw = [LUMP_WORLDLIGHTS, LUMP_WORLDLIGHTS_HDR]
            .iter()
            .find_map(|&l| {
                leaves::read_lump(bsp_bytes, l)
                    .ok()
                    .filter(|(d, _)| d.len() >= RECORD_SIZE)
                    .map(|(d, _)| d)
            })
            .unwrap_or_default();
        let f = |r: &[u8], o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        let i = |r: &[u8], o: usize| i32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        let lights = raw
            .chunks_exact(RECORD_SIZE)
            .map(|r| WorldLight {
                origin: Vec3::new(f(r, 0), f(r, 4), f(r, 8)),
                intensity: [f(r, 12), f(r, 16), f(r, 20)],
                normal: Vec3::new(f(r, 24), f(r, 28), f(r, 32)),
                kind: i(r, 40),
                stopdot: f(r, 48),
                stopdot2: f(r, 52),
                exponent: f(r, 56),
                radius: f(r, 60),
                constant: f(r, 64),
                linear: f(r, 68),
                quadratic: f(r, 72),
            })
            .filter(|l| l.intensity.iter().all(|c| c.is_finite()))
            .collect();
        Self { lights }
    }

    pub fn sun(&self) -> Option<Sun> {
        let sky = self.lights.iter().find(|l| l.kind == EMIT_SKYLIGHT)?;
        let ambient = self
            .lights
            .iter()
            .find(|l| l.kind == EMIT_SKYAMBIENT)
            .map(|l| luma(l.intensity))
            .unwrap_or(0.0);
        let travel = sky.normal;
        if travel.length() < 1e-3 {
            return None;
        }
        Some(Sun {
            to_sun: travel * (-1.0 / travel.length()),
            color: sky.intensity,
            sky_luma: ambient,
        })
    }

    /// The local (non-sky) lights that matter most at `point`: the
    /// `MAX_LOCAL_LIGHTS` with the largest irradiance there, ignoring
    /// anything below a visible threshold.
    pub fn local_lights_at(&self, point: Vec3) -> Vec<&WorldLight> {
        let mut scored: Vec<(f32, &WorldLight)> = self
            .lights
            .iter()
            .filter(|l| matches!(l.kind, EMIT_SURFACE | EMIT_POINT | EMIT_SPOTLIGHT))
            .filter_map(|l| {
                let s = luma(l.irradiance_at(point));
                (s > 0.002).then_some((s, l))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(MAX_LOCAL_LIGHTS);
        scored.into_iter().map(|(_, l)| l).collect()
    }
}

impl WorldLight {
    /// Light arriving at `point` on a surface facing the light squarely —
    /// distance falloff and the spot / surface cones, no Lambert term.
    pub fn irradiance_at(&self, point: Vec3) -> [f32; 3] {
        let delta = self.origin - point;
        let dist = delta.length().max(1.0);
        if self.radius > 0.0 && dist > self.radius {
            return [0.0; 3];
        }
        let dir_to_light = delta * (1.0 / dist);
        let denom = self.constant + self.linear * dist + self.quadratic * dist * dist;
        let mut scale = if denom > 1e-6 { 1.0 / denom } else { 1.0 };
        match self.kind {
            EMIT_SPOTLIGHT => {
                // Cone measured from the light's axis toward the point.
                let dot = -(dir_to_light.dot(self.normal));
                if dot < self.stopdot2 {
                    return [0.0; 3];
                }
                if dot < self.stopdot {
                    let t = (dot - self.stopdot2) / (self.stopdot - self.stopdot2).max(1e-6);
                    scale *= t.powf(self.exponent.max(1e-3));
                }
            }
            EMIT_SURFACE => {
                // A texlight emits over its own hemisphere, cosine-weighted.
                let dot = -(dir_to_light.dot(self.normal));
                if dot <= 0.0 {
                    return [0.0; 3];
                }
                scale *= dot;
            }
            _ => {}
        }
        [
            self.intensity[0] * scale,
            self.intensity[1] * scale,
            self.intensity[2] * scale,
        ]
    }

    /// Unit vector from `point` toward the light.
    pub fn direction_from(&self, point: Vec3) -> Vec3 {
        let delta = self.origin - point;
        let len = delta.length();
        if len < 1e-3 {
            Vec3::new(0.0, 0.0, 1.0)
        } else {
            delta * (1.0 / len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_light() -> WorldLight {
        WorldLight {
            origin: Vec3::new(0.0, 0.0, 100.0),
            intensity: [10000.0, 5000.0, 2500.0],
            normal: Vec3::new(0.0, 0.0, -1.0),
            kind: EMIT_POINT,
            stopdot: 0.0,
            stopdot2: 0.0,
            exponent: 0.0,
            radius: 0.0,
            constant: 0.0,
            linear: 0.0,
            quadratic: 1.0,
        }
    }

    #[test]
    fn point_light_falls_off_with_the_compiled_attenuation() {
        let l = point_light();
        let at = l.irradiance_at(Vec3::ZERO);
        assert!((at[0] - 1.0).abs() < 1e-4, "{at:?}");
        let far = l.irradiance_at(Vec3::new(0.0, 0.0, -100.0));
        assert!((far[0] - 0.25).abs() < 1e-4, "{far:?}");
    }

    #[test]
    fn spotlight_is_dark_outside_its_cone() {
        let mut l = point_light();
        l.kind = EMIT_SPOTLIGHT;
        l.stopdot = 0.9;
        l.stopdot2 = 0.8;
        l.exponent = 1.0;
        assert!(l.irradiance_at(Vec3::ZERO)[0] > 0.9);
        assert_eq!(l.irradiance_at(Vec3::new(100.0, 0.0, 100.0)), [0.0; 3]);
    }

    #[test]
    fn brightest_lights_win_and_the_list_is_capped() {
        let mut lights = Vec::new();
        for k in 0..8 {
            let mut l = point_light();
            l.origin = Vec3::new(0.0, 0.0, 50.0 + 50.0 * k as f32);
            lights.push(l);
        }
        let wl = WorldLights { lights };
        let picked = wl.local_lights_at(Vec3::ZERO);
        assert_eq!(picked.len(), MAX_LOCAL_LIGHTS);
        assert!((picked[0].origin.z - 50.0).abs() < 1e-3);
    }
}
