//! The menu model: what a settings row *is*, what activating one does, and
//! where the cursor sits on each page.
//!
//! This lives in the library rather than in `main.rs` for one reason: the
//! binary can't be unit-tested, and the parts most likely to break silently —
//! a setting added to the enum but not to the page, a slider whose range
//! doesn't round-trip — are exactly the parts that are pure data.

/// One adjustable setting. The menu is built from a table of these rather than
/// from index constants: a row's position stops being load-bearing, and adding
/// one can't silently shift what every other row does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    Sens,
    Brightness,
    ShadowLift,
    Vsync,
    ShowSync,
    ShowKeys,
    Ghost,
    GhostTrail,
    Airaccel,
    Audio,
    AudioVolume,
    AudioCore,
    AudioAir,
    AudioSub,
    Wipe,
    TurnSpeed,
}

pub enum SettingsEntry {
    Header(&'static str),
    Set(Setting),
    /// A rebindable key: activating it captures the next key press.
    Bind(Bind),
}

use crate::binds::Bind;
use SettingsEntry::{Header, Set};

/// The settings page, in order. Grouped under headings so it reads as sections
/// instead of one twenty-row wall.
pub const SETTINGS_PAGE: &[SettingsEntry] = &[
    Header("MOUSE"),
    Set(Setting::Sens),
    Header("VIDEO"),
    Set(Setting::Brightness),
    Set(Setting::ShadowLift),
    Set(Setting::Vsync),
    Header("HUD"),
    Set(Setting::ShowSync),
    Set(Setting::ShowKeys),
    Header("GHOST"),
    Set(Setting::Ghost),
    Set(Setting::GhostTrail),
    Header("MOVEMENT"),
    Set(Setting::Airaccel),
    Header("AUDIO"),
    Set(Setting::Audio),
    Set(Setting::AudioVolume),
    Set(Setting::AudioCore),
    Set(Setting::AudioAir),
    Set(Setting::AudioSub),
    Set(Setting::Wipe),
    Header("KEYBINDS"),
    SettingsEntry::Bind(Bind::Forward),
    SettingsEntry::Bind(Bind::Back),
    SettingsEntry::Bind(Bind::Left),
    SettingsEntry::Bind(Bind::Right),
    SettingsEntry::Bind(Bind::Jump),
    SettingsEntry::Bind(Bind::Duck),
    SettingsEntry::Bind(Bind::TurnLeft),
    SettingsEntry::Bind(Bind::TurnRight),
    Set(Setting::TurnSpeed),
    SettingsEntry::Bind(Bind::Reset),
    SettingsEntry::Bind(Bind::ResetStage),
    SettingsEntry::Bind(Bind::Practice),
];

/// A continuous setting's domain. `log` maps the slider geometrically, which is
/// the only way airaccelerate's 1..1000 is usable as a bar.
pub struct SliderRange {
    pub lo: f32,
    pub hi: f32,
    /// Keyboard / wheel increment, in value units (or in slider fraction when
    /// `log`, where a fixed value step would be useless at one end).
    pub step: f32,
    pub log: bool,
}

impl SliderRange {
    pub const fn linear(lo: f32, hi: f32, step: f32) -> Self {
        Self {
            lo,
            hi,
            step,
            log: false,
        }
    }

    pub fn frac_of(&self, v: f32) -> f32 {
        if self.log {
            let (l, h) = (self.lo.max(1e-6).ln(), self.hi.max(1e-6).ln());
            ((v.max(1e-6).ln() - l) / (h - l)).clamp(0.0, 1.0)
        } else {
            ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
        }
    }

    pub fn value_of(&self, frac: f32) -> f32 {
        let f = frac.clamp(0.0, 1.0);
        if self.log {
            let (l, h) = (self.lo.max(1e-6).ln(), self.hi.max(1e-6).ln());
            (l + (h - l) * f).exp()
        } else {
            self.lo + (self.hi - self.lo) * f
        }
    }

    /// One keyboard / wheel notch from `v`.
    pub fn nudge(&self, v: f32, dir: i32) -> f32 {
        let d = dir as f32;
        if self.log {
            self.value_of(self.frac_of(v) + d * self.step)
        } else {
            (v + d * self.step).clamp(self.lo, self.hi)
        }
    }
}

pub fn slider_range(s: Setting) -> Option<SliderRange> {
    Some(match s {
        Setting::Sens => SliderRange::linear(0.5, 20.0, 0.1),
        Setting::Brightness => SliderRange::linear(0.5, 3.0, 0.05),
        Setting::ShadowLift => SliderRange::linear(0.0, 0.9, 0.05),
        Setting::Airaccel => SliderRange {
            lo: 1.0,
            hi: 1000.0,
            step: 0.02,
            log: true,
        },
        Setting::AudioVolume => SliderRange::linear(0.0, 1.0, 0.02),
        Setting::TurnSpeed => SliderRange::linear(30.0, 720.0, 10.0),
        Setting::AudioCore | Setting::AudioAir | Setting::AudioSub => {
            SliderRange::linear(0.0, 1.5, 0.05)
        }
        _ => return None,
    })
}

/// Sections of the pause menu. Three panels' worth of content, one at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PauseTab {
    Settings,
    Locs,
    Times,
}

impl PauseTab {
    pub const ALL: [PauseTab; 3] = [PauseTab::Settings, PauseTab::Locs, PauseTab::Times];

    pub fn index(self) -> usize {
        PauseTab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Which region of a page the keyboard drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Rows,
    Buttons,
}

/// What activating a row does. Built alongside the row itself so the two can
/// never disagree about which line is which.
#[derive(Clone, Debug, PartialEq)]
pub enum RowAction {
    /// Headers and read-only lines.
    None,
    Adjust(Setting),
    /// Capture the next key press for this bind.
    Rebind(Bind),
    MainResume,
    MainPlay,
    MainLeaderboard,
    MainSettings,
    MainQuit,
    /// Index into `map_list`.
    PickMap(usize),
    /// Open one map's record list.
    OpenBoard(String),
    LocPractice,
    Loc(usize),
    LocLoad,
    LocClear,
}

/// Action-bar entries, in order, for the page that is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    Resume,
    Restart,
    Maps,
    MainMenu,
    Quit,
    Back,
}

impl ButtonAction {
    pub fn label(self) -> &'static str {
        match self {
            ButtonAction::Resume => "Resume",
            ButtonAction::Restart => "Restart",
            ButtonAction::Maps => "Maps",
            ButtonAction::MainMenu => "Main menu",
            ButtonAction::Quit => "Quit",
            ButtonAction::Back => "Back",
        }
    }
}

/// Identity of the page on screen — the key for "where was my cursor on this
/// screen last time". The shell settings page and the pause Settings tab share
/// one slot deliberately: it is the same list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageId {
    Main,
    Picker,
    Settings,
    Board,
    Records,
    Loading,
    Locs,
    Times,
}

/// Per-page selection. Kept out of `App`'s field soup so switching pages is one
/// lookup rather than a scatter of `*_selected` fields that drift apart.
#[derive(Default)]
pub struct Nav {
    main: usize,
    picker: usize,
    settings: usize,
    board: usize,
    records: usize,
    locs: usize,
    times: usize,
}

impl Nav {
    pub fn get(&self, id: PageId) -> usize {
        match id {
            PageId::Main => self.main,
            PageId::Picker => self.picker,
            PageId::Settings => self.settings,
            PageId::Board => self.board,
            PageId::Records => self.records,
            PageId::Locs => self.locs,
            PageId::Times => self.times,
            PageId::Loading => 0,
        }
    }

    pub fn set(&mut self, id: PageId, v: usize) {
        match id {
            PageId::Main => self.main = v,
            PageId::Picker => self.picker = v,
            PageId::Settings => self.settings = v,
            PageId::Board => self.board = v,
            PageId::Records => self.records = v,
            PageId::Locs => self.locs = v,
            PageId::Times => self.times = v,
            PageId::Loading => {}
        }
    }
}

/// Locs panel rows: practice toggle, then one row per loc, then load and
/// clear-all.
pub const LOC_ROW_PRACTICE: usize = 0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Adding a `Setting` variant and forgetting to list it on the page is the
    /// obvious way to lose a control with no compile error and no crash — it
    /// simply never appears. Also catches listing one twice.
    #[test]
    fn every_setting_appears_on_the_page_exactly_once() {
        let all = [
            Setting::Sens,
            Setting::Brightness,
            Setting::ShadowLift,
            Setting::Vsync,
            Setting::ShowSync,
            Setting::ShowKeys,
            Setting::Ghost,
            Setting::GhostTrail,
            Setting::Airaccel,
            Setting::Audio,
            Setting::AudioVolume,
            Setting::AudioCore,
            Setting::AudioAir,
            Setting::AudioSub,
            Setting::Wipe,
            Setting::TurnSpeed,
        ];
        for s in all {
            let n = SETTINGS_PAGE
                .iter()
                .filter(|e| matches!(e, SettingsEntry::Set(x) if *x == s))
                .count();
            assert_eq!(n, 1, "{s:?} appears {n} times on the settings page");
        }
        let listed = SETTINGS_PAGE
            .iter()
            .filter(|e| matches!(e, SettingsEntry::Set(_)))
            .count();
        assert_eq!(
            listed,
            all.len(),
            "page lists a setting not in the test set"
        );
    }

    #[test]
    fn the_page_starts_with_a_header_and_has_no_empty_sections() {
        assert!(matches!(SETTINGS_PAGE[0], SettingsEntry::Header(_)));
        for pair in SETTINGS_PAGE.windows(2) {
            if let (SettingsEntry::Header(a), SettingsEntry::Header(b)) = (&pair[0], &pair[1]) {
                panic!("empty section: {a} is immediately followed by {b}");
            }
        }
        assert!(!matches!(
            SETTINGS_PAGE.last().unwrap(),
            SettingsEntry::Header(_)
        ));
    }

    /// Same trap as the settings: a bind missing from the page has no key the
    /// player can change and no error to say so.
    #[test]
    fn every_bind_appears_on_the_page_exactly_once() {
        for b in Bind::ALL {
            let n = SETTINGS_PAGE
                .iter()
                .filter(|e| matches!(e, SettingsEntry::Bind(x) if *x == b))
                .count();
            assert_eq!(n, 1, "{b:?} appears {n} times on the settings page");
        }
    }

    /// A slider's geometry has to invert exactly, or dragging a knob lands the
    /// value somewhere other than where the bar is drawn.
    #[test]
    fn slider_positions_round_trip_through_their_values() {
        for e in SETTINGS_PAGE {
            let SettingsEntry::Set(s) = e else { continue };
            let Some(r) = slider_range(*s) else { continue };
            for f in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                let back = r.frac_of(r.value_of(f));
                assert!(
                    (back - f).abs() < 1e-3,
                    "{s:?}: frac {f} → {} → {back}",
                    r.value_of(f)
                );
            }
            assert!((r.value_of(0.0) - r.lo).abs() < 1e-3, "{s:?} lo");
            assert!((r.value_of(1.0) - r.hi).abs() < 1e-3, "{s:?} hi");
        }
    }

    #[test]
    fn nudging_a_slider_stays_inside_its_range_at_both_ends() {
        for e in SETTINGS_PAGE {
            let SettingsEntry::Set(s) = e else { continue };
            let Some(r) = slider_range(*s) else { continue };
            let mut v = r.lo;
            for _ in 0..200 {
                v = r.nudge(v, -1);
            }
            assert!(v >= r.lo - 1e-3, "{s:?} nudged below lo: {v}");
            let mut v = r.hi;
            for _ in 0..200 {
                v = r.nudge(v, 1);
            }
            assert!(v <= r.hi + 1e-3, "{s:?} nudged above hi: {v}");
        }
    }

    /// Airaccelerate spans three orders of magnitude; on a linear bar the whole
    /// usable range (aa 100-1000) would sit in the last tenth of the track.
    #[test]
    fn airaccelerate_is_logarithmic_so_the_defaults_are_reachable() {
        let r = slider_range(Setting::Airaccel).unwrap();
        assert!(r.log);
        let momentum = r.frac_of(150.0);
        assert!(
            (0.5..0.85).contains(&momentum),
            "aa 150 (the default feel) sits at {momentum} of the track"
        );
        let css_classic = r.frac_of(100.0);
        assert!(css_classic > 0.5 && css_classic < momentum);
    }

    #[test]
    fn nav_keeps_a_separate_cursor_per_page() {
        let pages = [
            PageId::Main,
            PageId::Picker,
            PageId::Settings,
            PageId::Board,
            PageId::Records,
            PageId::Locs,
            PageId::Times,
        ];
        let mut nav = Nav::default();
        for (i, p) in pages.iter().enumerate() {
            nav.set(*p, i + 1);
        }
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(nav.get(*p), i + 1, "{p:?} lost its cursor");
        }
        // Loading has no list, so it is deliberately a no-op sink.
        nav.set(PageId::Loading, 9);
        assert_eq!(nav.get(PageId::Loading), 0);
    }

    #[test]
    fn the_pause_tabs_index_themselves_consistently() {
        for (i, t) in PauseTab::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
        }
    }
}
