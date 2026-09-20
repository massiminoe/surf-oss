//! Where the game's `assets/` tree lives at runtime.
//!
//! Everything used to be looked up relative to the current directory, which is
//! fine while you only ever launch with `cargo run` from the repo root and
//! quietly broken the moment `surf-oss` is on your PATH: `read_dir("assets/maps")`
//! fails from any other directory and the map picker comes up empty.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The `assets/` directory, resolved once per process.
///
/// Explicit assets override, installed content, then a populated development
/// checkout. An isolated data directory always bypasses the development tree.
pub fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if std::env::var_os("SURF_OSS_DATA_DIR").is_some() {
            return crate::data::support_dir().join("content");
        }
        if let Some(p) = std::env::var_os("SURF_OSS_ASSETS") {
            let p = PathBuf::from(p);
            if p.is_dir() {
                return p;
            }
            eprintln!(
                "SURF_OSS_ASSETS={} is not a directory; ignoring",
                p.display()
            );
        }
        let installed = crate::data::support_dir().join("content");
        // Packaging changes the executable's location, not the user's content.
        // A locally built app must retain the same checkout fallback as the CLI.
        // On another machine that checkout is absent, so installed content wins.
        if installed.join("maps").is_dir() {
            return installed;
        }
        let cwd = PathBuf::from("assets");
        if cwd.join("maps/surf_summit.bsp").is_file() {
            return cwd;
        }
        // `CARGO_MANIFEST_DIR` is baked in at compile time: crates/surf-app.
        let built_from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        if built_from.join("maps/surf_summit.bsp").is_file() {
            // Tidy the `../..` away so log lines and errors stay readable.
            return built_from.canonicalize().unwrap_or(built_from);
        }
        installed
    })
}

pub fn maps_dir() -> PathBuf {
    root().join("maps")
}

pub fn zones_dir() -> PathBuf {
    root().join("zones")
}

/// KSF imported ghosts: `assets/replays/external/ksf/`.
pub fn ksf_dir() -> PathBuf {
    root().join("replays/external/ksf")
}

/// Resolve a map argument: a path, a full stem (`surf_summit`), or a short name
/// (`summit`). Returns the argument as given if nothing matches, so the caller
/// can report a not-found against what the user actually typed.
pub fn resolve_map_arg(s: &str) -> PathBuf {
    let direct = PathBuf::from(s);
    if direct.is_file() {
        return direct;
    }
    for stem in [s.to_string(), format!("surf_{s}")] {
        let p = maps_dir().join(format!("{stem}.bsp"));
        if p.is_file() {
            return p;
        }
    }
    direct
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: the tree is found without depending on the current
    /// directory. Tests run with cwd = the package dir, which has no `assets/`,
    /// so this exercises the compiled-in fallback rather than the cwd branch.
    #[test]
    fn the_assets_root_resolves_without_relying_on_the_working_directory() {
        let r = root();
        assert!(r.is_dir(), "assets root not found at {}", r.display());
        assert!(
            maps_dir().is_dir() || zones_dir().is_dir(),
            "resolved {} but it holds neither maps/ nor zones/",
            r.display()
        );
    }

    #[test]
    fn a_short_map_name_resolves_to_a_real_bsp() {
        if !maps_dir().join("surf_summit.bsp").is_file() {
            eprintln!("summit absent; skipping");
            return;
        }
        assert_eq!(
            resolve_map_arg("summit"),
            maps_dir().join("surf_summit.bsp")
        );
        assert_eq!(
            resolve_map_arg("surf_summit"),
            maps_dir().join("surf_summit.bsp")
        );
    }
}
