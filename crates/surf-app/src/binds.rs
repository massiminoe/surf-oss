//! Rebindable keys (Max, 2026-09-03): a KEYBINDS section in settings, and
//! Source-style turn binds (`+left` / `+right`, Q / E by default).
//!
//! Lives in the library so the table, the default set and the name
//! round-trip are unit-tested. `main.rs` only asks "which key is this bind on"
//! and "which bind is this key on" — a bind's position in the list is never
//! load-bearing.
//!
//! Serialised as `{ "forward": "KeyW", … }` in `settings.json`, keyed by the
//! bind's stable name and holding winit's `KeyCode` debug name. A key we do
//! not list in [`KEYS`] cannot be bound (the capture step refuses it), so a
//! stored name always parses back.

use std::collections::BTreeMap;

use winit::keyboard::KeyCode;

/// Every action a key can be bound to, in menu order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bind {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Duck,
    TurnLeft,
    TurnRight,
    Reset,
    ResetStage,
    Practice,
}

impl Bind {
    pub const ALL: [Bind; 11] = [
        Bind::Forward,
        Bind::Back,
        Bind::Left,
        Bind::Right,
        Bind::Jump,
        Bind::Duck,
        Bind::TurnLeft,
        Bind::TurnRight,
        Bind::Reset,
        Bind::ResetStage,
        Bind::Practice,
    ];

    /// Stable key in `settings.json`. Never rename one — it would silently
    /// reset that bind for every existing file.
    pub fn name(self) -> &'static str {
        match self {
            Bind::Forward => "forward",
            Bind::Back => "back",
            Bind::Left => "left",
            Bind::Right => "right",
            Bind::Jump => "jump",
            Bind::Duck => "duck",
            Bind::TurnLeft => "turn_left",
            Bind::TurnRight => "turn_right",
            Bind::Reset => "reset",
            Bind::ResetStage => "reset_stage",
            Bind::Practice => "practice",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Bind::Forward => "Forward",
            Bind::Back => "Back",
            Bind::Left => "Strafe left",
            Bind::Right => "Strafe right",
            Bind::Jump => "Jump",
            Bind::Duck => "Duck",
            Bind::TurnLeft => "Turn left",
            Bind::TurnRight => "Turn right",
            Bind::Reset => "Restart run",
            Bind::ResetStage => "Restart stage",
            Bind::Practice => "Practice mode",
        }
    }

    pub fn default_key(self) -> KeyCode {
        match self {
            Bind::Forward => KeyCode::KeyW,
            Bind::Back => KeyCode::KeyS,
            Bind::Left => KeyCode::KeyA,
            Bind::Right => KeyCode::KeyD,
            Bind::Jump => KeyCode::Space,
            Bind::Duck => KeyCode::ControlLeft,
            Bind::TurnLeft => KeyCode::KeyQ,
            Bind::TurnRight => KeyCode::KeyE,
            Bind::Reset => KeyCode::KeyR,
            Bind::ResetStage => KeyCode::KeyT,
            Bind::Practice => KeyCode::KeyP,
        }
    }

    fn index(self) -> usize {
        Bind::ALL.iter().position(|b| *b == self).unwrap_or(0)
    }
}

/// Source `cl_yawspeed` default, degrees per second while a turn bind is held.
pub const DEFAULT_TURN_SPEED: f32 = 210.0;

/// Keys that can be bound, with the short label the menu shows. Escape is
/// deliberately absent: it always backs out of a page, including the capture
/// prompt itself.
pub const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::Space, "Space"),
    (KeyCode::ShiftLeft, "L Shift"),
    (KeyCode::ShiftRight, "R Shift"),
    (KeyCode::ControlLeft, "L Ctrl"),
    (KeyCode::ControlRight, "R Ctrl"),
    (KeyCode::AltLeft, "L Alt"),
    (KeyCode::AltRight, "R Alt"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::CapsLock, "Caps"),
    (KeyCode::Enter, "Enter"),
    (KeyCode::Backspace, "Backspace"),
    (KeyCode::ArrowUp, "Up"),
    (KeyCode::ArrowDown, "Down"),
    (KeyCode::ArrowLeft, "Left"),
    (KeyCode::ArrowRight, "Right"),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Backquote, "`"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="),
    (KeyCode::Insert, "Ins"),
    (KeyCode::Delete, "Del"),
    (KeyCode::Home, "Home"),
    (KeyCode::End, "End"),
    (KeyCode::PageUp, "PgUp"),
    (KeyCode::PageDown, "PgDn"),
    (KeyCode::Numpad0, "Num 0"),
    (KeyCode::Numpad1, "Num 1"),
    (KeyCode::Numpad2, "Num 2"),
    (KeyCode::Numpad3, "Num 3"),
    (KeyCode::Numpad4, "Num 4"),
    (KeyCode::Numpad5, "Num 5"),
    (KeyCode::Numpad6, "Num 6"),
    (KeyCode::Numpad7, "Num 7"),
    (KeyCode::Numpad8, "Num 8"),
    (KeyCode::Numpad9, "Num 9"),
    (KeyCode::NumpadEnter, "Num Enter"),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
];

/// Short label for a key, or `None` when it is not bindable.
pub fn key_label(code: KeyCode) -> Option<&'static str> {
    KEYS.iter().find(|(k, _)| *k == code).map(|(_, l)| *l)
}

/// Stable on-disk name (winit's variant name).
pub fn key_name(code: KeyCode) -> String {
    format!("{code:?}")
}

pub fn parse_key(name: &str) -> Option<KeyCode> {
    KEYS.iter()
        .map(|(k, _)| *k)
        .find(|k| key_name(*k) == name)
}

/// The full bind table. One key per bind, always populated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binds {
    keys: [KeyCode; Bind::ALL.len()],
}

impl Default for Binds {
    fn default() -> Self {
        let mut keys = [KeyCode::KeyW; Bind::ALL.len()];
        for b in Bind::ALL {
            keys[b.index()] = b.default_key();
        }
        Self { keys }
    }
}

impl Binds {
    pub fn key(&self, b: Bind) -> KeyCode {
        self.keys[b.index()]
    }

    /// The bind a key drives, if any.
    pub fn bind_of(&self, code: KeyCode) -> Option<Bind> {
        Bind::ALL.iter().copied().find(|b| self.key(*b) == code)
    }

    /// Put `b` on `code`. A key already driving another bind swaps: that bind
    /// takes `b`'s old key, so nothing is ever left unbound and nothing fires
    /// two actions. Returns `false` (and changes nothing) for an unbindable key.
    pub fn set(&mut self, b: Bind, code: KeyCode) -> bool {
        if key_label(code).is_none() {
            return false;
        }
        let old = self.key(b);
        if let Some(other) = self.bind_of(code) {
            if other != b {
                self.keys[other.index()] = old;
            }
        }
        self.keys[b.index()] = code;
        true
    }

    /// `settings.json` form. Every bind is written, so a file always shows the
    /// full table rather than just the ones that differ from default.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        Bind::ALL
            .iter()
            .map(|b| (b.name().to_string(), key_name(self.key(*b))))
            .collect()
    }

    /// Defaults, overridden by whatever in `map` parses. A missing or unknown
    /// entry keeps the default — an old file must not lose its movement keys
    /// because a new bind was added.
    pub fn from_map(map: &BTreeMap<String, String>) -> Self {
        let mut out = Self::default();
        for b in Bind::ALL {
            if let Some(code) = map.get(b.name()).and_then(|s| parse_key(s)) {
                out.keys[b.index()] = code;
            }
        }
        out
    }
}

/// Keyboard turning, applied once per simulation tick before the command is
/// built so the recorded viewangles carry it. `+left` adds yaw (Source: yaw
/// increases counter-clockwise), `+right` subtracts.
pub fn turn_delta(left: bool, right: bool, turn_speed: f32, dt: f32) -> f32 {
    let dir = (left as i32 - right as i32) as f32;
    dir * turn_speed * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_keys_and_do_not_collide() {
        let b = Binds::default();
        assert_eq!(b.key(Bind::TurnLeft), KeyCode::KeyQ);
        assert_eq!(b.key(Bind::TurnRight), KeyCode::KeyE);
        assert_eq!(b.key(Bind::Forward), KeyCode::KeyW);
        for (i, x) in Bind::ALL.iter().enumerate() {
            for y in &Bind::ALL[i + 1..] {
                assert_ne!(b.key(*x), b.key(*y), "{x:?} and {y:?} share a key");
            }
        }
    }

    #[test]
    fn every_default_key_is_bindable_and_round_trips_by_name() {
        for b in Bind::ALL {
            let k = b.default_key();
            assert!(key_label(k).is_some(), "{b:?} default {k:?} not in KEYS");
            assert_eq!(parse_key(&key_name(k)), Some(k));
        }
        assert_eq!(parse_key("Escape"), None);
        assert_eq!(parse_key("nonsense"), None);
    }

    #[test]
    fn rebinding_onto_a_used_key_swaps_instead_of_duplicating() {
        let mut b = Binds::default();
        assert!(b.set(Bind::Jump, KeyCode::KeyQ));
        assert_eq!(b.key(Bind::Jump), KeyCode::KeyQ);
        // Turn-left took Jump's old key rather than going dark.
        assert_eq!(b.key(Bind::TurnLeft), KeyCode::Space);
        assert_eq!(b.bind_of(KeyCode::KeyQ), Some(Bind::Jump));
        // Binding a key to the bind it already drives is a no-op.
        assert!(b.set(Bind::Jump, KeyCode::KeyQ));
        assert_eq!(b.key(Bind::TurnLeft), KeyCode::Space);
    }

    #[test]
    fn escape_and_unknown_keys_are_refused() {
        let mut b = Binds::default();
        assert!(!b.set(Bind::Reset, KeyCode::Escape));
        assert_eq!(b.key(Bind::Reset), KeyCode::KeyR);
    }

    #[test]
    fn map_round_trip_and_partial_files_keep_defaults() {
        let mut b = Binds::default();
        b.set(Bind::Duck, KeyCode::ShiftLeft);
        let back = Binds::from_map(&b.to_map());
        assert_eq!(back, b);

        let mut partial = BTreeMap::new();
        partial.insert("jump".to_string(), "KeyV".to_string());
        partial.insert("reset".to_string(), "Escape".to_string()); // refused
        let p = Binds::from_map(&partial);
        assert_eq!(p.key(Bind::Jump), KeyCode::KeyV);
        assert_eq!(p.key(Bind::Reset), KeyCode::KeyR);
        assert_eq!(p.key(Bind::Forward), KeyCode::KeyW);
    }

    #[test]
    fn turn_bind_matches_cl_yawspeed_per_tick() {
        // 210 deg/s at 66.67 Hz is 3.15 degrees a tick, left positive.
        let d = turn_delta(true, false, DEFAULT_TURN_SPEED, 0.015);
        assert!((d - 3.15).abs() < 1e-4, "{d}");
        assert_eq!(turn_delta(false, true, 210.0, 0.015), -d);
        assert_eq!(turn_delta(true, true, 210.0, 0.015), 0.0);
        assert_eq!(turn_delta(false, false, 210.0, 0.015), 0.0);
    }
}
