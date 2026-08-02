//! Unsorted leaf brush ranges — vbsp's Leaves sorts by cluster and breaks indices.

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

    let stride = match version {
        0 => 56,
        1 => 32,
        v => return Err(format!("unsupported leaf lump version {v}")),
    };
    if filelen % stride != 0 {
        return Err(format!(
            "leaf lump len {filelen} not divisible by stride {stride}"
        ));
    }
    if fileofs + filelen > bsp_bytes.len() {
        return Err("leaf lump out of range".into());
    }

    let lump = &bsp_bytes[fileofs..fileofs + filelen];
    let count = filelen / stride;
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
