//! Per-leaf ambient light cubes (`LUMP_LEAF_AMBIENT_LIGHTING` + index) — what
//! Source lights a static prop with when it has no baked per-vertex light.
//!
//! Each leaf owns a run of samples; each sample is six `ColorRGBExp32`
//! colours (+x −x +y −y +z −z) at a position given as a 0..255 fraction of
//! the leaf's box. A model is lit by the cube nearest its origin, weighted by
//! the squared components of the vertex normal (`AmbientLight` in Source's
//! studio shaders: a normal of `(0,0,1)` reads the +z face alone, a diagonal
//! mixes the faces it leans toward).
//!
//! **Scale.** The lightmap decode divides by 255 (`rgb * 2^exp / 255`); the
//! cube values are stored one power of ~2^8 smaller and are read here as
//! `rgb * 2^exp` — *no* `/255`. That is an empirical calibration, not a quote
//! from Valve code: on summit the lightmap's lit luxels sit at L 0.05–0.9,
//! the props' own baked vertex light (`.vhv`) at 0.02–0.75, and the cubes
//! read this way at 0.03–0.12, which is where an ambient term belongs. With
//! the `/255` every ambient-lit prop is black.

use crate::leaves;

const LUMP_LEAVES: usize = 10;
const LUMP_LEAF_AMBIENT_INDEX_HDR: usize = 51;
const LUMP_LEAF_AMBIENT_INDEX: usize = 52;
const LUMP_LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
const LUMP_LEAF_AMBIENT_LIGHTING: usize = 56;
const SAMPLE_SIZE: usize = 28;

/// One ambient cube: linear RGB for +x, −x, +y, −y, +z, −z.
pub type Cube = [[f32; 3]; 6];

#[derive(Clone, Debug)]
struct Sample {
    cube: Cube,
    /// Fraction of the leaf box, 0..1.
    pos: [f32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct AmbientCubes {
    /// Per leaf: (first sample, count).
    index: Vec<(u32, u32)>,
    samples: Vec<Sample>,
    /// Per leaf: (mins, maxs) from the leaf lump.
    leaf_bounds: Vec<([f32; 3], [f32; 3])>,
}

impl AmbientCubes {
    /// LDR lumps when the map has them (they pair with the LDR lightmap the
    /// atlas uses), else HDR. `None` when the map carries no ambient data.
    pub fn parse(bsp_bytes: &[u8]) -> Option<Self> {
        let pairs = [
            (LUMP_LEAF_AMBIENT_INDEX, LUMP_LEAF_AMBIENT_LIGHTING),
            (LUMP_LEAF_AMBIENT_INDEX_HDR, LUMP_LEAF_AMBIENT_LIGHTING_HDR),
        ];
        let (index_raw, samples_raw) = pairs.iter().find_map(|&(i, l)| {
            let (idx, _) = leaves::read_lump(bsp_bytes, i).ok()?;
            let (smp, _) = leaves::read_lump(bsp_bytes, l).ok()?;
            (idx.len() >= 4 && smp.len() >= SAMPLE_SIZE).then_some((idx, smp))
        })?;
        let (leaf_raw, leaf_version) = leaves::read_lump(bsp_bytes, LUMP_LEAVES).ok()?;
        let stride = match leaf_version {
            0 => 56,
            _ => 32,
        };
        let leaf_bounds: Vec<_> = leaf_raw
            .chunks_exact(stride)
            .map(|l| {
                let i16_at = |o: usize| i16::from_le_bytes([l[o], l[o + 1]]) as f32;
                (
                    [i16_at(8), i16_at(10), i16_at(12)],
                    [i16_at(14), i16_at(16), i16_at(18)],
                )
            })
            .collect();
        let samples: Vec<Sample> = samples_raw
            .chunks_exact(SAMPLE_SIZE)
            .map(|s| {
                let mut cube = [[0.0f32; 3]; 6];
                for (f, face) in cube.iter_mut().enumerate() {
                    let o = f * 4;
                    let scale = 2f32.powi(s[o + 3] as i8 as i32);
                    *face = [s[o] as f32 * scale, s[o + 1] as f32 * scale, s[o + 2] as f32 * scale];
                }
                Sample {
                    cube,
                    pos: [s[24] as f32 / 255.0, s[25] as f32 / 255.0, s[26] as f32 / 255.0],
                }
            })
            .collect();
        let index: Vec<(u32, u32)> = index_raw
            .chunks_exact(4)
            .map(|e| {
                let count = u16::from_le_bytes([e[0], e[1]]) as u32;
                let first = u16::from_le_bytes([e[2], e[3]]) as u32;
                (first, count)
            })
            .collect();
        if index.len() != leaf_bounds.len() {
            return None;
        }
        Some(Self {
            index,
            samples,
            leaf_bounds,
        })
    }

    /// Mean of every sample in the map — the fallback for a prop buried so
    /// deep in solid that no probe above it reaches a lit leaf.
    pub fn average(&self) -> Option<Cube> {
        let lit: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.cube.iter().flatten().any(|v| *v > 0.0))
            .collect();
        if lit.is_empty() {
            return None;
        }
        let n = lit.len() as f32;
        let mut out: Cube = [[0.0; 3]; 6];
        for s in lit {
            for f in 0..6 {
                for c in 0..3 {
                    out[f][c] += s.cube[f][c] / n;
                }
            }
        }
        Some(out)
    }

    /// The cube nearest `point` in the leaf that contains it. Falls back to
    /// the first sample of the leaf when positions are degenerate, and to
    /// `None` when the leaf has no samples (solid leaves).
    pub fn cube_at(&self, bsp_bytes: &[u8], point: [f32; 3]) -> Option<Cube> {
        let leaf = leaves::leaf_index_at(bsp_bytes, point).ok()?;
        let &(first, count) = self.index.get(leaf)?;
        if count == 0 {
            return None;
        }
        let (mins, maxs) = self.leaf_bounds[leaf];
        let frac = |i: usize| {
            let span = maxs[i] - mins[i];
            if span <= 0.0 {
                0.5
            } else {
                ((point[i] - mins[i]) / span).clamp(0.0, 1.0)
            }
        };
        let p = [frac(0), frac(1), frac(2)];
        let run = self
            .samples
            .get(first as usize..(first + count) as usize)?;
        run.iter()
            .min_by(|a, b| {
                let d = |s: &Sample| {
                    (s.pos[0] - p[0]).powi(2) + (s.pos[1] - p[1]).powi(2) + (s.pos[2] - p[2]).powi(2)
                };
                d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|s| s.cube)
    }
}

/// Light arriving on a surface with unit normal `n` from `cube`.
pub fn eval(cube: &Cube, n: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for axis in 0..3 {
        let w = n[axis] * n[axis];
        let face = axis * 2 + usize::from(n[axis] < 0.0);
        for c in 0..3 {
            out[c] += cube[face][c] * w;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_reads_the_face_a_normal_points_at_and_mixes_diagonals() {
        let mut cube: Cube = [[0.0; 3]; 6];
        cube[4] = [1.0, 1.0, 1.0]; // +z
        cube[1] = [0.5, 0.0, 0.0]; // −x
        assert_eq!(eval(&cube, [0.0, 0.0, 1.0]), [1.0, 1.0, 1.0]);
        assert_eq!(eval(&cube, [0.0, 0.0, -1.0]), [0.0, 0.0, 0.0]);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let d = eval(&cube, [-s, 0.0, s]);
        assert!((d[0] - 0.75).abs() < 1e-5 && (d[1] - 0.5).abs() < 1e-5);
    }
}
