//! Asking the Windows shell to show things. We never launch processes ourselves;
//! the shell opens folders and URLs with the user's default handlers.

use std::path::Path;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Opens a folder in Explorer. Refuses anything that isn't a directory, so a
/// hand-edited settings path can never turn "Open folder" into launching a file.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if !dir.is_dir() {
        crate::log(&format!("not a folder, refusing to open: {}", dir.display()));
        return;
    }
    let p = HSTRING::from(dir.as_os_str());
    unsafe {
        ShellExecuteW(None, &HSTRING::from("explore"), &p, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Opens Explorer with `file` selected.
pub fn show_in_folder(file: &Path) {
    let p = HSTRING::from(file.as_os_str());
    unsafe {
        let pidl = ILCreateFromPathW(&p);
        if pidl.is_null() {
            if let Some(d) = file.parent() {
                open_folder(d);
            }
            return;
        }
        let _ = SHOpenFolderAndSelectItems(pidl, None, 0);
        ILFree(Some(pidl));
    }
}

/// Opens Windows Settings > Accessibility > Keyboard (where the Print Screen key can be
/// taken back from the Snipping Tool). A fixed URI; nothing user-supplied is passed.
pub fn open_keyboard_settings() {
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from("ms-settings:easeofaccess-keyboard"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Opens an https URL in the default browser. Anything else is refused.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}
