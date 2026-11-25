//! Hotkey boxes in Settings record a key combination when pressed, instead of being typed.
//!
//! Only keystrokes sent to the focused box itself are seen (a window subclass on our own
//! edit control), so this is not a keyboard hook (see the EDR rule in `hotkeys.rs`). While a
//! box has focus, Snap It's own global hotkeys are paused, so pressing PrintScreen records
//! it instead of starting a capture.
//!
//! - A combination (Ctrl/Alt/Shift/Win + key, or a key alone) replaces the box's text.
//! - Backspace or Delete clears it (no hotkey). Esc leaves the box unchanged.
//! - PrintScreen only sends key-up to applications, so key-up of PrintScreen counts too.

use std::cell::Cell;

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use windows::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETSEL};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetParent, SendMessageW, SetWindowTextW, WM_CHAR, WM_CLEAR, WM_CONTEXTMENU, WM_CUT, WM_DEADCHAR,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_PASTE, WM_SETFOCUS, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::hotkeys::Chord;

thread_local! {
    /// Global hotkeys are unregistered while a hotkey box has focus.
    static PAUSED: Cell<bool> = const { Cell::new(false) };
}

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_SNAPSHOT: u32 = 0x2C;
const VK_DELETE: u32 = 0x2E;

/// Turns an edit control into a hotkey recorder.
pub fn attach(edit: HWND) {
    unsafe {
        let _ = SetWindowSubclass(edit, Some(proc), 2, 0);
        // Shown while the box is empty, even when focused.
        let cue = w!("Click, then press keys");
        SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
    }
}

/// Resumes the global hotkeys if a hotkey box paused them (also called when Settings closes).
pub fn resume_hotkeys() {
    if PAUSED.with(|p| p.replace(false)) {
        let failed = crate::reregister_hotkeys();
        if !failed.is_empty() {
            crate::log(&format!("hotkeys unavailable: {}", failed.join(", ")));
        }
    }
}

fn pause_hotkeys() {
    if !PAUSED.with(|p| p.replace(true)) {
        crate::suspend_hotkeys();
    }
}

fn is_modifier(vk: u32) -> bool {
    // Shift, Ctrl, Alt (generic, left and right) and the Windows keys.
    matches!(vk, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

fn down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

fn current_mods() -> u32 {
    let mut m = 0;
    if down(0x11) {
        m |= MOD_CONTROL.0;
    }
    if down(0x12) {
        m |= MOD_ALT.0;
    }
    if down(0x10) {
        m |= MOD_SHIFT.0;
    }
    if down(0x5B) || down(0x5C) {
        m |= MOD_WIN.0;
    }
    m
}

/// The chord for `vk` pressed with `mods`, if it is one Snap It can save and register.
pub fn chord_for(mods: u32, vk: u32) -> Option<Chord> {
    if is_modifier(vk) {
        return None;
    }
    let c = Chord { mods, vk };
    // Only keys whose name parses back to the same chord (no "0x.." codes).
    (Chord::parse(&c.display()) == Some(c)).then_some(c)
}

fn set_text(edit: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(edit, &HSTRING::from(s));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(usize::MAX)), Some(LPARAM(-1)));
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    let vk = wp.0 as u32;
    match msg {
        WM_SETFOCUS => {
            pause_hotkeys();
            unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
        }
        WM_KILLFOCUS => {
            resume_hotkeys();
            unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let mods = current_mods();
            match vk {
                // Plain Tab still moves between controls.
                VK_TAB if mods == 0 => return unsafe { DefSubclassProc(hwnd, msg, wp, lp) },
                VK_BACK | VK_DELETE if mods == 0 => set_text(hwnd, ""),
                VK_ESCAPE if mods == 0 => unsafe {
                    // Leave the box unchanged.
                    let _ = SetFocus(GetParent(hwnd).ok());
                },
                _ => {
                    if let Some(c) = chord_for(mods, vk) {
                        set_text(hwnd, &c.display());
                    }
                }
            }
            LRESULT(0)
        }
        // PrintScreen reaches applications only as key-up.
        WM_KEYUP | WM_SYSKEYUP if vk == VK_SNAPSHOT => {
            if let Some(c) = chord_for(current_mods(), vk) {
                set_text(hwnd, &c.display());
            }
            LRESULT(0)
        }
        // No typing, pasting or editing menu: the box only records key presses.
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_CONTEXTMENU => LRESULT(0),
        _ => unsafe { DefSubclassProc(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_chords_round_trip() {
        let ctrl_shift = MOD_CONTROL.0 | MOD_SHIFT.0;
        assert_eq!(chord_for(ctrl_shift, VK_SNAPSHOT).unwrap().display(), "Ctrl+Shift+PrintScreen");
        assert_eq!(chord_for(MOD_ALT.0, 0x70).unwrap().display(), "Alt+F1");
        assert_eq!(chord_for(0, VK_SNAPSHOT).unwrap().display(), "PrintScreen");
        assert_eq!(chord_for(MOD_WIN.0 | MOD_SHIFT.0, b'S' as u32).unwrap().display(), "Shift+Win+S");
        // Modifiers alone, and keys without a savable name, are not recorded.
        assert!(chord_for(MOD_CONTROL.0, 0x11).is_none());
        assert!(chord_for(0, 0xFF).is_none());
    }
}
