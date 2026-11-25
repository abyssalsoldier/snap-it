//! Coordination between copies of Snap It, using only named kernel objects
//! (no window lookup and no cross-process window messages):
//!
//! - `Local\SnapIt.SingleInstance` mutex: owned by the running copy.
//! - `Local\SnapIt.Instance` shared memory: the running copy's version and exe path.
//! - `Local\SnapIt.Quit` event: another launch asks it to exit (installers, take-over).
//! - `Local\SnapIt.Activate` event: another launch tells it "the user started Snap It again".
//!
//! A second launch without arguments therefore never just vanishes: it reminds the
//! user that Snap It is running, or, if it is a newer or different copy, takes over.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE, PAGE_READWRITE};
use windows::Win32::System::Threading::{CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

const QUIT_EVENT: PCWSTR = w!("Local\\SnapIt.Quit");
const ACTIVATE_EVENT: PCWSTR = w!("Local\\SnapIt.Activate");
const INFO_MAP: PCWSTR = w!("Local\\SnapIt.Instance");
/// Room for "version\0exe path\0" in UTF-16.
const INFO_BYTES: usize = 4096;

pub const WM_APP_QUIT: u32 = WM_APP + 7;
pub const WM_APP_ACTIVATE: u32 = WM_APP + 9;

fn signal(name: PCWSTR) -> bool {
    match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, name) } {
        Ok(ev) => unsafe {
            let ok = SetEvent(ev).is_ok();
            let _ = CloseHandle(ev);
            ok
        },
        // Nothing running (or a copy too old to listen): nothing to signal.
        Err(_) => false,
    }
}

/// Asks the running copy to exit cleanly (as the tray's Exit does).
pub fn signal_quit() -> bool {
    signal(QUIT_EVENT)
}

/// Tells the running copy that the user started Snap It again (it shows Settings).
pub fn signal_activate() -> bool {
    // This launch came from the user, so it may bring a window to the front; pass that
    // right on, or the running copy's Settings window would only flash in the taskbar.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    signal(ACTIVATE_EVENT)
}

/// Turns the quit and activate events into messages for the main window.
pub fn watch(main: HWND) {
    let (Ok(quit), Ok(activate)) = (unsafe { CreateEventW(None, false, false, QUIT_EVENT) }, unsafe { CreateEventW(None, false, false, ACTIVATE_EVENT) }) else { return };
    // Handles stay open for the life of the process.
    let (quit, activate, main) = (quit.0 as isize, activate.0 as isize, main.0 as isize);
    let _ = std::thread::Builder::new().name("snapit-instance".into()).spawn(move || {
        let handles = [HANDLE(quit as *mut _), HANDLE(activate as *mut _)];
        loop {
            let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            let msg = match r.0.wrapping_sub(WAIT_OBJECT_0.0) {
                0 => WM_APP_QUIT,
                1 => WM_APP_ACTIVATE,
                _ => return,
            };
            if unsafe { PostMessageW(Some(HWND(main as *mut _)), msg, WPARAM(0), LPARAM(0)) }.is_err() || msg == WM_APP_QUIT {
                return;
            }
        }
    });
}

/// The copy that is already running, as it described itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub version: String,
    pub exe: String,
}

fn this_exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Publishes this copy's version and exe path for later launches (call once it owns the mutex).
pub fn publish() {
    let text = format!("{}\0{}\0", env!("CARGO_PKG_VERSION"), this_exe());
    let words: Vec<u16> = text.encode_utf16().take(INFO_BYTES / 2).collect();
    unsafe {
        let Ok(map) = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, INFO_BYTES as u32, INFO_MAP) else { return };
        let view = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, INFO_BYTES);
        if view.Value.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(words.as_ptr(), view.Value as *mut u16, words.len());
        let _ = UnmapViewOfFile(view);
        // The mapping handle stays open (and the memory alive) for the life of the process.
    }
}

/// Reads what the running copy published. `None` if nothing is running, or it is
/// a copy from before this existed (0.2.0 and older).
pub fn running() -> Option<Running> {
    unsafe {
        let map = OpenFileMappingW(FILE_MAP_READ.0, false, INFO_MAP).ok()?;
        let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, INFO_BYTES);
        let out = (!view.Value.is_null()).then(|| {
            let words = std::slice::from_raw_parts(view.Value as *const u16, INFO_BYTES / 2);
            parse_info(words)
        });
        if !view.Value.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(map);
        out.flatten()
    }
}

fn parse_info(words: &[u16]) -> Option<Running> {
    let mut parts = words.split(|&c| c == 0).map(String::from_utf16_lossy);
    let version = parts.next().filter(|v| !v.is_empty())?;
    let exe = parts.next().unwrap_or_default();
    Some(Running { version, exe })
}

/// What a second launch without arguments should do.
#[derive(Clone, Debug, PartialEq)]
pub enum SecondLaunch {
    /// Remind the user that the running copy is there.
    Activate,
    /// Replace the running copy without asking (this one is a newer version).
    TakeOver { updated_from: String },
    /// A different copy, not newer (or an older copy that can't say): ask first.
    Ask { running: Option<Running> },
}

pub fn decide(running: Option<Running>, me_exe: &str, me_version: &str) -> SecondLaunch {
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    match running {
        Some(r) if crate::integration::is_newer(me_version, &r.version) => SecondLaunch::TakeOver { updated_from: r.version },
        Some(r) if same(&r.exe, me_exe) => SecondLaunch::Activate,
        other => SecondLaunch::Ask { running: other },
    }
}

pub fn decide_now() -> SecondLaunch {
    decide(running(), &this_exe(), env!("CARGO_PKG_VERSION"))
}

/// Asks the running copy to exit and waits (up to `ms`) to inherit the single-instance
/// mutex. Returns true once this copy owns it.
pub fn take_over(mutex: HANDLE, ms: u32) -> bool {
    signal_quit();
    let r = unsafe { WaitForSingleObject(mutex, ms) };
    r == WAIT_OBJECT_0 || r == WAIT_ABANDONED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(v: &str, exe: &str) -> Option<Running> {
        Some(Running { version: v.into(), exe: exe.into() })
    }

    #[test]
    fn second_launch_decisions() {
        let me = r"C:\Users\x\AppData\Local\Programs\Snap It\snapit.exe";
        // Same exe, same version: just remind.
        assert_eq!(decide(r("0.3.0", &me.to_uppercase()), me, "0.3.0"), SecondLaunch::Activate);
        // Newer copy (installed over, or a new portable unzip): take over quietly.
        assert_eq!(decide(r("0.2.0", r"D:\old\snapit.exe"), me, "0.3.0"), SecondLaunch::TakeOver { updated_from: "0.2.0".into() });
        // A different copy that isn't newer: ask.
        assert!(matches!(decide(r("0.3.0", r"D:\portable\snapit.exe"), me, "0.3.0"), SecondLaunch::Ask { .. }));
        // An old copy that publishes nothing: ask.
        assert_eq!(decide(None, me, "0.3.0"), SecondLaunch::Ask { running: None });
    }

    #[test]
    fn info_round_trip() {
        let words: Vec<u16> = "0.3.0\0C:\\x\\snapit.exe\0\0\0".encode_utf16().collect();
        assert_eq!(parse_info(&words), r("0.3.0", r"C:\x\snapit.exe"));
        assert_eq!(parse_info(&[0, 0, 0]), None);
    }
}
