//! Unsorted leaf brush ranges — vbsp's Leaves sorts by cluster and breaks indices.
//!
//! Leaf lump may be Source-LZMA compressed (fourCC / `ident` = uncompressed size).

use lzma_rs::decompress::{Options, UnpackedSize};

const LUMP_PLANES: usize = 1;
const LUMP_NODES: usize = 5;
const LUMP_LEAFS: usize = 10;
/// dplane_t: normal[3] f32, dist f32, type i32.
const PLANE_SIZE: usize = 20;
/// dnode_t: planenum i32, children[2] i32, mins[3] i16, maxs[3] i16,
/// firstface u16, numfaces u16, area i16, padding i16.
const NODE_SIZE: usize = 32;
use std::io::Cursor;

#[derive(Clone, Copy, Debug)]
pub struct LeafBrushRange {
    pub first: u16,
    pub count: u16,
}

/// Reads one lump, transparently decompressing Source's per-lump LZMA.
/// Returns the bytes and the lump's version.
fn read_lump(bsp_bytes: &[u8], index: usize) -> Result<(Vec<u8>, i32), String> {
    if bsp_bytes.len() < 8 + 64 * 16 + 4 {
        return Err("file too small for BSP header".into());
    }
    let ident = i32::from_le_bytes(bsp_bytes[0..4].try_into().unwrap());
    if ident != 0x5053_4256 {
        // 'VBSP' LE
        return Err(format!("bad ident {ident:#x}"));
    }
    let base = 8 + index * 16;
    let fileofs = i32::from_le_bytes(bsp_bytes[base..base + 4].try_into().unwrap()) as usize;
    let filelen = i32::from_le_bytes(bsp_bytes[base + 4..base + 8].try_into().unwrap()) as usize;
    let version = i32::from_le_bytes(bsp_bytes[base + 8..base + 12].try_into().unwrap());
    let fourcc = i32::from_le_bytes(bsp_bytes[base + 12..base + 16].try_into().unwrap());
    if fileofs + filelen > bsp_bytes.len() {
        return Err(format!("lump {index} out of range"));
    }
    let raw = &bsp_bytes[fileofs..fileofs + filelen];
    if fourcc != 0 || raw.starts_with(b"LZMA") {
        let expected = if fourcc != 0 {
            fourcc as usize
        } else if raw.len() >= 8 {
            u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize
        } else {
            return Err(format!("truncated LZMA lump {index}"));
        };
        Ok((source_lzma_decompress(raw, expected)?, version))
    } else {
        Ok((raw.to_vec(), version))
    }
}

fn leaf_stride(version: i32) -> Result<usize, String> {
    match version {
        0 => Ok(56),
        1 => Ok(32),
        v => Err(format!("unsupported leaf lump version {v}")),
    }
}

pub fn parse_leaf_brush_ranges(bsp_bytes: &[u8]) -> Result<Vec<LeafBrushRange>, String> {
    let (lump, version) = read_lump(bsp_bytes, LUMP_LEAFS)?;
    let stride = leaf_stride(version)?;
    if lump.len() % stride != 0 {
        return Err(format!(
            "leaf lump len {} not divisible by stride {stride}",
            lump.len()
        ));
    }
    let mut out = Vec::with_capacity(lump.len() / stride);
    for leaf in lump.chunks_exact(stride) {
        // Layout common to v0/v1 up through leaf_brush_count:
        // contents i32, cluster i16, area i16, mins[3] i16, maxs[3] i16,
        // first_leaf_face u16, leaf_face_count u16,
        // first_leaf_brush u16, leaf_brush_count u16
        out.push(LeafBrushRange {
            first: u16::from_le_bytes(leaf[24..26].try_into().unwrap()),
            count: u16::from_le_bytes(leaf[26..28].try_into().unwrap()),
        });
    }
    Ok(out)
}

/// Per-leaf BSP *area*. The compiler puts the 3D skybox in its own area, sealed
/// off from the playable map, so this is the map's own statement about which
/// geometry is scenery — not a guess based on how far away something is.
///
/// Packed as `area_and_flags`: low 9 bits area, high 7 bits flags.
pub fn parse_leaf_areas(bsp_bytes: &[u8]) -> Result<Vec<u16>, String> {
    let (lump, version) = read_lump(bsp_bytes, LUMP_LEAFS)?;
    let stride = leaf_stride(version)?;
    if lump.len() % stride != 0 {
        return Err("leaf lump not divisible by stride".into());
    }
    Ok(lump
        .chunks_exact(stride)
        .map(|leaf| i16::from_le_bytes(leaf[6..8].try_into().unwrap()) as u16 & 0x1ff)
        .collect())
}

/// Descends the BSP node tree to the leaf containing `point`.
///
/// Done here rather than through vbsp because `vbsp::Leaves` sorts leaves by
/// cluster, which invalidates every index that points at them — the same trap
/// this module already exists to work around for leaf brushes.
pub fn leaf_index_at(bsp_bytes: &[u8], point: [f32; 3]) -> Result<usize, String> {
    let (planes, _) = read_lump(bsp_bytes, LUMP_PLANES)?;
    let (nodes, _) = read_lump(bsp_bytes, LUMP_NODES)?;
    if planes.len() % PLANE_SIZE != 0 || nodes.len() % NODE_SIZE != 0 {
        return Err("plane/node lump size mismatch".into());
    }
    let node_count = nodes.len() / NODE_SIZE;
    if node_count == 0 {
        return Err("no BSP nodes".into());
    }

    let mut index: i32 = 0;
    // The tree is at most as deep as it has nodes; the bound stops a malformed
    // or cyclic lump from hanging the loader.
    for _ in 0..node_count + 1 {
        if index < 0 {
            return Ok((-1 - index) as usize);
        }
        let n = index as usize;
        if n >= node_count {
            return Err("node index out of range".into());
        }
        let node = &nodes[n * NODE_SIZE..];
        let plane_num = i32::from_le_bytes(node[0..4].try_into().unwrap()) as usize;
        if (plane_num + 1) * PLANE_SIZE > planes.len() {
            return Err("plane index out of range".into());
        }
        let pl = &planes[plane_num * PLANE_SIZE..];
        let normal = [
            f32::from_le_bytes(pl[0..4].try_into().unwrap()),
            f32::from_le_bytes(pl[4..8].try_into().unwrap()),
            f32::from_le_bytes(pl[8..12].try_into().unwrap()),
        ];
        let dist = f32::from_le_bytes(pl[12..16].try_into().unwrap());
        let d = normal[0] * point[0] + normal[1] * point[1] + normal[2] * point[2] - dist;
        let child = if d >= 0.0 { 4 } else { 8 };
        index = i32::from_le_bytes(node[child..child + 4].try_into().unwrap());
    }
    Err("BSP node descent did not terminate".into())
}

/// Source engine per-lump LZMA: `LZMA` + actual_size u32 + lzma_size u32 + props+payload.
fn source_lzma_decompress(data: &[u8], expected_length: usize) -> Result<Vec<u8>, String> {
    if data.len() < 17 {
        return Err("LZMA leaf lump too short".into());
    }
    if &data[0..4] != b"LZMA" {
        return Err("leaf lump marked compressed but missing LZMA magic".into());
    }
    let actual_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let lzma_size = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    if data.len() < lzma_size + 12 {
        return Err(format!(
            "LZMA leaf truncated: got {} need {}",
            data.len(),
            lzma_size + 12
        ));
    }
    if actual_size != expected_length {
        return Err(format!(
            "LZMA leaf size mismatch: header {actual_size} vs fourCC {expected_length}"
        ));
    }

    let mut output = Vec::with_capacity(expected_length);
    let mut cursor = Cursor::new(&data[12..]);
    lzma_rs::lzma_decompress_with_options(
        &mut cursor,
        &mut output,
        &Options {
            unpacked_size: UnpackedSize::UseProvided(Some(actual_size as u64)),
            allow_incomplete: false,
            memlimit: None,
        },
    )
    .map_err(|e| format!("LZMA leaf decompress: {e}"))?;
    if output.len() != expected_length {
        return Err(format!(
            "LZMA leaf unpacked {} bytes, expected {expected_length}",
            output.len()
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn lzma_leaf_maps_parse() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps");
        for name in ["surf_boreas.bsp", "surf_tendies.bsp", "surf_lovetunnel.bsp"] {
            let path = root.join(name);
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let ranges = parse_leaf_brush_ranges(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!ranges.is_empty(), "{name} produced 0 leaves");
        }
    }
}
