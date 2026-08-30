//! Dump the loaded skybox atlas faces to PNGs.
//!
//!   cargo run -p surf-map --example dump_sky --release -- <map.bsp> <out_dir>

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = surf_map::LoadedMap::load_path(&args[0]).expect("load");
    let out = args.get(1).cloned().unwrap_or_else(|| ".".into());
    let sky = &map.skybox;
    println!(
        "skyname={:?} layer_size={} bytes={} empty={}",
        map.skyname,
        sky.layer_size,
        sky.rgba.len(),
        sky.is_empty()
    );
    println!(
        "materials: layers={} textured={} missing={}",
        map.materials.layer_count, map.materials.textured_count, map.materials.missing_count
    );
    for n in &map.materials.missing_names {
        println!("  MISS {n}");
    }
    if sky.is_empty() {
        return;
    }
    let s = sky.layer_size;
    let face = (s * s * 4) as usize;
    for (i, name) in ["ft", "bk", "lf", "rt", "up", "dn"].iter().enumerate() {
        let px = &sky.rgba[i * face..(i + 1) * face];
        // Flat-colour check: a fallback face is a single uniform RGBA.
        let first = &px[0..4];
        let flat = px.chunks_exact(4).all(|c| c == first);
        println!("  {name}: flat={flat} first={:?}", &first);
        image::RgbaImage::from_raw(s, s, px.to_vec())
            .unwrap()
            .save(format!("{out}/sky_{name}.png"))
            .unwrap();
    }
}
