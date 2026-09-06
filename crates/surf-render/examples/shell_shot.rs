//! Capture the menu pages headlessly, so menu work can be looked at without
//! opening a window.
//!
//! ```text
//! cargo run -p surf-render --example shell_shot --release -- <map.bsp> <out_dir>
//! ```

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::ReplayHud;
use surf_render::{
    HudState, HudTimerPhase, MenuPage, MenuPanel, MenuRow, Offscreen, RowTone, ShowKeysState,
    ViewParams,
};

fn rows(v: Vec<MenuRow>) -> MenuPanel {
    MenuPanel {
        rows: v,
        selected: 0,
        hovered: None,
        scroll: 0,
        focused: true,
    }
}

/// The settings screen is a list of families now, not one long table.
fn settings_families() -> Vec<MenuRow> {
    vec![
        MenuRow::item("Mouse", ">").with_note("sensitivity"),
        MenuRow::item("Video", ">").with_note("brightness, shadows, vsync"),
        MenuRow::item("HUD", ">").with_note("on-screen readouts"),
        MenuRow::item("Ghost", ">").with_note("replay ghost and trail"),
        MenuRow::item("Movement", ">").with_note("airaccelerate"),
        MenuRow::item("Audio", ">").with_note("levels and wipe style"),
        MenuRow::item("Keybinds", ">").with_note("movement keys, turn speed"),
    ]
}

/// One family, with a value field open — the case the field box has to look
/// right in.
fn audio_section() -> Vec<MenuRow> {
    vec![
        MenuRow::item("Audio", "On"),
        MenuRow::slider("Volume", "0.7_", 0.72)
            .with_tone(RowTone::Accent)
            .editing(),
        MenuRow::slider("Core level", "0.43", 0.29),
        MenuRow::slider("Air level", "0.53", 0.35),
        MenuRow::slider("Sub level", "0.58", 0.39),
        MenuRow::item("Wipe sound", "Rewind"),
    ]
}

fn keybinds_section() -> Vec<MenuRow> {
    vec![
        MenuRow::item("Forward", "W"),
        MenuRow::item("Back", "S"),
        MenuRow::item("Left", "A"),
        MenuRow::item("Right", "D"),
        MenuRow::item("Jump", "Space"),
        MenuRow::item("Duck", "L Ctrl"),
        MenuRow::item("Turn left", "Q"),
        MenuRow::item("Turn right", "press a key").with_tone(RowTone::Warn),
        MenuRow::slider("Turn speed", "210°/s", 0.26),
        MenuRow::item("Reset", "R"),
        MenuRow::item("Reset stage", "T"),
        MenuRow::item("Practice", "P"),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: shell_shot <map.bsp> <out_dir>");
        std::process::exit(2);
    }
    let map = LoadedMap::load_path(std::path::Path::new(&args[0])).expect("map");
    let out_dir = std::path::PathBuf::from(&args[1]);
    std::fs::create_dir_all(&out_dir).expect("out dir");

    let (w, h) = (1280u32, 800u32);
    let mut off = Offscreen::new(&map, w, h).expect("offscreen");
    let eye = map.spawn_origin + Vec3::new(0.0, 0.0, 64.0);
    let angles = map.spawn_angles;

    let map_rows: Vec<MenuRow> = [
        ("andromeda", "—", ""),
        ("aquaflow", "—", ""),
        ("boreas", "1:07.443", "wr 1:02.118"),
        ("botanica", "—", ""),
        ("cement", "—", ""),
        ("cyberwave", "52.106", "wr 48.900"),
        ("demise", "—", ""),
        ("frost", "—", "wr 1:21.550"),
        ("lovetunnel", "—", "wr 55.201"),
        ("summit", "44.312", "wr 41.475"),
    ]
    .iter()
    .map(|(a, b, note)| MenuRow::item(*a, *b).with_note(*note))
    .collect();

    let board_rows: Vec<MenuRow> = vec![
        MenuRow::header("WORLD RECORDS · KSF"),
        MenuRow::text("#1  FinCS2", "41.475").with_note("replay"),
        MenuRow::text("#2  jonkler", "41.910").with_note("replay"),
        MenuRow::text("#3  vaporeon", "42.203"),
        MenuRow::header("YOUR TIMES"),
        MenuRow::text("PB", "44.312")
            .with_note("+2.837 vs wr · replay")
            .with_tone(RowTone::Accent),
        MenuRow::text("run 1", "44.312")
            .with_note("pb · replay")
            .with_tone(RowTone::Good),
        MenuRow::text("run 2", "46.008")
            .with_note("replay")
            .with_tone(RowTone::Dim),
    ];

    let pages: Vec<(&str, MenuPage)> = vec![
        (
            "main_menu",
            MenuPage {
                title: "MX-SURF".into(),
                panel: rows(vec![
                    MenuRow::item("Resume", "summit").with_tone(RowTone::Accent),
                    MenuRow::item("Play", ""),
                    MenuRow::item("Leaderboard", ""),
                    MenuRow::item("Settings", ""),
                    MenuRow::item("Quit", ""),
                ]),
                backdrop: true,
                large: true,
                ..Default::default()
            },
        ),
        (
            "map_picker",
            MenuPage {
                title: "SELECT MAP".into(),
                panel: MenuPanel {
                    rows: map_rows,
                    selected: 10,
                    hovered: Some(5),
                    scroll: 0,
                    focused: true,
                },
                buttons: vec!["Back".into()],
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "shell_settings",
            MenuPage {
                title: "SETTINGS".into(),
                panel: MenuPanel {
                    rows: settings_families(),
                    selected: 5,
                    hovered: Some(1),
                    scroll: 0,
                    focused: true,
                },
                buttons: vec!["Back".into()],
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "shell_settings_section",
            MenuPage {
                title: "AUDIO".into(),
                subtitle: "type a value · enter accepts · esc cancels".into(),
                panel: MenuPanel {
                    rows: audio_section(),
                    selected: 1,
                    hovered: None,
                    scroll: 0,
                    focused: true,
                },
                buttons: vec!["Back".into()],
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "leaderboard",
            MenuPage {
                title: "SUMMIT".into(),
                panel: rows(board_rows.clone()),
                buttons: vec!["Back".into()],
                wide: true,
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "loading",
            MenuPage {
                title: "LOADING".into(),
                subtitle: "surf_boreas".into(),
                backdrop: true,
                busy: true,
                ..Default::default()
            },
        ),
        (
            "pause_settings",
            MenuPage {
                title: "SUMMIT".into(),
                subtitle: "press a key · esc cancels".into(),
                tabs: vec!["SETTINGS".into(), "LOCS".into(), "TIMES".into()],
                tab: 0,
                panel: MenuPanel {
                    rows: keybinds_section(),
                    selected: 7,
                    hovered: Some(8),
                    scroll: 0,
                    focused: true,
                },
                buttons: vec![
                    "Resume".into(),
                    "Restart".into(),
                    "Maps".into(),
                    "Main menu".into(),
                    "Quit".into(),
                ],
                ..Default::default()
            },
        ),
        (
            "pause_locs",
            MenuPage {
                title: "SUMMIT".into(),
                tabs: vec!["SETTINGS".into(), "LOCS".into(), "TIMES".into()],
                tab: 1,
                panel: MenuPanel {
                    rows: vec![
                        MenuRow::item("Practice mode", "On").with_tone(RowTone::Warn),
                        MenuRow::item("  #1", "3.210").with_note("1204 u/s"),
                        MenuRow::item("▸ #2", "11.480").with_note("2871 u/s"),
                        MenuRow::item("  #3", "19.902").with_note("3344 u/s"),
                        MenuRow::item("Load selected", "enter"),
                        MenuRow::item("Clear all", "x"),
                    ],
                    selected: 2,
                    hovered: None,
                    scroll: 0,
                    focused: true,
                },
                buttons: vec![
                    "Resume".into(),
                    "Restart".into(),
                    "Maps".into(),
                    "Main menu".into(),
                    "Quit".into(),
                ],
                ..Default::default()
            },
        ),
        (
            "pause_times",
            MenuPage {
                title: "SUMMIT".into(),
                tabs: vec!["SETTINGS".into(), "LOCS".into(), "TIMES".into()],
                tab: 2,
                panel: rows(board_rows),
                buttons: vec![
                    "Resume".into(),
                    "Restart".into(),
                    "Maps".into(),
                    "Main menu".into(),
                    "Quit".into(),
                ],
                wide: true,
                ..Default::default()
            },
        ),
    ];

    // In-run HUD, no page: the readout the player actually surfs with.
    {
        let hud = HudState {
            speed: 1842.0,
            grounded: false,
            time_secs: Some(31.42),
            timer_phase: HudTimerPhase::Running,
            show_keys: Some(ShowKeysState {
                forward: true,
                left: true,
                ..Default::default()
            }),
            stage_line: Some("STAGE 2/4".into()),
            cp_label: Some("CP3".into()),
            cp_delta_secs: Some(-0.184),
            ghost_speed_delta: Some(62.0),
            time: 3.0,
            ..HudState::default()
        };
        let rgba = off.capture_with_hud(eye, angles, ViewParams::default(), Some(hud));
        let path = out_dir.join("hud_run.png");
        image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
        println!("wrote {}", path.display());
    }

    // Replay viewer HUD: the caption, the recorded keys, the scrub bar with
    // its checkpoint ticks.
    {
        let hud = HudState {
            speed: 2210.0,
            time_secs: Some(18.765),
            timer_phase: HudTimerPhase::Running,
            show_keys: Some(ShowKeysState {
                forward: true,
                right: true,
                ..Default::default()
            }),
            cp_label: Some("CP2".into()),
            replay: Some(ReplayHud {
                label: "KSF #1 FinCS2".into(),
                transport: "1x".into(),
                progress: 18.765 / 41.475,
                total_secs: 41.475,
                split_marks: vec![0.21, 0.44, 0.63, 0.85],
                chase: false,
            }),
            time: 3.0,
            ..HudState::default()
        };
        let rgba = off.capture_with_hud(eye, angles, ViewParams::default(), Some(hud));
        let path = out_dir.join("hud_replay.png");
        image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
        println!("wrote {}", path.display());
    }

    for (name, page) in pages {
        let hud = HudState {
            page: Some(page),
            // A fixed clock so a capture is reproducible: the backdrop and the
            // busy bar are both animated.
            time: 3.0,
            ..HudState::default()
        };
        let rgba = off.capture_with_hud(eye, angles, ViewParams::default(), Some(hud));
        let path = out_dir.join(format!("{name}.png"));
        image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
        println!("wrote {}", path.display());
    }
}
