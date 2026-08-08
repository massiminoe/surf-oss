//! Import a KSF `.rec` into a cropped native `.osxr` ghost.
//!
//! ```text
//! cargo run -p surf-app --bin ksf-import -- \
//!   assets/replays/external/ksf/surf_summit/raw/replay_css_2946_0_712551_1745965307.rec \
//!   --map surf_summit --time 41.474906
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use surf_app::ksf_replay::{import_ksf_to_replay, STYLE_KSF_CSS_66T};
use surf_app::zones::load_zones_file;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut rec: Option<PathBuf> = None;
    let mut map = String::from("surf_summit");
    let mut out: Option<PathBuf> = None;
    let mut zones_path: Option<PathBuf> = None;
    let mut meta_time: Option<f32> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--map" => {
                map = args.next().unwrap_or_else(|| usage_exit("--map needs value"));
            }
            "--out" => {
                out = Some(PathBuf::from(
                    args.next().unwrap_or_else(|| usage_exit("--out needs value")),
                ));
            }
            "--zones" => {
                zones_path = Some(PathBuf::from(
                    args.next()
                        .unwrap_or_else(|| usage_exit("--zones needs value")),
                ));
            }
            "--time" => {
                let v = args.next().unwrap_or_else(|| usage_exit("--time needs value"));
                meta_time = Some(
                    v.parse()
                        .unwrap_or_else(|_| usage_exit("bad --time value")),
                );
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => usage_exit(&format!("unknown flag {other}")),
            other => {
                if rec.is_some() {
                    usage_exit("unexpected extra argument");
                }
                rec = Some(PathBuf::from(other));
            }
        }
    }

    let rec = match rec {
        Some(p) => p,
        None => usage_exit("missing .rec path"),
    };

    let zpath = zones_path.unwrap_or_else(|| {
        PathBuf::from("assets/zones").join(format!("{map}.json"))
    });
    let zones = match load_zones_file(&zpath) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("zones {}: {e}", zpath.display());
            return ExitCode::FAILURE;
        }
    };
    if zones.map_name != map {
        eprintln!(
            "warning: zone mapName {:?} != --map {map}",
            zones.map_name
        );
    }

    let (replay, crop, header) = match import_ksf_to_replay(&rec, &map, &zones, meta_time) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("import failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let out = out.unwrap_or_else(|| {
        let stem = rec
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("import");
        PathBuf::from("assets/replays/external/ksf")
            .join(&map)
            .join("imported")
            .join(format!("{stem}.osxr"))
    });

    if let Err(e) = replay.save(&out) {
        eprintln!("save failed: {e}");
        return ExitCode::FAILURE;
    }

    let cropped = crop.duration_secs(replay.header.tick_interval);
    println!("KSF import ok");
    println!("  source:   {}", rec.display());
    println!(
        "  header:   {:?} ticks={} bookmarks={} tick_size={}",
        header.version, header.tick_count, header.bookmark_count, header.tick_size
    );
    println!(
        "  crop:     frames [{}..={}] → {} ticks ({cropped:.3}s)",
        crop.start_idx,
        crop.end_idx,
        crop.frame_count()
    );
    if let Some(t) = meta_time {
        println!(
            "  meta:     {t:.6}s  delta={:.4}s",
            cropped - t
        );
    }
    println!(
        "  osxr:     {}  style={STYLE_KSF_CSS_66T}  splits={}",
        out.display(),
        replay.header.splits.len()
    );
    ExitCode::SUCCESS
}

fn print_usage() {
    eprintln!(
        "Usage: ksf-import <file.rec> [--map NAME] [--zones PATH] [--out PATH] [--time SECS]"
    );
}

fn usage_exit(msg: &str) -> ! {
    eprintln!("error: {msg}");
    print_usage();
    std::process::exit(2);
}
