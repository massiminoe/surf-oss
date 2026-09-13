//! The command-line setup path must not reach the normal profile during a fresh test.
use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn isolated_check_does_not_migrate_or_modify_a_legacy_profile() {
    let root = std::env::temp_dir().join(format!(
        "surf-setup-isolation-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let legacy = root.join("home/Library/Application Support/mx-surf");
    fs::create_dir_all(&legacy).unwrap();
    let sentinel = legacy.join("settings.json");
    fs::write(&sentinel, "keep this profile").unwrap();
    let profile = root.join("fresh profile");
    let output = Command::new(env!("CARGO_BIN_EXE_surf-oss"))
        .args(["--check", "--data-dir"])
        .arg(&profile)
        .arg("--game-dir")
        .arg(root.join("missing CS:S"))
        .env("HOME", root.join("home"))
        .env("SURF_OSS_ASSETS", &legacy)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing CS:S"));
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep this profile");
    assert!(!root
        .join("home/Library/Application Support/surf-oss")
        .exists());
    assert!(
        !profile.exists(),
        "read-only check must not create a profile"
    );
    // Retained for inspection: no cleanup touches other local data.
}

#[test]
fn help_is_available_without_steam_or_a_profile() {
    let output = Command::new(env!("CARGO_BIN_EXE_surf-oss"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    for option in ["--setup", "--check", "--data-dir", "--game-dir", "16 maps"] {
        assert!(text.contains(option));
    }
}
