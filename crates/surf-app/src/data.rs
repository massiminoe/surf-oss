//! One saved-data root for settings, PBs, savelocs, and replays.
use std::{
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(crate) fn support_dir() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        resolve(&home.join("Library/Application Support"))
            .unwrap_or_else(|err| panic!("surf-oss saved-data migration failed: {err}"))
    })
}

/// Resolve paths persisted by older versions without rewriting the user's DB.
pub(crate) fn saved_path(path: &str) -> PathBuf {
    rebase(Path::new(path), support_dir())
}

fn rebase(path: &Path, current: &Path) -> PathBuf {
    if let Some(parent) = current.parent() {
        for name in ["mx-surf", "osx-surf"] {
            if let Ok(relative) = path.strip_prefix(parent.join(name)) {
                return current.join(relative);
            }
        }
    }
    path.to_path_buf()
}

fn resolve(parent: &Path) -> io::Result<PathBuf> {
    let destination = parent.join("surf-oss");
    if destination.try_exists()? {
        return Ok(destination);
    }
    for name in ["mx-surf", "osx-surf"] {
        let source = parent.join(name);
        if source.try_exists()? {
            // A same-parent rename moves the entire profile, including SQLite
            // sidecars, in one operation; a failed move leaves the source intact.
            std::fs::rename(&source, &destination).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("{} -> {}: {err}", source.display(), destination.display()),
                )
            })?;
            break;
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_paths_follow_the_profile_but_external_paths_do_not() {
        let current = Path::new("/home/player/Library/Application Support/surf-oss");
        for name in ["mx-surf", "osx-surf"] {
            let old = current
                .parent()
                .unwrap()
                .join(name)
                .join("replays/map/run.osxr");
            assert_eq!(rebase(&old, current), current.join("replays/map/run.osxr"));
        }
        for external in [
            "assets/replays/run.osxr",
            "/tmp/mx-surf/run.osxr",
            "/home/player/Library/Application Support/mx-surf-other/run.osxr",
        ] {
            assert_eq!(rebase(Path::new(external), current), Path::new(external));
        }
    }

    #[test]
    fn migration_preserves_profile_and_prefers_current_name() {
        let root = std::env::temp_dir().join(format!(
            "surf-oss-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("mx-surf/replays/map")).unwrap();
        std::fs::create_dir_all(root.join("osx-surf")).unwrap();
        for file in [
            "settings.json",
            "pbs.sqlite",
            "pbs.sqlite-wal",
            "replays/map/pb.osxr",
        ] {
            std::fs::write(root.join("mx-surf").join(file), file).unwrap();
        }
        let dest = resolve(&root).unwrap();
        for file in [
            "settings.json",
            "pbs.sqlite",
            "pbs.sqlite-wal",
            "replays/map/pb.osxr",
        ] {
            assert_eq!(std::fs::read_to_string(dest.join(file)).unwrap(), file);
        }
        assert!(!root.join("mx-surf").exists());
        assert!(root.join("osx-surf").exists());
        assert_eq!(resolve(&root).unwrap(), dest);
        std::fs::remove_dir_all(&dest).unwrap();
        std::fs::write(root.join("osx-surf/settings.json"), "legacy").unwrap();
        assert_eq!(resolve(&root).unwrap(), dest);
        assert_eq!(
            std::fs::read_to_string(dest.join("settings.json")).unwrap(),
            "legacy"
        );
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(resolve(&root).unwrap(), dest);
        assert!(!dest.exists());
    }
}
