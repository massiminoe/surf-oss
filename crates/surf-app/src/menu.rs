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
    Set(Setting),
    /// A rebindable key: activating it captures the next key press.
    Bind(Bind),
}

use crate::binds::Bind;
use SettingsEntry::{Bind as Key, Set};

/// A family of settings. The settings screen lists these; picking one opens
/// its rows. One page of twenty-odd rows was a scroll, not a menu (Max,
/// 2026-09-06).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Mouse,
    Video,
    Hud,
    Ghost,
    Movement,
    Audio,
    Keybinds,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::Mouse,
        Section::Video,
        Section::Hud,
        Section::Ghost,
        Section::Movement,
        Section::Audio,
        Section::Keybinds,
    ];

    pub fn index(self) -> usize {
        Section::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Row label on the section list, and the title once inside it.
    pub fn label(self) -> &'static str {
        match self {
            Section::Mouse => "Mouse",
            Section::Video => "Video",
            Section::Hud => "HUD",
            Section::Ghost => "Ghost",
            Section::Movement => "Movement",
            Section::Audio => "Audio",
            Section::Keybinds => "Keybinds",
        }
    }

    /// What is inside, so the list is navigable without opening each one.
    pub fn summary(self) -> &'static str {
        match self {
            Section::Mouse => "sensitivity",
            Section::Video => "brightness, shadows, vsync",
            Section::Hud => "on-screen readouts",
            Section::Ghost => "replay ghost and trail",
            Section::Movement => "airaccelerate",
            Section::Audio => "levels and wipe style",
            Section::Keybinds => "movement keys, turn speed",
        }
    }

    pub fn entries(self) -> &'static [SettingsEntry] {
        match self {
            Section::Mouse => &[Set(Setting::Sens)],
            Section::Video => &[
                Set(Setting::Brightness),
                Set(Setting::ShadowLift),
                Set(Setting::Vsync),
            ],
            Section::Hud => &[Set(Setting::ShowKeys)],
            Section::Ghost => &[Set(Setting::Ghost), Set(Setting::GhostTrail)],
            Section::Movement => &[Set(Setting::Airaccel)],
            Section::Audio => &[
                Set(Setting::Audio),
                Set(Setting::AudioVolume),
                Set(Setting::AudioCore),
                Set(Setting::AudioAir),
                Set(Setting::AudioSub),
                Set(Setting::Wipe),
            ],
            Section::Keybinds => &[
                Key(Bind::Forward),
                Key(Bind::Back),
                Key(Bind::Left),
                Key(Bind::Right),
                Key(Bind::Jump),
                Key(Bind::Duck),
                Key(Bind::TurnLeft),
                Key(Bind::TurnRight),
                Set(Setting::TurnSpeed),
                Key(Bind::Reset),
                Key(Bind::ResetStage),
                Key(Bind::Practice),
            ],
        }
    }
}

/// Every settings row on every section, in order. Only the tests and anything
/// that needs the whole table should use this — the screen shows one section.
pub fn all_entries() -> impl Iterator<Item = &'static SettingsEntry> {
    Section::ALL.iter().flat_map(|s| s.entries().iter())
}

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
        // CS:S sensitivity: the same number `sensitivity` takes in Source, and
        // 1..5 is where nearly every player lives.
        Setting::Sens => SliderRange::linear(0.1, 10.0, 0.05),
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

/// How a continuous setting's number reads — and therefore what a text field
/// is seeded with. No units: the field parses back exactly what it shows.
pub fn value_text(s: Setting, v: f32) -> String {
    match s {
        Setting::Airaccel | Setting::TurnSpeed => format!("{v:.0}"),
        _ => format!("{v:.2}"),
    }
}

/// A typed value, clamped into the setting's range. `None` for anything that
/// is not a finite number, so a half-typed or empty field leaves the value
/// alone rather than writing a zero.
pub fn parse_value(s: Setting, text: &str) -> Option<f32> {
    let v: f32 = text.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(match slider_range(s) {
        Some(r) => v.clamp(r.lo, r.hi),
        None => v,
    })
}

/// An open text field on a numeric settings row.
///
/// The buffer is free text while you type and is only parsed on commit, so
/// "1." is a legal intermediate state. It lives here rather than in the event
/// loop so the editing rules are testable.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueEdit {
    pub setting: Setting,
    buf: String,
}

impl ValueEdit {
    /// Longer than any setting's range needs; past this it can only be a typo.
    const MAX_LEN: usize = 8;

    /// Open a field seeded with what the row currently reads.
    pub fn new(setting: Setting, value: f32) -> Self {
        Self {
            setting,
            buf: value_text(setting, value),
        }
    }

    pub fn text(&self) -> &str {
        &self.buf
    }

    /// Accept one typed character. Anything that could not appear in a number
    /// is ignored, as is a second decimal point.
    pub fn push(&mut self, ch: char) {
        let ok = match ch {
            '0'..='9' => true,
            '.' => !self.buf.contains('.'),
            // A leading minus only: no setting has a negative range today, but
            // typing one should not produce "1-2".
            '-' => self.buf.is_empty(),
            _ => false,
        };
        if ok && self.buf.len() < Self::MAX_LEN {
            self.buf.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        self.buf.pop();
    }

    /// The value to write, or `None` to leave the setting alone.
    pub fn commit(&self) -> Option<f32> {
        parse_value(self.setting, &self.buf)
    }
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
    /// Open one settings family.
    OpenSection(Section),
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
    /// Watch a replay — a KSF record, the PB, or one of your runs.
    Watch(crate::watch::ReplayPick),
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
    /// The list of settings families.
    Settings,
    /// One family's rows.
    Section(Section),
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
    /// One cursor per settings family, so backing out and re-entering lands
    /// where you left off.
    section: [usize; Section::ALL.len()],
}

impl Nav {
    pub fn get(&self, id: PageId) -> usize {
        match id {
            PageId::Main => self.main,
            PageId::Picker => self.picker,
            PageId::Settings => self.settings,
            PageId::Section(sec) => self.section[sec.index()],
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
            PageId::Section(sec) => self.section[sec.index()] = v,
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

    /// Adding a `Setting` variant and forgetting to list it in a section is the
    /// obvious way to lose a control with no compile error and no crash — it
    /// simply never appears. Also catches listing one twice.
    #[test]
    fn every_setting_appears_in_exactly_one_section() {
        let all = [
            Setting::Sens,
            Setting::Brightness,
            Setting::ShadowLift,
            Setting::Vsync,
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
            let n = all_entries()
                .filter(|e| matches!(e, SettingsEntry::Set(x) if *x == s))
                .count();
            assert_eq!(n, 1, "{s:?} appears in {n} sections");
        }
        let listed = all_entries()
            .filter(|e| matches!(e, SettingsEntry::Set(_)))
            .count();
        assert_eq!(
            listed,
            all.len(),
            "a section lists a setting not in the test set"
        );
    }

    /// A section with nothing in it is a dead end the cursor can still reach.
    #[test]
    fn no_section_is_empty() {
        for sec in Section::ALL {
            assert!(
                !sec.entries().is_empty(),
                "{sec:?} ({}) has no rows",
                sec.label()
            );
            assert!(!sec.label().is_empty() && !sec.summary().is_empty());
        }
    }

    #[test]
    fn sections_index_themselves_consistently() {
        for (i, sec) in Section::ALL.iter().enumerate() {
            assert_eq!(sec.index(), i);
        }
    }

    /// Same trap as the settings: a bind missing from every section has no key
    /// the player can change and no error to say so.
    #[test]
    fn every_bind_appears_in_exactly_one_section() {
        for b in Bind::ALL {
            let n = all_entries()
                .filter(|e| matches!(e, SettingsEntry::Bind(x) if *x == b))
                .count();
            assert_eq!(n, 1, "{b:?} appears in {n} sections");
        }
    }

    /// A slider's geometry has to invert exactly, or dragging a knob lands the
    /// value somewhere other than where the bar is drawn.
    #[test]
    fn slider_positions_round_trip_through_their_values() {
        for e in all_entries() {
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
        for e in all_entries() {
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

    /// Sensitivity is CS:S's own `sensitivity` number (our per-count scale is
    /// Source's `m_yaw`/`m_pitch` default of 0.022), so the track has to put
    /// the band real players use — roughly 1 to 5 — in reach, not at one end.
    #[test]
    fn sensitivity_covers_the_range_a_source_player_would_type() {
        let r = slider_range(Setting::Sens).unwrap();
        assert!(!r.log);
        assert!(r.lo <= 0.1 && r.hi >= 10.0, "sens range {}..{}", r.lo, r.hi);
        for v in [1.0f32, 2.0, 3.0, 5.0] {
            let f = r.frac_of(v);
            assert!(
                (0.0..=0.55).contains(&f),
                "sens {v} sits at {f} of the track"
            );
        }
        // Fine enough to land on a two-decimal value from the keyboard.
        assert!(r.step <= 0.05);
    }

    /// A field is seeded with what the row shows, so typing nothing and
    /// pressing enter has to give back the value that was already there. If
    /// the display rounds harder than the field parses, every open-and-close
    /// nudges the setting.
    #[test]
    fn opening_a_field_and_committing_it_unchanged_keeps_the_value() {
        for e in all_entries() {
            let SettingsEntry::Set(s) = e else { continue };
            let Some(r) = slider_range(*s) else { continue };
            for f in [0.0f32, 0.13, 0.5, 0.77, 1.0] {
                let v = r.value_of(f);
                let got = ValueEdit::new(*s, v).commit().unwrap();
                // Within half a display step of what the row showed.
                let tol = if matches!(s, Setting::Airaccel | Setting::TurnSpeed) {
                    0.5
                } else {
                    0.005
                };
                assert!(
                    (got - v).abs() <= tol,
                    "{s:?}: showed {} for {v}, read back {got}",
                    value_text(*s, v)
                );
            }
        }
    }

    #[test]
    fn a_typed_value_is_clamped_into_the_settings_range() {
        for e in all_entries() {
            let SettingsEntry::Set(s) = e else { continue };
            let Some(r) = slider_range(*s) else { continue };
            assert_eq!(parse_value(*s, "999999"), Some(r.hi), "{s:?} above range");
            assert_eq!(parse_value(*s, "-999999"), Some(r.lo), "{s:?} below range");
        }
    }

    /// A field that writes a zero when you clear it would silently mute the
    /// audio or drop sensitivity to nothing.
    #[test]
    fn an_unfinished_field_commits_nothing() {
        for text in ["", " ", ".", "-", "abc", "1.2.3", "inf", "NaN"] {
            assert_eq!(
                parse_value(Setting::Sens, text),
                None,
                "{text:?} parsed as a value"
            );
        }
    }

    #[test]
    fn a_field_only_accepts_characters_that_can_appear_in_a_number() {
        let mut e = ValueEdit::new(Setting::Sens, 2.57);
        assert_eq!(e.text(), "2.57");
        e.backspace();
        e.backspace();
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "");
        for ch in "1.2.3abc-4".chars() {
            e.push(ch);
        }
        assert_eq!(e.text(), "1.234");
        // And it cannot grow without bound.
        for _ in 0..40 {
            e.push('9');
        }
        assert_eq!(e.text().len(), ValueEdit::MAX_LEN);
    }

    /// The point of splitting settings into families is that a family is a
    /// page, not a scroll (Max, 2026-09-06). Checked against the tightest
    /// layout we draw: the pause overlay, which spends room on a tab strip and
    /// a five-button action bar, at a small window.
    #[test]
    fn every_settings_family_fits_on_one_page_without_scrolling() {
        for (w, h) in [(1280.0f32, 720.0f32), (1280.0, 800.0), (1512.0, 982.0)] {
            for sec in Section::ALL {
                let rows = sec.entries().len();
                let layout = surf_render::page_layout(
                    w,
                    h,
                    surf_render::PageSpec {
                        rows,
                        tabs: PauseTab::ALL.len(),
                        buttons: 5,
                        wide: false,
                        large: false,
                    },
                );
                assert!(
                    layout.panel.rows >= rows,
                    "{} has {rows} rows but only {} fit at {w}x{h}",
                    sec.label(),
                    layout.panel.rows
                );
            }
        }
    }

    #[test]
    fn nav_keeps_a_separate_cursor_per_page() {
        let mut pages = vec![
            PageId::Main,
            PageId::Picker,
            PageId::Settings,
            PageId::Board,
            PageId::Records,
            PageId::Locs,
            PageId::Times,
        ];
        pages.extend(Section::ALL.iter().map(|s| PageId::Section(*s)));
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
