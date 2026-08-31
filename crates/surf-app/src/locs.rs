//! Saved locations ("locs") — practice checkpoints.
//!
//! A loc freezes everything that decides how the next second plays out: pose,
//! velocity, view angles, duck state, and the run clock (elapsed time, phase,
//! splits, stage). Loading one puts all of it back, which is why a loaded run
//! is marked practice by the caller: the clock is real, the run is not.
//!
//! Stored per map at `~/Library/Application Support/mx-surf/locs/<map>.json`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use surf_core::math::{Angle, Vec3};
use surf_core::movement::PlayerState;

use crate::timer::{TimerPhase, TimerSnapshot};

pub const LOCS_FORMAT_VERSION: u32 = 1;

/// Upper bound on locs per map. Saveloc is a spam key; without a cap the file
/// and the menu list both grow without limit.
pub const MAX_LOCS: usize = 64;

/// Map name used when there is no BSP (graybox).
pub const GRAYBOX_MAP: &str = "graybox";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Loc {
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    /// pitch, yaw, roll.
    pub angles: [f32; 3],
    pub ducked: bool,
    pub grounded: bool,
    pub ground_normal: [f32; 3],
    pub surface_friction: f32,
    pub old_jump: bool,
    /// Run clock at save time.
    pub time_secs: f32,
    /// [`TimerPhase::as_str`].
    pub phase: String,
    pub current_stage: usize,
    pub splits: Vec<f32>,
    pub next_cp: usize,
}

impl Loc {
    pub fn capture(player: &PlayerState, timer: &TimerSnapshot) -> Self {
        Self {
            origin: [player.origin.x, player.origin.y, player.origin.z],
            velocity: [player.velocity.x, player.velocity.y, player.velocity.z],
            angles: [
                player.viewangles.pitch,
                player.viewangles.yaw,
                player.viewangles.roll,
            ],
            ducked: player.ducked,
            grounded: player.grounded,
            ground_normal: [
                player.ground_normal.x,
                player.ground_normal.y,
                player.ground_normal.z,
            ],
            surface_friction: player.surface_friction,
            old_jump: player.old_jump,
            time_secs: timer.time_secs,
            phase: timer.phase.as_str().to_string(),
            current_stage: timer.current_stage,
            splits: timer.splits.clone(),
            next_cp: timer.next_cp,
        }
    }

    /// Overwrite the movement-relevant parts of `player`. `basevelocity` and
    /// `gravity_scale` are intentionally left out — field triggers rewrite both
    /// at the top of every tick, so restoring them would be meaningless.
    pub fn apply(&self, player: &mut PlayerState) {
        player.origin = Vec3::new(self.origin[0], self.origin[1], self.origin[2]);
        player.velocity = Vec3::new(self.velocity[0], self.velocity[1], self.velocity[2]);
        player.viewangles = Angle::new(self.angles[0], self.angles[1], self.angles[2]);
        player.ducked = self.ducked;
        player.grounded = self.grounded;
        player.ground_normal = Vec3::new(
            self.ground_normal[0],
            self.ground_normal[1],
            self.ground_normal[2],
        );
        player.surface_friction = self.surface_friction;
        player.old_jump = self.old_jump;
        player.basevelocity = Vec3::ZERO;
        player.gravity_scale = 1.0;
    }

    pub fn timer_snapshot(&self) -> TimerSnapshot {
        TimerSnapshot {
            phase: TimerPhase::from_str_or_idle(&self.phase),
            time_secs: self.time_secs,
            current_stage: self.current_stage.max(1),
            splits: self.splits.clone(),
            next_cp: self.next_cp,
        }
    }

    /// 2D speed at save time — the one number that identifies a loc at a glance.
    pub fn speed_2d(&self) -> f32 {
        (self.velocity[0] * self.velocity[0] + self.velocity[1] * self.velocity[1]).sqrt()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LocFile {
    format_version: u32,
    map: String,
    locs: Vec<Loc>,
}

/// Per-map loc list plus the currently selected index (what a bare loadloc uses).
#[derive(Clone, Debug, Default)]
pub struct LocStore {
    pub map: String,
    locs: Vec<Loc>,
    selected: usize,
    path: Option<PathBuf>,
}

impl LocStore {
    /// Load the map's locs, or an empty store if none exist / the file is bad.
    pub fn load_for_map(map: &str) -> Self {
        let path = locs_path_for_map(map);
        let mut store = Self {
            map: map.to_string(),
            locs: Vec::new(),
            selected: 0,
            path: Some(path.clone()),
        };
        if let Ok(text) = fs::read_to_string(&path) {
            match serde_json::from_str::<LocFile>(&text) {
                Ok(file) => {
                    store.locs = file.locs;
                    store.locs.truncate(MAX_LOCS);
                    store.selected = store.locs.len().saturating_sub(1);
                }
                Err(e) => eprintln!("locs load failed ({}): {e}", path.display()),
            }
        }
        store
    }

    /// In-memory store with no backing file (tests).
    pub fn in_memory(map: &str) -> Self {
        Self {
            map: map.to_string(),
            locs: Vec::new(),
            selected: 0,
            path: None,
        }
    }

    pub fn len(&self) -> usize {
        self.locs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locs.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Loc> {
        self.locs.get(index)
    }

    /// Selected index, clamped into range. `None` when the list is empty.
    pub fn selected(&self) -> Option<usize> {
        if self.locs.is_empty() {
            None
        } else {
            Some(self.selected.min(self.locs.len() - 1))
        }
    }

    pub fn selected_loc(&self) -> Option<&Loc> {
        self.selected().and_then(|i| self.locs.get(i))
    }

    pub fn set_selected(&mut self, index: usize) {
        if !self.locs.is_empty() {
            self.selected = index.min(self.locs.len() - 1);
        }
    }

    /// Move the selection by `dir`, wrapping.
    pub fn cycle_selected(&mut self, dir: i32) {
        if self.locs.is_empty() {
            return;
        }
        let n = self.locs.len() as i32;
        let cur = self.selected.min(self.locs.len() - 1) as i32;
        self.selected = (cur + dir).rem_euclid(n) as usize;
    }

    /// Append a loc and select it. `Err` when the list is full.
    pub fn push(&mut self, loc: Loc) -> Result<usize, String> {
        if self.locs.len() >= MAX_LOCS {
            return Err(format!("loc list full ({MAX_LOCS})"));
        }
        self.locs.push(loc);
        self.selected = self.locs.len() - 1;
        Ok(self.locs.len() - 1)
    }

    /// Remove one loc. Later locs renumber down; selection follows the gap.
    pub fn remove(&mut self, index: usize) -> Option<Loc> {
        if index >= self.locs.len() {
            return None;
        }
        let loc = self.locs.remove(index);
        if !self.locs.is_empty() {
            self.selected = self.selected.min(self.locs.len() - 1);
        } else {
            self.selected = 0;
        }
        Some(loc)
    }

    pub fn clear(&mut self) {
        self.locs.clear();
        self.selected = 0;
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("locs dir: {e}"))?;
        }
        if self.locs.is_empty() {
            // Nothing to remember: drop the file rather than leaving an empty one.
            if path.is_file() {
                fs::remove_file(path).map_err(|e| format!("locs remove: {e}"))?;
            }
            return Ok(());
        }
        let file = LocFile {
            format_version: LOCS_FORMAT_VERSION,
            map: self.map.clone(),
            locs: self.locs.clone(),
        };
        let text =
            serde_json::to_string_pretty(&file).map_err(|e| format!("locs serialize: {e}"))?;
        fs::write(path, text).map_err(|e| format!("locs write: {e}"))
    }

    /// Menu value column: `3/7  1420 u/s`, or `none`.
    pub fn menu_label(&self) -> String {
        match (self.selected(), self.selected_loc()) {
            (Some(i), Some(loc)) => {
                format!("{}/{}  {:.0} u/s", i + 1, self.locs.len(), loc.speed_2d())
            }
            _ => "none".into(),
        }
    }
}

/// First visible index for a list of `total` items showing at most `visible`
/// rows, keeping `focus` on screen and centred where possible. The window is
/// always flush against the end of the list rather than running past it.
pub fn scroll_window_start(total: usize, visible: usize, focus: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    focus.saturating_sub(visible / 2).min(total - visible)
}

/// Locs-panel row layout. Logical rows are: `0` practice toggle, `1..=n` the
/// locs, then load and clear-all. Only [`scroll_window_start`]'s window of loc
/// rows is actually drawn, so the highlight has to be remapped onto the drawn
/// list — that remap is [`drawn_row`].
pub fn locs_page_len(total: usize) -> usize {
    total + 3
}

/// Loc index a logical row points at, if it is a loc row.
pub fn loc_index_for_row(total: usize, row: usize) -> Option<usize> {
    if row >= 1 && row <= total {
        Some(row - 1)
    } else {
        None
    }
}

/// Position of logical `row` within the drawn (windowed) row list.
pub fn drawn_row(total: usize, visible_cap: usize, row: usize) -> usize {
    let visible = total.min(visible_cap);
    let focus = loc_index_for_row(total, row).unwrap_or(0);
    let first = scroll_window_start(total, visible_cap, focus);
    match loc_index_for_row(total, row) {
        Some(i) => 1 + i.saturating_sub(first),
        None if row == 0 => 0,
        // Load, then clear-all.
        None if row == total + 1 => 1 + visible,
        None => 2 + visible,
    }
}

pub fn locs_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/mx-surf/locs")
}

pub fn locs_path_for_map(map: &str) -> PathBuf {
    locs_dir().join(format!("{}.json", sanitize_map(map)))
}

/// Map names come from the BSP; keep them to a filename-safe subset.
fn sanitize_map(map: &str) -> String {
    let cleaned: String = map
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        GRAYBOX_MAP.to_string()
    } else {
        cleaned
    }
}

/// Round-trip a loc list through JSON (used by tests and by the loader).
pub fn parse_locs_json(text: &str) -> Result<Vec<Loc>, String> {
    let file: LocFile = serde_json::from_str(text).map_err(|e| format!("locs json: {e}"))?;
    Ok(file.locs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::TimerPhase;

    fn sample_player() -> PlayerState {
        PlayerState {
            origin: Vec3::new(100.5, -220.25, 3300.0),
            velocity: Vec3::new(1200.0, -900.0, 250.0),
            viewangles: Angle::new(-12.5, 88.0, 0.0),
            ducked: true,
            grounded: false,
            surface_friction: 0.75,
            old_jump: true,
            ground_normal: Vec3::new(0.0, 0.0, 1.0),
            ..PlayerState::default()
        }
    }

    fn sample_snapshot() -> TimerSnapshot {
        TimerSnapshot {
            phase: TimerPhase::Running,
            time_secs: 21.375,
            current_stage: 3,
            splits: vec![4.5, 11.25],
            next_cp: 2,
        }
    }

    #[test]
    fn capture_then_apply_restores_pose_exactly() {
        let player = sample_player();
        let loc = Loc::capture(&player, &sample_snapshot());
        let mut restored = PlayerState::default();
        loc.apply(&mut restored);
        assert_eq!(restored.origin.x, player.origin.x);
        assert_eq!(restored.origin.y, player.origin.y);
        assert_eq!(restored.origin.z, player.origin.z);
        assert_eq!(restored.velocity.x, player.velocity.x);
        assert_eq!(restored.velocity.z, player.velocity.z);
        assert_eq!(restored.viewangles.yaw, player.viewangles.yaw);
        assert_eq!(restored.viewangles.pitch, player.viewangles.pitch);
        assert!(restored.ducked);
        assert!(!restored.grounded);
        assert_eq!(restored.surface_friction, 0.75);
        assert!(restored.old_jump);
    }

    #[test]
    fn timer_snapshot_survives_json_roundtrip() {
        let loc = Loc::capture(&sample_player(), &sample_snapshot());
        let file = LocFile {
            format_version: LOCS_FORMAT_VERSION,
            map: "surf_summit".into(),
            locs: vec![loc],
        };
        let text = serde_json::to_string(&file).unwrap();
        let back = parse_locs_json(&text).unwrap();
        let snap = back[0].timer_snapshot();
        assert_eq!(snap.phase, TimerPhase::Running);
        assert!((snap.time_secs - 21.375).abs() < 1e-6);
        assert_eq!(snap.current_stage, 3);
        assert_eq!(snap.splits, vec![4.5, 11.25]);
        assert_eq!(snap.next_cp, 2);
    }

    #[test]
    fn push_selects_newest_and_caps() {
        let mut store = LocStore::in_memory("t");
        assert!(store.selected().is_none());
        for _ in 0..MAX_LOCS {
            store
                .push(Loc::capture(&sample_player(), &sample_snapshot()))
                .unwrap();
        }
        assert_eq!(store.len(), MAX_LOCS);
        assert_eq!(store.selected(), Some(MAX_LOCS - 1));
        assert!(store
            .push(Loc::capture(&sample_player(), &sample_snapshot()))
            .is_err());
    }

    #[test]
    fn remove_and_clear_keep_selection_in_range() {
        let mut store = LocStore::in_memory("t");
        for _ in 0..3 {
            store
                .push(Loc::capture(&sample_player(), &sample_snapshot()))
                .unwrap();
        }
        assert_eq!(store.selected(), Some(2));
        store.remove(2);
        assert_eq!(store.selected(), Some(1));
        store.remove(0);
        assert_eq!(store.len(), 1);
        assert_eq!(store.selected(), Some(0));
        store.clear();
        assert!(store.selected().is_none());
        assert_eq!(store.menu_label(), "none");
    }

    #[test]
    fn cycle_selected_wraps() {
        let mut store = LocStore::in_memory("t");
        for _ in 0..3 {
            store
                .push(Loc::capture(&sample_player(), &sample_snapshot()))
                .unwrap();
        }
        store.set_selected(0);
        store.cycle_selected(-1);
        assert_eq!(store.selected(), Some(2));
        store.cycle_selected(1);
        assert_eq!(store.selected(), Some(0));
    }

    #[test]
    fn scroll_window_keeps_focus_visible() {
        // Short list: no scrolling at all.
        assert_eq!(scroll_window_start(5, 14, 4), 0);
        // Long list: centred…
        assert_eq!(scroll_window_start(40, 14, 20), 13);
        // …clamped at the top…
        assert_eq!(scroll_window_start(40, 14, 2), 0);
        // …and flush at the bottom, never past the end.
        assert_eq!(scroll_window_start(40, 14, 39), 26);
        // Focus is inside the window in every case.
        for focus in 0..40 {
            let first = scroll_window_start(40, 14, focus);
            assert!(focus >= first && focus < first + 14, "focus {focus} lost");
            assert!(first + 14 <= 40);
        }
    }

    #[test]
    fn page_rows_map_onto_drawn_rows() {
        // Short list — every row is drawn, so logical == drawn.
        let n = 3;
        assert_eq!(locs_page_len(n), 6);
        for row in 0..locs_page_len(n) {
            assert_eq!(drawn_row(n, 14, row), row, "row {row}");
        }

        // Long list — locs scroll, and the two trailing action rows always sit
        // immediately after the visible window.
        let n = 40;
        assert_eq!(drawn_row(n, 14, 0), 0, "practice row stays first");
        assert_eq!(drawn_row(n, 14, 41), 15, "load after 14 loc rows");
        assert_eq!(drawn_row(n, 14, 42), 16, "clear-all is last");
        // Selected loc always lands on a drawn loc row (1..=14).
        for row in 1..=n {
            let d = drawn_row(n, 14, row);
            assert!((1..=14).contains(&d), "row {row} drawn at {d}");
        }
    }

    #[test]
    fn empty_list_still_has_practice_load_and_clear() {
        assert_eq!(locs_page_len(0), 3);
        assert_eq!(drawn_row(0, 14, 0), 0);
        assert_eq!(drawn_row(0, 14, 1), 1);
        assert_eq!(drawn_row(0, 14, 2), 2);
        assert!(loc_index_for_row(0, 1).is_none());
    }

    #[test]
    fn map_names_are_filename_safe() {
        assert_eq!(sanitize_map("surf_summit"), "surf_summit");
        assert_eq!(sanitize_map("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_map(""), GRAYBOX_MAP);
    }
}
