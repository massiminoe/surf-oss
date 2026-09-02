//! Capture the menu pages headlessly, so menu work can be looked at without
//! opening a window.
//!
//! ```text
//! cargo run -p surf-render --example shell_shot --release -- <map.bsp> <out_dir>
//! ```

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::{HudState, MenuPage, MenuPanel, MenuRow, Offscreen, RowTone, ViewParams};

fn rows(v: Vec<MenuRow>) -> MenuPanel {
    MenuPanel {
        rows: v,
        selected: 0,
        hovered: None,
        scroll: 0,
        focused: true,
    }
}

fn settings_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::header("MOUSE"),
        MenuRow::slider("Sensitivity", "5.0", 0.23),
        MenuRow::header("VIDEO"),
        MenuRow::slider("Brightness", "1.00", 0.20),
        MenuRow::slider("Shadow lift", "0.00", 0.0),
        MenuRow::item("VSync", "On"),
        MenuRow::header("HUD"),
        MenuRow::item("Show sync %", "Off"),
        MenuRow::item("Show keys", "Off"),
        MenuRow::header("GHOST"),
        MenuRow::item("Ghost", "Auto (KSF #1 FinCS2 41.475)"),
        MenuRow::item("Ghost trail", "On"),
        MenuRow::header("MOVEMENT"),
        MenuRow::slider("Airaccelerate", "150", 0.73),
        MenuRow::header("AUDIO"),
        MenuRow::item("Audio", "On"),
        MenuRow::slider("Volume", "0.70", 0.7),
        MenuRow::slider("Core level", "1.00", 0.67),
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
        ("kitsune", "—", ""),
        ("lovetunnel", "—", "wr 55.201"),
        ("summit", "44.312", "wr 41.475"),
        ("graybox arena", "—", ""),
    ]
    .iter()
    .map(|(a, b, note)| MenuRow::item(*a, *b).with_note(*note))
    .collect();

    let board_rows: Vec<MenuRow> = vec![
        MenuRow::header("WORLD RECORDS · KSF"),
        MenuRow::text("#1  FinCS2", "41.475"),
        MenuRow::text("#2  jonkler", "41.910"),
        MenuRow::text("#3  vaporeon", "42.203"),
        MenuRow::header("YOUR TIMES"),
        MenuRow::text("PB", "44.312")
            .with_note("+2.837 vs wr")
            .with_tone(RowTone::Accent),
        MenuRow::text("run 1", "44.312")
            .with_note("pb")
            .with_tone(RowTone::Good),
        MenuRow::text("run 2", "46.008").with_tone(RowTone::Dim),
    ];

    let pages: Vec<(&str, MenuPage)> = vec![
        (
            "main_menu",
            MenuPage {
                title: "MX-SURF".into(),
                subtitle: "source-faithful surf · single player".into(),
                panel: rows(vec![
                    MenuRow::item("Resume", "summit").with_tone(RowTone::Accent),
                    MenuRow::item("Play", ""),
                    MenuRow::item("Leaderboard", ""),
                    MenuRow::item("Settings", ""),
                    MenuRow::item("Quit", ""),
                ]),
                hint: "↑↓ select   enter choose   click anywhere".into(),
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "map_picker",
            MenuPage {
                title: "SELECT MAP".into(),
                subtitle: "24 available".into(),
                panel: MenuPanel {
                    rows: map_rows,
                    selected: 10,
                    hovered: Some(5),
                    scroll: 0,
                    focused: true,
                },
                buttons: vec!["Back".into()],
                hint: "↑↓ select   enter load   esc back".into(),
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "shell_settings",
            MenuPage {
                title: "SETTINGS".into(),
                subtitle: "saved on close".into(),
                panel: MenuPanel {
                    rows: settings_rows(),
                    selected: 1,
                    hovered: None,
                    scroll: 0,
                    focused: true,
                },
                buttons: vec!["Back".into()],
                hint: "←→ or drag adjusts   wheel over a row   esc back".into(),
                backdrop: true,
                ..Default::default()
            },
        ),
        (
            "leaderboard",
            MenuPage {
                title: "SUMMIT".into(),
                subtitle: "ksf records · your runs".into(),
                panel: rows(board_rows.clone()),
                buttons: vec!["Back".into()],
                hint: "esc back".into(),
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
                hint: "1.4s elapsed".into(),
                backdrop: true,
                busy: true,
                ..Default::default()
            },
        ),
        (
            "pause_settings",
            MenuPage {
                title: "SUMMIT".into(),
                subtitle: "paused · pb 44.312".into(),
                tabs: vec!["SETTINGS".into(), "LOCS".into(), "TIMES".into()],
                tab: 0,
                panel: MenuPanel {
                    rows: settings_rows(),
                    selected: 3,
                    hovered: Some(5),
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
                hint: "←→ or drag adjusts   wheel over a row   tab: actions".into(),
                ..Default::default()
            },
        ),
        (
            "pause_locs",
            MenuPage {
                title: "SUMMIT".into(),
                subtitle: "paused · pb 44.312".into(),
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
                hint: "click picks · enter loads · x deletes".into(),
                ..Default::default()
            },
        ),
        (
            "pause_times",
            MenuPage {
                title: "SUMMIT".into(),
                subtitle: "paused · pb 44.312".into(),
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
                hint: "esc resumes".into(),
                wide: true,
                ..Default::default()
            },
        ),
    ];

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
