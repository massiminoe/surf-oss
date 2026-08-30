//! Extract a file from a map's embedded pakfile — built for pulling `.phy` /
//! `.mdl` out to inspect by hand when the collision hull looks wrong.
//!
//!   cargo run -p surf-map --example dump_phy --release -- \
//!       assets/maps/surf_boreas.bsp models/project_tendies/ramps/ramp_c1.phy out.phy
//!
//! Validating a decoded `.phy` against the MDL is the quickest correctness
//! check: `hull_min` sits at byte 104 of the `.mdl` header and `hull_max` at
//! byte 116, and the decoded hull must land inside that box. That is how the
//! IVP axis order (`source = (x, z, -y) / 0.0254`) was pinned down.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: dump_phy <map.bsp> <pak/path/inside.phy> [out_file]");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[0]).expect("read bsp");
    let bsp = vbsp::Bsp::read(&data).expect("parse bsp");

    match bsp.pack.get(&args[1]) {
        Ok(Some(bytes)) => {
            println!("{}: {} bytes", args[1], bytes.len());
            if let Some(out) = args.get(2) {
                std::fs::write(out, &bytes).expect("write");
                println!("wrote {out}");
            }
        }
        Ok(None) => {
            eprintln!("{} not present in this map's pakfile", args[1]);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("pakfile error: {e}");
            std::process::exit(1);
        }
    }
}
