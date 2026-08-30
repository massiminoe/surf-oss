//! Census of the touch-driven effects a map applies to the player.
//!
//! Boosters, launch pads and one-shot gates are built out of `trigger_multiple`
//! + `AddOutput`, so they are invisible in the classname census `diag_map`
//! prints. This names each one and what it does.
//!
//! With `--replay` it also walks the map's KSF WR recording and reports every
//! tick a trigger fires, next to the velocity change the *recording* shows at
//! that tick — so a booster can be checked against real CS:S rather than
//! against our own belief about it.
//!
//! `cargo run -p surf-app --example field_audit --release -- [map.bsp | --all] [--replay]`

use surf_app::replay::Replay;
use surf_core::math::Vec3;
use surf_map::{FieldAction, FieldState, LoadedMap};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let replay = args.iter().any(|a| a == "--replay");
    let arg = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "--all".into());
    let paths: Vec<String> = if arg == "--all" {
        let mut v: Vec<String> = std::fs::read_dir("assets/maps")
            .expect("assets/maps")
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "bsp").then(|| p.to_string_lossy().into_owned())
            })
            .collect();
        v.sort();
        v
    } else {
        vec![arg]
    };

    for path in paths {
        let map = match LoadedMap::load_path(&path) {
            Ok(m) => m,
            Err(e) => {
                println!("{path}: load failed: {e}");
                continue;
            }
        };
        if map.pushes.is_empty() && map.gravities.is_empty() && map.fields.is_empty() {
            continue;
        }
        println!("== {} ==", map.name);
        for (i, p) in map.pushes.iter().enumerate() {
            println!(
                "  push[{i}]  {:.0} u/s dir=({:.2},{:.2},{:.2})  ({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})",
                p.velocity.length(),
                p.velocity.x / p.velocity.length(),
                p.velocity.y / p.velocity.length(),
                p.velocity.z / p.velocity.length(),
                p.bounds.mins.x,
                p.bounds.mins.y,
                p.bounds.mins.z,
                p.bounds.maxs.x,
                p.bounds.maxs.y,
                p.bounds.maxs.z,
            );
        }
        for (i, g) in map.gravities.iter().enumerate() {
            println!(
                "  grav[{i}]  x{}  at ({:.0},{:.0},{:.0})",
                g.scale, g.bounds.mins.x, g.bounds.mins.y, g.bounds.mins.z
            );
        }
        for (i, f) in map.fields.iter().enumerate() {
            let gate = match &f.filter {
                Some(nf) if nf.negated => format!("  [only if name != {}]", nf.name),
                Some(nf) => format!("  [only if name == {}]", nf.name),
                None => String::new(),
            };
            println!(
                "  field[{i}] ({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0}){gate}",
                f.bounds.mins.x,
                f.bounds.mins.y,
                f.bounds.mins.z,
                f.bounds.maxs.x,
                f.bounds.maxs.y,
                f.bounds.maxs.z
            );
            for (edge, actions) in [("start", &f.on_start), ("end", &f.on_end)] {
                for a in actions.iter() {
                    println!("      on_{edge}: {}", describe(a));
                }
            }
        }
        if replay {
            walk_wr(&map);
        }
    }
}

/// Fire the map's triggers along the KSF WR path and print each payout beside
/// the velocity step the recording actually took on that tick.
fn walk_wr(map: &LoadedMap) {
    let dir = surf_app::replay::ksf_imported_dir(&map.name);
    let Some(path) = std::fs::read_dir(&dir).ok().and_then(|rd| {
        rd.filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "osxr"))
    }) else {
        println!("  (no imported KSF replay under {})", dir.display());
        return;
    };
    let Ok(rep) = Replay::load(&path) else {
        println!("  (replay unreadable)");
        return;
    };
    let frames = rep.frames;
    let mut state = FieldState::new();
    for (i, f) in frames.iter().enumerate() {
        let eff = state.update(&map.fields, f.origin, map.touch_push(f.origin));
        if eff.released == Vec3::ZERO {
            continue;
        }
        // The payout lands on the *next* move, so that is the tick of the
        // recording it has to explain.
        let Some(next) = frames.get(i + 1) else {
            continue;
        };
        let recorded = next.velocity - f.velocity;
        println!(
            "    end of t{i:<5} pays out ({:.0},{:.0},{:.0}) -> t{:<5} of the recording steps ({:.0},{:.0},{:.0})",
            eff.released.x,
            eff.released.y,
            eff.released.z,
            i + 1,
            recorded.x,
            recorded.y,
            recorded.z,
        );
    }
}

fn describe(a: &FieldAction) -> String {
    match a {
        FieldAction::BaseVelocity(v) => format!(
            "boost ({:.0},{:.0},{:.0}) — {:.0} u/s",
            v.x,
            v.y,
            v.z,
            v.length()
        ),
        FieldAction::Gravity(g) => format!("gravity x{g}"),
        FieldAction::TargetName(n) => format!("rename player -> {n:?}"),
    }
}
