//! Dump the loaded skybox faces and albedo-atlas layers to PNGs, and report
//! which materials resolved to a real VTF vs a placeholder.
//!
//!   cargo run -p surf-map --example dump_materials --release -- <map.bsp> [out_dir]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: dump_materials <map.bsp> [out_dir]");
        std::process::exit(2);
    }
    let map = surf_map::LoadedMap::load_path(&args[0]).expect("load map");
    let out = args.get(1).cloned().unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&out);

    let sky = &map.skybox;
    println!(
        "skyname={:?} sky_layer={} sky_empty={}",
        map.skyname,
        sky.layer_size,
        sky.is_empty()
    );
    if !sky.is_empty() {
        let s = sky.layer_size;
        let face = (s * s * 4) as usize;
        for (i, name) in ["ft", "bk", "lf", "rt", "up", "dn"].iter().enumerate() {
            let px = &sky.rgba[i * face..(i + 1) * face];
            image::RgbaImage::from_raw(s, s, px.to_vec())
                .unwrap()
                .save(format!("{out}/sky_{name}.png"))
                .unwrap();
        }
    }

    let lm = &map.lightmaps;
    println!(
        "lightmap: {}x{} faces={} stub={}",
        lm.width,
        lm.height,
        lm.face_count,
        lm.is_stub()
    );
    if !lm.is_stub() {
        // How colourful is the bake? Saturation = (max-min)/max per texel.
        let mut n = 0u64;
        let mut satsum = 0.0f64;
        let mut lumsum = 0.0f64;
        let mut sat_over_10 = 0u64;
        for c in lm.rgba.chunks_exact(4) {
            let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
            satsum += sat as f64;
            lumsum += ((r + g + b) / 3.0) as f64;
            if sat > 0.10 {
                sat_over_10 += 1;
            }
            n += 1;
        }
        println!(
            "  mean sat {:.3}  mean luma {:.1}  texels with sat>0.10: {:.1}%",
            satsum / n as f64,
            lumsum / n as f64,
            100.0 * sat_over_10 as f64 / n as f64
        );
        image::RgbaImage::from_raw(lm.width, lm.height, lm.rgba.clone())
            .unwrap()
            .save(format!("{out}/lightmap.png"))
            .unwrap();
    }

    let m = &map.materials;
    println!(
        "materials: layers={} textured={} missing={}",
        m.layer_count, m.textured_count, m.missing_count
    );
    for n in &m.missing_names {
        println!("  MISS {n}");
    }
    for (name, id) in &m.layer_of {
        println!("  layer {id:3} <- {name}");
    }
    let s = m.layer_size;
    let layer = (s * s * 4) as usize;
    for i in 0..m.layer_count as usize {
        let px = &m.rgba[i * layer..(i + 1) * layer];
        let first = &px[0..4];
        let flat = px.chunks_exact(4).all(|c| c == first);
        println!("  layer {i:3}: flat={flat} rgba={first:?}");
        image::RgbaImage::from_raw(s, s, px.to_vec())
            .unwrap()
            .save(format!("{out}/mat_{i:03}.png"))
            .unwrap();
    }
}

// appended: lightmap dump lives here too so one command answers "what colour is
// the light, and what colour is the albedo?"
