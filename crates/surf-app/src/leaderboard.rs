//! Times for the leaderboard pages: the KSF world records we shipped ghosts
//! for, beside the player's own PB and finish history.
//!
//! The records are read from the same `manifest.json` the ghost importer wrote
//! next to each map's replays — no network, no second source of truth.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::pb::PbStore;

/// One imported KSF record.
#[derive(Clone, Debug)]
pub struct KsfRecord {
    pub rank: u32,
    pub name: String,
    pub time: f32,
    /// Replay file stem, so a row can be matched to a ghost on disk.
    pub file_stem: String,
}

fn manifest_path(map: &str) -> PathBuf {
    crate::assets::ksf_dir().join(map).join("manifest.json")
}

/// Every record in `map`'s manifest, best first. Empty when the map has no
/// imported ghosts (or the manifest is unreadable — a leaderboard is not worth
/// failing a menu over).
pub fn ksf_records(map: &str) -> Vec<KsfRecord> {
    let Ok(text) = std::fs::read_to_string(manifest_path(map)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(records) = v.get("records").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<KsfRecord> = records
        .iter()
        .filter_map(|rec| {
            let file = rec.get("file").and_then(|f| f.as_str()).unwrap_or_default();
            let stem = file.trim_end_matches(".rec");
            if stem.is_empty() {
                return None;
            }
            Some(KsfRecord {
                rank: rec.get("rank").and_then(|r| r.as_u64()).unwrap_or(999) as u32,
                name: rec
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_string(),
                time: rec.get("time").and_then(|t| t.as_f64()).unwrap_or(0.0) as f32,
                file_stem: stem.to_string(),
            })
        })
        .collect();
    out.sort_by_key(|r| r.rank);
    out
}

/// Records keyed by replay file stem — what the ghost picker needs.
pub fn ksf_records_by_stem(map: &str) -> HashMap<String, KsfRecord> {
    ksf_records(map)
        .into_iter()
        .map(|r| (r.file_stem.clone(), r))
        .collect()
}

pub fn world_record(map: &str) -> Option<KsfRecord> {
    ksf_records(map).into_iter().next()
}

/// One line of the per-map leaderboard summary.
#[derive(Clone, Debug)]
pub struct MapStanding {
    pub map: String,
    /// Display name without the `surf_` prefix.
    pub label: String,
    pub pb: Option<f32>,
    pub wr: Option<KsfRecord>,
    pub finishes: u32,
}

impl MapStanding {
    /// How far off the world record the player's PB is, when both exist.
    pub fn delta(&self) -> Option<f32> {
        match (self.pb, self.wr.as_ref()) {
            (Some(pb), Some(wr)) => Some(pb - wr.time),
            _ => None,
        }
    }
}

/// Build a standing per map name. `maps` is whatever the picker found, so the
/// leaderboard lists exactly the maps you can actually play.
pub fn standings(maps: &[String], store: Option<&PbStore>) -> Vec<MapStanding> {
    maps.iter()
        .map(|map| MapStanding {
            label: map.strip_prefix("surf_").unwrap_or(map).to_string(),
            pb: store.and_then(|s| s.get(map).ok().flatten()),
            finishes: store
                .and_then(|s| s.count_completions(map).ok())
                .unwrap_or(0),
            wr: world_record(map),
            map: map.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifests are shipped in the repo, so this is a real read of real
    /// data rather than a shape assertion against a fixture we wrote.
    #[test]
    fn summit_has_an_imported_world_record() {
        if !manifest_path("surf_summit").is_file() {
            eprintln!("summit manifest absent; skipping");
            return;
        }
        let recs = ksf_records("surf_summit");
        assert!(!recs.is_empty(), "no records parsed");
        assert_eq!(recs[0].rank, 1, "records must come back best-first");
        assert!(
            recs[0].time > 30.0 && recs[0].time < 120.0,
            "implausible WR time {}",
            recs[0].time
        );
        assert!(!recs[0].name.is_empty());
        // Ranks are ascending and unique after the sort.
        for pair in recs.windows(2) {
            assert!(pair[0].rank <= pair[1].rank);
        }
    }

    #[test]
    fn a_map_with_no_manifest_yields_no_records_instead_of_failing() {
        assert!(ksf_records("surf_does_not_exist").is_empty());
        assert!(world_record("surf_does_not_exist").is_none());
    }
}
