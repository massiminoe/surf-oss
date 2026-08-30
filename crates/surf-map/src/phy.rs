//! `.phy` — the collision hull Source actually uses for a prop.
//!
//! We previously used a prop's *render* mesh as its collision, which surfs
//! badly: a render mesh carries every decorative fold the artist put on the
//! surface, and near-perpendicular neighbouring faces form creases that wedge
//! the player. boreas' `ramp_c1` is 684 render triangles but only **74**
//! collision triangles, and at 3606 u/s the difference was a dead stop
//! (velocity·normal was −3.9 — a pure graze — yet `TryPlayerMove` took the
//! two-plane crease branch and projected 3606 u/s down to 79).
//!
//! Format (Havok/IVP "compact surface", documented publicly on VDC — this is a
//! file layout, not Valve code):
//!
//! ```text
//! phyheader   { i32 size; i32 id; i32 solid_count; i32 checksum }
//! per solid   { i32 block_size; "VPHY"; i16 ver; i16 model_type;
//!               i32 surface_size; f32 drag[3]; i32 axis_map_size }   (32 B)
//!   surface   { f32 mass_center[3]; f32 rot_inertia[3]; f32 radius;
//!               u32 (max_dev:8 | byte_size:24); i32 root; i32 dummy[3] }
//!             dummy[2] spells "IVPS"; `root` is relative to the surface start.
//!   node      { i32 offset_right; i32 offset_ledge; f32 c[3]; f32 r;
//!               u8 box[3]; u8 free }                                 (28 B)
//!             offset_right == 0 marks a leaf (its ledge is at offset_ledge);
//!             otherwise the left child sits immediately after the node and the
//!             right child at offset_right. Interior nodes also carry a ledge
//!             offset, so that field cannot be used to detect a leaf.
//!   ledge     { i32 point_offset; i32 _; u32 (flags:8 | size_div_16:24);
//!               i16 n_tri; i16 _ }                                   (16 B)
//!   triangle  { u32 packed; edge[3] }   edge low 16 bits = vertex index (16 B)
//!   vertex    f32[4], IVP metres, at ledge + point_offset. Shared between
//!             ledges, so it is NOT bounded by the ledge's own size.
//! ```
//!
//! IVP is metres and Y-down: `source = (x, z, -y) / 0.0254`.

use surf_core::math::Vec3;

/// IVP metres → Source units (1 unit = 1 inch).
const IVP_TO_SOURCE: f32 = 1.0 / 0.0254;

const NODE_SIZE: i64 = 28;
const LEDGE_HEADER_SIZE: i64 = 16;
const TRI_SIZE: i64 = 16;
const VERT_SIZE: i64 = 16;

/// Refuse absurd files rather than allocating on garbage.
const MAX_TRIS: usize = 200_000;
const MAX_NODES: usize = 100_000;

fn i32_at(d: &[u8], off: i64) -> Option<i32> {
    let off = usize::try_from(off).ok()?;
    let b = d.get(off..off + 4)?;
    Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u32_at(d: &[u8], off: i64) -> Option<u32> {
    i32_at(d, off).map(|v| v as u32)
}

fn i16_at(d: &[u8], off: i64) -> Option<i16> {
    let off = usize::try_from(off).ok()?;
    let b = d.get(off..off + 2)?;
    Some(i16::from_le_bytes([b[0], b[1]]))
}

fn f32_at(d: &[u8], off: i64) -> Option<f32> {
    i32_at(d, off).map(f32::from_bits_preserve)
}

trait FromBitsPreserve {
    fn from_bits_preserve(v: i32) -> f32;
}
impl FromBitsPreserve for f32 {
    fn from_bits_preserve(v: i32) -> f32 {
        f32::from_bits(v as u32)
    }
}

/// Decode every collision triangle in a `.phy`, in Source model space.
///
/// Returns `None` when the file is not a usable compact surface; callers should
/// fall back to their previous source of collision rather than treating a prop
/// as non-solid.
pub fn decode_collision(data: &[u8]) -> Option<Vec<[Vec3; 3]>> {
    let header_size = i32_at(data, 0)? as i64;
    let solid_count = i32_at(data, 8)?;
    if header_size <= 0 || !(0..=4096).contains(&solid_count) {
        return None;
    }

    let mut out: Vec<[Vec3; 3]> = Vec::new();
    let mut off = header_size;

    for _ in 0..solid_count {
        let block_size = i32_at(data, off)? as i64;
        if block_size <= 0 {
            break;
        }
        if data.get(usize::try_from(off + 4).ok()?..usize::try_from(off + 8).ok()?)? != b"VPHY" {
            break;
        }
        let surface = off + 32;
        // dummy[2] == "IVPS" confirms we are aligned on the surface header.
        let ivps =
            data.get(usize::try_from(surface + 44).ok()?..usize::try_from(surface + 48).ok()?)?;
        if ivps != b"IVPS" {
            break;
        }
        let byte_size = (u32_at(data, surface + 28)? >> 8) as i64;
        let surface_end = surface + byte_size.max(0);
        let root = i32_at(data, surface + 32)? as i64;

        collect_ledges(data, surface + root, surface, surface_end, &mut out)?;

        off += block_size + 4;
    }

    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// Walk the ledge tree iteratively and emit each leaf ledge's triangles.
fn collect_ledges(
    data: &[u8],
    root: i64,
    surface: i64,
    surface_end: i64,
    out: &mut Vec<[Vec3; 3]>,
) -> Option<()> {
    let mut stack = vec![root];
    let mut visited = 0usize;

    while let Some(addr) = stack.pop() {
        visited += 1;
        if visited > MAX_NODES {
            return None;
        }
        if addr < surface || addr >= surface_end {
            continue;
        }
        let offset_right = i32_at(data, addr)? as i64;
        let offset_ledge = i32_at(data, addr + 4)? as i64;

        // A node is a LEAF when it has no right child. `offset_ledge` alone does
        // not decide it: interior nodes carry a ledge offset too, and treating
        // that as a leaf collapses the whole tree to a single ledge (ramp_c1
        // decoded 1 ledge / 74 tris instead of 16 / 128).
        if offset_right == 0 {
            if offset_ledge != 0 {
                read_ledge(data, addr + offset_ledge, surface, surface_end, out)?;
            }
            continue;
        }
        // Interior node: left child immediately follows, right child at offset.
        stack.push(addr + NODE_SIZE);
        stack.push(addr + offset_right);
    }
    Some(())
}

fn read_ledge(
    data: &[u8],
    ledge: i64,
    surface: i64,
    surface_end: i64,
    out: &mut Vec<[Vec3; 3]>,
) -> Option<()> {
    if ledge < surface || ledge >= surface_end {
        return Some(());
    }
    let point_offset = i32_at(data, ledge)? as i64;
    let n_tri = i16_at(data, ledge + 12)? as i64;
    if n_tri <= 0 {
        return Some(());
    }
    let verts = ledge + point_offset;
    if verts < surface || verts >= surface_end {
        return Some(());
    }
    // Vertices are shared between ledges, so they are bounded by the surface,
    // not by this ledge's own size_div_16.
    let max_vert = (surface_end - verts) / VERT_SIZE;

    for t in 0..n_tri {
        if out.len() >= MAX_TRIS {
            return None;
        }
        let tri = ledge + LEDGE_HEADER_SIZE + t * TRI_SIZE;
        let mut pts = [Vec3::ZERO; 3];
        let mut ok = true;
        for (e, pt) in pts.iter_mut().enumerate() {
            let edge = u32_at(data, tri + 4 + e as i64 * 4)?;
            let idx = (edge & 0xFFFF) as i64;
            if idx >= max_vert {
                ok = false;
                break;
            }
            let v = verts + idx * VERT_SIZE;
            let (x, y, z) = (f32_at(data, v)?, f32_at(data, v + 4)?, f32_at(data, v + 8)?);
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                ok = false;
                break;
            }
            *pt = Vec3::new(x * IVP_TO_SOURCE, z * IVP_TO_SOURCE, -y * IVP_TO_SOURCE);
        }
        if ok {
            out.push(pts);
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(decode_collision(&[]).is_none());
        assert!(decode_collision(&[0u8; 16]).is_none());
        assert!(decode_collision(&[0xFFu8; 64]).is_none());
        // Plausible header, no VPHY block behind it.
        let mut d = vec![0u8; 128];
        d[0..4].copy_from_slice(&16i32.to_le_bytes());
        d[8..12].copy_from_slice(&1i32.to_le_bytes());
        assert!(decode_collision(&d).is_none());
    }
}
