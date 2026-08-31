//! Capture the shell pages (title screen, map picker) headlessly, so menu work
//! can be looked at without opening a window.
//!
//! ```text
//! cargo run -p surf-render --example shell_shot --release -- <map.bsp> <out_dir>
//! ```

use surf_core::math::Vec3;
use surf_map::LoadedMap;
use surf_render::{HudState, Offscreen, ShellHud, ViewParams};

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

    let pages: Vec<(&str, ShellHud)> = vec![
        (
            "main_menu",
            ShellHud {
                title: "MX-SURF".into(),
                subtitle: "source-faithful surf".into(),
                items: vec![
                    ("Play".into(), String::new()),
                    ("Quit".into(), String::new()),
                ],
                selected: 0,
                hovered: None,
                hint: "↑↓ select   enter choose   esc quit".into(),
                message: None,
                scroll: 0,
            },
        ),
        (
            "map_picker",
            ShellHud {
                title: "SELECT MAP".into(),
                subtitle: "24 available".into(),
                items: [
                    ("andromeda", "—"),
                    ("aquaflow", "—"),
                    ("boreas", "1:07.443"),
                    ("botanica", "—"),
                    ("cement", "—"),
                    ("cyberwave", "52.106"),
                    ("demise", "—"),
                    ("frost", "—"),
                    ("kitsune", "—"),
                    ("lovetunnel", "—"),
                    ("summit", "44.312"),
                    ("graybox arena", "—"),
                ]
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
                selected: 10,
                hovered: Some(5),
                hint: "↑↓ select   enter load   esc back".into(),
                message: None,
                scroll: 0,
            },
        ),
        (
            "loading",
            ShellHud {
                title: "LOADING".into(),
                subtitle: "surf_boreas".into(),
                items: Vec::new(),
                selected: 0,
                hovered: None,
                hint: "1.4s".into(),
                message: None,
                scroll: 0,
            },
        ),
    ];

    for (name, shell) in pages {
        let hud = HudState {
            shell: Some(shell),
            ..HudState::default()
        };
        let rgba = off.capture_with_hud(eye, angles, ViewParams::default(), Some(hud));
        let path = out_dir.join(format!("{name}.png"));
        image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
        println!("wrote {}", path.display());
    }
}
