//! Local personal-best store (SQLite).

use std::fs;
use std::path::PathBuf;

use rusqlite::{params, Connection};

const STYLE_MOMENTUM_SURF: &str = "momentum_surf";
const TRACK_MAIN: &str = "main";

pub struct PbStore {
    conn: Connection,
}

impl PbStore {
    pub fn open_default() -> Result<Self, String> {
        let path = default_db_path();
        Self::open_path(&path)
    }

    pub fn open_path(path: &std::path::Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("pb dir: {e}"))?;
        }
        let conn = Connection::open(path).map_err(|e| format!("pb open: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS personal_bests (
                map TEXT NOT NULL,
                track TEXT NOT NULL,
                style TEXT NOT NULL,
                time_secs REAL NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (map, track, style)
            );",
        )
        .map_err(|e| format!("pb migrate: {e}"))?;
        Ok(Self { conn })
    }

    pub fn get(&self, map: &str) -> Result<Option<f32>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT time_secs FROM personal_bests
                 WHERE map = ?1 AND track = ?2 AND style = ?3",
            )
            .map_err(|e| format!("pb prepare: {e}"))?;
        let mut rows = stmt
            .query(params![map, TRACK_MAIN, STYLE_MOMENTUM_SURF])
            .map_err(|e| format!("pb query: {e}"))?;
        if let Some(row) = rows.next().map_err(|e| format!("pb row: {e}"))? {
            let t: f64 = row.get(0).map_err(|e| format!("pb get: {e}"))?;
            Ok(Some(t as f32))
        } else {
            Ok(None)
        }
    }

    /// Returns `Some(new_pb)` if this time beat the previous PB (or first finish).
    pub fn record_finish(&self, map: &str, time_secs: f32) -> Result<Option<f32>, String> {
        let prev = self.get(map)?;
        let is_pb = match prev {
            None => true,
            Some(p) => time_secs < p,
        };
        if !is_pb {
            return Ok(None);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO personal_bests (map, track, style, time_secs, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(map, track, style) DO UPDATE SET
                   time_secs = excluded.time_secs,
                   updated_at = excluded.updated_at",
                params![
                    map,
                    TRACK_MAIN,
                    STYLE_MOMENTUM_SURF,
                    time_secs as f64,
                    now
                ],
            )
            .map_err(|e| format!("pb upsert: {e}"))?;
        Ok(Some(time_secs))
    }
}

fn default_db_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/osx-surf/pbs.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn record_and_beat_pb() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("osx-surf-pb-{stamp}.sqlite"));
        let store = PbStore::open_path(&path).expect("open");
        assert!(store.get("surf_test").unwrap().is_none());
        assert_eq!(store.record_finish("surf_test", 40.0).unwrap(), Some(40.0));
        assert!((store.get("surf_test").unwrap().unwrap() - 40.0).abs() < 1e-5);
        assert!(store.record_finish("surf_test", 41.0).unwrap().is_none());
        assert_eq!(store.record_finish("surf_test", 39.5).unwrap(), Some(39.5));
        let _ = std::fs::remove_file(&path);
    }
}
