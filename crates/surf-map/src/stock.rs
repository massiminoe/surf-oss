//! Optional local CS:S / HL2 content for stock materials (never shipped).
//!
//! Search order after the BSP pakfile:
//! 1. Loose files under each root (`materials/...`)
//! 2. `*_dir.vpk` archives beside those roots (VPK v2)
//!
//! Configure via env `SURF_OSS_GAME_DIR` (folder containing `cstrike/` and/or `hl2/`),
//! or `SURF_OSS_MATERIALS` (parent of a `materials/` tree). See `docs/M3-HANDOFF.md`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::pak::normalize_path;

pub struct StockFs {
    loose_roots: Vec<PathBuf>,
    vpks: Vec<VpkArchive>,
}

impl StockFs {
    pub fn from_env() -> Self {
        let mut roots = Vec::new();
        if let Ok(dir) = std::env::var("SURF_OSS_GAME_DIR") {
            let p = PathBuf::from(dir);
            for sub in ["cstrike", "hl2"] {
                let cand = p.join(sub);
                if cand.is_dir() {
                    roots.push(cand);
                }
            }
            if roots.is_empty() && p.is_dir() {
                roots.push(p);
            }
        }
        if let Ok(dir) = std::env::var("SURF_OSS_MATERIALS") {
            let p = PathBuf::from(dir);
            if p.is_dir() {
                roots.push(p);
            }
        }
        // Common Steam macOS layout (optional convenience).
        if let Some(home) = std::env::var_os("HOME") {
            let steam = PathBuf::from(home)
                .join("Library/Application Support/Steam/steamapps/common/Counter-Strike Source");
            if steam.is_dir() {
                for sub in ["cstrike", "hl2"] {
                    let cand = steam.join(sub);
                    if cand.is_dir() && !roots.contains(&cand) {
                        roots.push(cand);
                    }
                }
            }
        }
        Self::from_roots(roots)
    }

    pub fn from_roots(roots: Vec<PathBuf>) -> Self {
        let mut vpks = Vec::new();
        for root in &roots {
            if let Ok(rd) = std::fs::read_dir(root) {
                for ent in rd.flatten() {
                    let path = ent.path();
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if name.ends_with("_dir.vpk") {
                        if let Ok(v) = VpkArchive::open(&path) {
                            vpks.push(v);
                        }
                    }
                }
            }
        }
        Self {
            loose_roots: roots,
            vpks,
        }
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        let key = normalize_path(path);
        for root in &self.loose_roots {
            let full = root.join(&key);
            if full.is_file() {
                return std::fs::read(full).ok();
            }
        }
        for vpk in &self.vpks {
            if let Some(bytes) = vpk.read(&key) {
                return Some(bytes);
            }
        }
        None
    }

}

struct VpkEntry {
    archive_index: u16,
    entry_offset: u32,
    entry_length: u32,
    preload: Vec<u8>,
}

struct VpkArchive {
    dir_path: PathBuf,
    header_size: u64,
    tree_size: u64,
    /// lowercase "path/filename.ext" → entry
    index: HashMap<String, VpkEntry>,
}

impl VpkArchive {
    fn open(dir_vpk: &Path) -> Result<Self, String> {
        let mut f = File::open(dir_vpk).map_err(|e| e.to_string())?;
        let mut header = [0u8; 12];
        f.read_exact(&mut header).map_err(|e| e.to_string())?;
        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if magic != 0x55aa_1234 {
            return Err("bad vpk magic".into());
        }
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let tree_size = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
        let header_size = if version == 2 {
            let mut extra = [0u8; 16];
            f.read_exact(&mut extra).map_err(|e| e.to_string())?;
            28u64
        } else {
            12u64
        };
        let mut tree = vec![0u8; tree_size as usize];
        f.read_exact(&mut tree).map_err(|e| e.to_string())?;

        let mut index = HashMap::new();
        let mut i = 0usize;
        loop {
            let ext = read_zt(&tree, &mut i)?;
            if ext.is_empty() {
                break;
            }
            loop {
                let path = read_zt(&tree, &mut i)?;
                if path.is_empty() {
                    break;
                }
                loop {
                    let name = read_zt(&tree, &mut i)?;
                    if name.is_empty() {
                        break;
                    }
                    if i + 18 > tree.len() {
                        return Err("vpk tree truncated".into());
                    }
                    // CRC32 skip
                    i += 4;
                    let preload_bytes =
                        u16::from_le_bytes(tree[i..i + 2].try_into().unwrap()) as usize;
                    i += 2;
                    let archive_index = u16::from_le_bytes(tree[i..i + 2].try_into().unwrap());
                    i += 2;
                    let entry_offset = u32::from_le_bytes(tree[i..i + 4].try_into().unwrap());
                    i += 4;
                    let entry_length = u32::from_le_bytes(tree[i..i + 4].try_into().unwrap());
                    i += 4;
                    let term = u16::from_le_bytes(tree[i..i + 2].try_into().unwrap());
                    i += 2;
                    if term != 0xffff {
                        return Err("bad vpk entry terminator".into());
                    }
                    let preload = if preload_bytes > 0 {
                        if i + preload_bytes > tree.len() {
                            return Err("vpk preload truncated".into());
                        }
                        let p = tree[i..i + preload_bytes].to_vec();
                        i += preload_bytes;
                        p
                    } else {
                        Vec::new()
                    };
                    let key = if path == " " || path.is_empty() {
                        normalize_path(&format!("{name}.{ext}"))
                    } else {
                        normalize_path(&format!("{path}/{name}.{ext}"))
                    };
                    index.insert(
                        key,
                        VpkEntry {
                            archive_index,
                            entry_offset,
                            entry_length,
                            preload,
                        },
                    );
                }
            }
        }

        Ok(Self {
            dir_path: dir_vpk.to_path_buf(),
            header_size,
            tree_size,
            index,
        })
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let ent = self.index.get(&normalize_path(path))?;
        let mut out = ent.preload.clone();
        if ent.entry_length == 0 {
            return Some(out);
        }
        let data_path = if ent.archive_index == 0x7fff {
            self.dir_path.clone()
        } else {
            let stem = self
                .dir_path
                .file_stem()
                .and_then(|s| s.to_str())?
                .strip_suffix("_dir")
                .unwrap_or("")
                .to_string();
            let parent = self.dir_path.parent()?;
            parent.join(format!("{stem}_{:03}.vpk", ent.archive_index))
        };
        let mut f = File::open(data_path).ok()?;
        // 0x7fff: offset into data section after header+tree. Else: offset in numbered archive.
        let seek = if ent.archive_index == 0x7fff {
            self.header_size + self.tree_size + ent.entry_offset as u64
        } else {
            ent.entry_offset as u64
        };
        f.seek(SeekFrom::Start(seek)).ok()?;
        let mut buf = vec![0u8; ent.entry_length as usize];
        f.read_exact(&mut buf).ok()?;
        out.extend_from_slice(&buf);
        Some(out)
    }
}

fn read_zt(buf: &[u8], i: &mut usize) -> Result<String, String> {
    let start = *i;
    while *i < buf.len() && buf[*i] != 0 {
        *i += 1;
    }
    if *i >= buf.len() {
        return Err("unterminated vpk string".into());
    }
    let s = String::from_utf8_lossy(&buf[start..*i]).into_owned();
    *i += 1;
    Ok(s)
}
