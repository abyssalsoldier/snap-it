//! Where Snap It keeps its settings, history and screenshots.
//!
//! Portable mode: if a file named `portable` sits next to `snapit.exe`, everything
//! lives beside the exe instead of in %APPDATA% / Pictures.

use std::path::PathBuf;
use std::sync::OnceLock;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn portable() -> bool {
    static P: OnceLock<bool> = OnceLock::new();
    *P.get_or_init(|| exe_dir().join("portable").exists())
}

/// `SNAPIT_HOME` (used by the self-test) redirects all data into one folder.
fn home_override() -> Option<PathBuf> {
    std::env::var_os("SNAPIT_HOME").map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("data");
    }
    if portable() {
        return exe_dir().join("data");
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(exe_dir);
    base.join("SnapIt")
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.toml")
}

pub fn history_file() -> PathBuf {
    config_dir().join("history.jsonl")
}

fn pictures_dir() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub fn default_save_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("Screenshots");
    }
    if portable() {
        return exe_dir().join("Screenshots");
    }
    pictures_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Pictures")))
        .unwrap_or_else(exe_dir)
        .join("Snap It")
}
