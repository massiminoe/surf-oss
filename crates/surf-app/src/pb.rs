//! Local personal-best + run history store (SQLite).

use std::fs;
use std::path::PathBuf;

use rusqlite::{params, Connection};

use crate::replay::{STYLE_MOMENTUM_SURF, TRACK_MAIN};

#[derive(Clone, Debug)]
pub struct RunRecord {
    pub time_secs: f32,
    pub is_pb: bool,
    pub finished_at: i64,
    pub replay_path: Option<String>,
}

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
                replay_path TEXT,
                PRIMARY KEY (map, track, style)
            );
            CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY,
                map TEXT NOT NULL,
                track TEXT NOT NULL,
                style TEXT NOT NULL,
                time_secs REAL NOT NULL,
                is_pb INTEGER NOT NULL,
                finished_at INTEGER NOT NULL,
                replay_path TEXT
            );
            CREATE INDEX IF NOT EXISTS runs_map_time
                ON runs(map, track, style, time_secs);
            CREATE INDEX IF NOT EXISTS runs_map_recent
                ON runs(map, track, style, id DESC);",
        )
        .map_err(|e| format!("pb migrate: {e}"))?;
        // Additive migrations for DBs created before replay support.
        let _ = conn.execute(
            "ALTER TABLE runs ADD COLUMN replay_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE personal_bests ADD COLUMN replay_path TEXT",
            [],
        );
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

    /// Record a finish: always appends to `runs`; upserts PB when improved.
    /// `replay_path` is stored on the run row and on the PB row when this is a PB.
    /// Returns `Some(new_pb)` if this time beat the previous PB (or first finish).
    pub fn record_finish(
        &self,
        map: &str,
        time_secs: f32,
        replay_path: Option<&str>,
    ) -> Result<Option<f32>, String> {
        let prev = self.get(map)?;
        let is_pb = match prev {
            None => true,
            Some(p) => time_secs < p,
        };
        let now = unix_now();
        self.conn
            .execute(
                "INSERT INTO runs (map, track, style, time_secs, is_pb, finished_at, replay_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    map,
                    TRACK_MAIN,
                    STYLE_MOMENTUM_SURF,
                    time_secs as f64,
                    if is_pb { 1i32 } else { 0i32 },
                    now,
                    replay_path
                ],
            )
            .map_err(|e| format!("runs insert: {e}"))?;

        if !is_pb {
            return Ok(None);
        }
        self.conn
            .execute(
                "INSERT INTO personal_bests (map, track, style, time_secs, updated_at, replay_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(map, track, style) DO UPDATE SET
                   time_secs = excluded.time_secs,
                   updated_at = excluded.updated_at,
                   replay_path = excluded.replay_path",
                params![
                    map,
                    TRACK_MAIN,
                    STYLE_MOMENTUM_SURF,
                    time_secs as f64,
                    now,
                    replay_path
                ],
            )
            .map_err(|e| format!("pb upsert: {e}"))?;
        Ok(Some(time_secs))
    }

    /// Path of the current PB replay file, if any.
    pub fn pb_replay_path(&self, map: &str) -> Result<Option<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT replay_path FROM personal_bests
                 WHERE map = ?1 AND track = ?2 AND style = ?3",
            )
            .map_err(|e| format!("pb prepare: {e}"))?;
        let mut rows = stmt
            .query(params![map, TRACK_MAIN, STYLE_MOMENTUM_SURF])
            .map_err(|e| format!("pb query: {e}"))?;
        if let Some(row) = rows.next().map_err(|e| format!("pb row: {e}"))? {
            let path: Option<String> = row.get(0).map_err(|e| format!("pb get: {e}"))?;
            Ok(path.filter(|p| !p.is_empty()))
        } else {
            Ok(None)
        }
    }

    pub fn list_recent(&self, map: &str, limit: usize) -> Result<Vec<RunRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT time_secs, is_pb, finished_at, replay_path FROM runs
                 WHERE map = ?1 AND track = ?2 AND style = ?3
                 ORDER BY id DESC
                 LIMIT ?4",
            )
            .map_err(|e| format!("runs prepare: {e}"))?;
        let rows = stmt
            .query_map(
                params![map, TRACK_MAIN, STYLE_MOMENTUM_SURF, limit as i64],
                |row| {
                    Ok(RunRecord {
                        time_secs: row.get::<_, f64>(0)? as f32,
                        is_pb: row.get::<_, i32>(1)? != 0,
                        finished_at: row.get(2)?,
                        replay_path: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("runs query: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("runs row: {e}"))?);
        }
        Ok(out)
    }

    pub fn count_completions(&self, map: &str) -> Result<u32, String> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM runs
                 WHERE map = ?1 AND track = ?2 AND style = ?3",
                params![map, TRACK_MAIN, STYLE_MOMENTUM_SURF],
                |row| row.get(0),
            )
            .map_err(|e| format!("runs count: {e}"))?;
        Ok(n as u32)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

    fn temp_store() -> (PbStore, PathBuf) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("osx-surf-pb-{stamp}.sqlite"));
        let store = PbStore::open_path(&path).expect("open");
        (store, path)
    }

    #[test]
    fn record_and_beat_pb() {
        let (store, path) = temp_store();
        assert!(store.get("surf_test").unwrap().is_none());
        assert_eq!(
            store.record_finish("surf_test", 40.0, Some("/tmp/a.osxr")).unwrap(),
            Some(40.0)
        );
        assert!((store.get("surf_test").unwrap().unwrap() - 40.0).abs() < 1e-5);
        assert_eq!(
            store.pb_replay_path("surf_test").unwrap().as_deref(),
            Some("/tmp/a.osxr")
        );
        assert!(store
            .record_finish("surf_test", 41.0, Some("/tmp/b.osxr"))
            .unwrap()
            .is_none());
        assert_eq!(
            store.record_finish("surf_test", 39.5, Some("/tmp/c.osxr")).unwrap(),
            Some(39.5)
        );
        assert_eq!(
            store.pb_replay_path("surf_test").unwrap().as_deref(),
            Some("/tmp/c.osxr")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn run_history_records_all_finishes() {
        let (store, path) = temp_store();
        store.record_finish("surf_test", 40.0, None).unwrap();
        store
            .record_finish("surf_test", 41.0, Some("/tmp/r.osxr"))
            .unwrap();
        store.record_finish("surf_test", 39.0, None).unwrap();
        assert_eq!(store.count_completions("surf_test").unwrap(), 3);
        let recent = store.list_recent("surf_test", 5).unwrap();
        assert_eq!(recent.len(), 3);
        assert!((recent[0].time_secs - 39.0).abs() < 1e-5);
        assert!(recent[0].is_pb);
        assert!(!recent[1].is_pb);
        assert_eq!(recent[1].replay_path.as_deref(), Some("/tmp/r.osxr"));
        assert!(recent[2].is_pb);
        let _ = std::fs::remove_file(&path);
    }
}
