//! Notification-area icon, its menu, and balloon notifications. The icon is
//! always visible while Snap It runs, so it never operates "hidden".

use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, HICON, HMENU, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN,
    TPM_RIGHTBUTTON, WM_APP, WM_NULL, WM_USER,
};

use crate::actions::Action;

pub const WM_TRAY: u32 = WM_APP + 1;
/// Sent (as the tray callback's lparam) when the user clicks a balloon notification.
pub const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const TRAY_UID: u32 = 1;

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        ..Default::default()
    }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

pub fn add(hwnd: HWND, icon: HICON) -> Result<()> {
    let mut nid = base_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    copy_into(&mut nid.szTip, "Snap It");
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid).ok() }
}

pub fn remove(hwnd: HWND) {
    let nid = base_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Shows a Windows notification from the tray icon (appears as a toast on Windows 10/11).
pub fn notify(hwnd: HWND, title: &str, body: &str) {
    let mut nid = base_data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
    copy_into(&mut nid.szInfoTitle, title);
    copy_into(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn add_item(menu: HMENU, a: Action) -> Result<()> {
    let s = crate::app::settings();
    let label = match s.hotkey_for(a) {
        Some(k) => format!("{}\t{}", a.label(), k),
        None => a.label().to_string(),
    };
    unsafe { AppendMenuW(menu, MF_STRING, a.id() as usize, &HSTRING::from(label)) }
}

fn submenu(parent: HMENU, title: &str, items: &[Action]) -> Result<()> {
    unsafe {
        let m = CreatePopupMenu()?;
        for &a in items {
            add_item(m, a)?;
        }
        AppendMenuW(parent, MF_POPUP, m.0 as usize, &HSTRING::from(title))
    }
}

/// Shows the context menu at the cursor. The chosen item arrives as WM_COMMAND.
pub fn show_menu(hwnd: HWND) -> Result<()> {
    use Action::*;
    unsafe {
        let menu = CreatePopupMenu()?;
        let mut capture = vec![CaptureRegion, CaptureWindow, CaptureActiveWindow, CaptureMonitor, CaptureFullscreen, CaptureLastRegion, CaptureCustomRegion];
        if crate::app::settings().scrolling_capture {
            capture.push(CaptureScrolling);
        }
        submenu(menu, "Capture", &capture)?;
        submenu(menu, "Record", &[RecordMp4, RecordGif])?;
        submenu(menu, "Tools", &[Ocr, ScanQr, MakeQr, ColorPicker, Ruler, OpenEditor])?;
        AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
        for a in [OpenHistory, OpenFolder, OpenSettings] {
            add_item(menu, a)?;
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
        add_item(menu, Exit)?;

        let mut pt = POINT::default();
        GetCursorPos(&mut pt)?;
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        DestroyMenu(menu)?; // also destroys the submenus
    }
    Ok(())
}
