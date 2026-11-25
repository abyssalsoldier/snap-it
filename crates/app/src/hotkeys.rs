//! Global hotkeys via `RegisterHotKey` only.
//!
//! EDR rule: never install low-level keyboard hooks (`SetWindowsHookEx` with
//! `WH_KEYBOARD_LL`), because that is the classic keylogger signature.
//! `RegisterHotKey` tells the OS which chords we want, and we receive
//! only those as `WM_HOTKEY`, never any other keystrokes.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

/// A parsed chord such as `Ctrl+Shift+PrintScreen`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub mods: u32,
    pub vk: u32,
}

const KEYS: &[(&str, u32)] = &[
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Space", 0x20),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Backspace", 0x08),
    ("Escape", 0x1B),
    ("Up", 0x26),
    ("Down", 0x28),
    ("Left", 0x25),
    ("Right", 0x27),
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    ("[", 0xDB),
    ("]", 0xDD),
    (";", 0xBA),
    ("'", 0xDE),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
    ("\\", 0xDC),
];

fn key_from_name(s: &str) -> Option<u32> {
    let up = s.to_ascii_uppercase();
    if let Some(&(_, vk)) = KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
        return Some(vk);
    }
    match up.as_str() {
        "PRTSC" | "PRINT" => return Some(0x2C),
        "ESC" => return Some(0x1B),
        "DEL" => return Some(0x2E),
        "INS" => return Some(0x2D),
        _ => {}
    }
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = up.strip_prefix("NUMPAD").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    None
}

pub fn key_name(vk: u32) -> String {
    if let Some((n, _)) = KEYS.iter().find(|(_, v)| *v == vk) {
        return n.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x60..=0x69 => format!("Numpad{}", vk - 0x60),
        _ => format!("0x{vk:02X}"),
    }
}

impl Chord {
    pub fn parse(s: &str) -> Option<Chord> {
        let mut mods = 0;
        let mut vk = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= MOD_CONTROL.0,
                "alt" => mods |= MOD_ALT.0,
                "shift" => mods |= MOD_SHIFT.0,
                "win" | "super" => mods |= MOD_WIN.0,
                _ => {
                    if vk.is_some() {
                        return None; // two non-modifier keys
                    }
                    vk = Some(key_from_name(part)?);
                }
            }
        }
        Some(Chord { mods, vk: vk? })
    }

    pub fn display(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(MOD_CONTROL.0, "Ctrl+"), (MOD_ALT.0, "Alt+"), (MOD_SHIFT.0, "Shift+"), (MOD_WIN.0, "Win+")] {
            if self.mods & bit != 0 {
                s.push_str(name);
            }
        }
        s.push_str(&key_name(self.vk));
        s
    }
}

/// Registers each `(id, chord)`, returning the chords that failed
/// (usually because another app already owns them).
pub fn register_all(hwnd: HWND, bindings: &[(u16, Chord)]) -> Vec<Chord> {
    let mut failed = Vec::new();
    for (id, c) in bindings {
        let ok = unsafe {
            RegisterHotKey(Some(hwnd), *id as i32, HOT_KEY_MODIFIERS(c.mods) | MOD_NOREPEAT, c.vk)
        };
        if ok.is_err() {
            failed.push(*c);
        }
    }
    failed
}

pub fn unregister_all(hwnd: HWND, bindings: &[(u16, Chord)]) {
    for (id, _) in bindings {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), *id as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_chords() {
        let c = Chord::parse("ctrl + shift + prtsc").unwrap();
        assert_eq!(c.vk, 0x2C);
        assert_eq!(c.display(), "Ctrl+Shift+PrintScreen");
        assert_eq!(Chord::parse("Alt+F12").unwrap().display(), "Alt+F12");
        assert_eq!(Chord::parse("Win+Shift+S").unwrap().vk, b'S' as u32);
        assert_eq!(Chord::parse("PrintScreen").unwrap().mods, 0);
        assert!(Chord::parse("Ctrl+").is_none());
        assert!(Chord::parse("A+B").is_none());
        assert!(Chord::parse("Ctrl+Nope").is_none());
    }
}
