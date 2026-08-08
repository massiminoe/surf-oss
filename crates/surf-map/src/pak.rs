//! Case-insensitive pakfile (BSP lump 40 ZIP) lookups.

use std::collections::HashMap;

use vbsp::Bsp;

/// Normalize Source filesystem paths: lowercase, forward slashes, no leading `/`.
/// Collapses `//` (common in cubemap-patched VMTs: `materials//TILE_22.vmt`).
pub fn normalize_path(path: &str) -> String {
    let mut s = path.trim().replace('\\', "/").to_ascii_lowercase();
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    while s.starts_with('/') {
        s.remove(0);
    }
    s
}

/// Indexed view of a BSP pakfile for material loading.
pub struct PakFs {
    /// lowercase path → exact zip entry name
    index: HashMap<String, String>,
    pack: vbsp::Packfile,
}

impl PakFs {
    pub fn from_bsp(bsp: &Bsp) -> Self {
        let pack = bsp.pack.clone();
        let zip_mutex = bsp.pack.clone().into_zip();
        let zip = zip_mutex.lock().unwrap();
        let mut index = HashMap::new();
        for name in zip.file_names() {
            let key = normalize_path(name);
            index.entry(key).or_insert_with(|| name.to_string());
        }
        Self { index, pack }
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        let key = normalize_path(path);
        let entry = self.index.get(&key)?;
        self.pack.get(entry).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_path;

    #[test]
    fn collapses_double_slash() {
        assert_eq!(
            normalize_path("materials//TILE_22.vmt"),
            "materials/tile_22.vmt"
        );
        assert_eq!(
            normalize_path("maps/surf_pantheon//tile_22_-1_2_3"),
            "maps/surf_pantheon/tile_22_-1_2_3"
        );
    }
}
