//! Unsorted leaf brush ranges — vbsp's Leaves sorts by cluster and breaks indices.
//!
//! Leaf lump may be Source-LZMA compressed (fourCC / `ident` = uncompressed size).

use lzma_rs::decompress::{Options, UnpackedSize};
use std::io::Cursor;

#[derive(Clone, Copy, Debug)]
pub struct LeafBrushRange {
    pub first: u16,
    pub count: u16,
}

pub fn parse_leaf_brush_ranges(bsp_bytes: &[u8]) -> Result<Vec<LeafBrushRange>, String> {
    if bsp_bytes.len() < 8 + 64 * 16 + 4 {
        return Err("file too small for BSP header".into());
    }
    let ident = i32::from_le_bytes(bsp_bytes[0..4].try_into().unwrap());
    if ident != 0x5053_4256 {
        // 'VBSP' LE
        return Err(format!("bad ident {ident:#x}"));
    }

    // lump 10 = LEAFS: offset at 8 + 10*16
    let base = 8 + 10 * 16;
    let fileofs = i32::from_le_bytes(bsp_bytes[base..base + 4].try_into().unwrap()) as usize;
    let filelen = i32::from_le_bytes(bsp_bytes[base + 4..base + 8].try_into().unwrap()) as usize;
    let version = i32::from_le_bytes(bsp_bytes[base + 8..base + 12].try_into().unwrap());
    let fourcc = i32::from_le_bytes(bsp_bytes[base + 12..base + 16].try_into().unwrap());

    let stride = match version {
        0 => 56,
        1 => 32,
        v => return Err(format!("unsupported leaf lump version {v}")),
    };
    if fileofs + filelen > bsp_bytes.len() {
        return Err("leaf lump out of range".into());
    }

    let raw = &bsp_bytes[fileofs..fileofs + filelen];
    let owned;
    let lump: &[u8] = if fourcc != 0 || raw.starts_with(b"LZMA") {
        let expected = if fourcc != 0 {
            fourcc as usize
        } else if raw.len() >= 8 {
            u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize
        } else {
            return Err("truncated LZMA leaf lump".into());
        };
        owned = source_lzma_decompress(raw, expected)?;
        &owned
    } else {
        raw
    };

    if lump.len() % stride != 0 {
        return Err(format!(
            "leaf lump len {} not divisible by stride {stride}",
            lump.len()
        ));
    }

    let count = lump.len() / stride;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let leaf = &lump[i * stride..];
        // Layout common to v0/v1 up through leaf_brush_count:
        // contents i32, cluster i16, area i16, mins[3] i16, maxs[3] i16,
        // first_leaf_face u16, leaf_face_count u16,
        // first_leaf_brush u16, leaf_brush_count u16
        let first = u16::from_le_bytes(leaf[24..26].try_into().unwrap());
        let n = u16::from_le_bytes(leaf[26..28].try_into().unwrap());
        out.push(LeafBrushRange {
            first,
            count: n,
        });
    }
    Ok(out)
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
