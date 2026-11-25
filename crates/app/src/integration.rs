//! Opt-in Windows integration. Everything here is off by default.
//!
//! - **Start with Windows:** a shortcut in the user's Startup folder. No `Run`
//!   registry key, no scheduled task, no service.
//! - **Explorer context menu:** "Edit with Snap It" / "Pin with Snap It" verbs for
//!   image files, under HKCU only (no admin, removable with one click).
//! - **Update check:** one HTTPS GET to the GitHub releases API through WinHTTP
//!   (system proxy and certificate store). It only *notifies*; nothing is downloaded
//!   or run. The user agent is a generic product string with no personal data.

use std::path::PathBuf;

use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{IShellLinkW, SHGetKnownFolderPath, ShellLink, FOLDERID_Startup, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::{self, Command};

pub const WM_UPDATE_RESULT: u32 = WM_APP + 3;

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("snapit.exe"))
}

fn startup_link() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(|s| PathBuf::from(s).join("Snap It.lnk"))
    }
}

pub fn set_start_with_windows(on: bool) -> Result<(), String> {
    let link = startup_link().ok_or("Startup folder not found")?;
    if !on {
        if link.exists() {
            std::fs::remove_file(&link).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let exe = exe();
        sl.SetPath(&HSTRING::from(exe.as_os_str())).map_err(|e| e.to_string())?;
        if let Some(dir) = exe.parent() {
            let _ = sl.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()));
        }
        let _ = sl.SetDescription(w!("Snap It screenshot tool"));
        let pf: IPersistFile = sl.cast().map_err(|e| e.to_string())?;
        pf.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| e.to_string())
    }
}

pub fn start_with_windows_enabled() -> bool {
    startup_link().is_some_and(|l| l.exists())
}

const VERBS_ROOT: &str = "Software\\Classes\\SystemFileAssociations\\image\\shell";

fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<(), String> {
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
    let name = name.map(HSTRING::from);
    let r = unsafe { RegSetValueExW(key, name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), None, REG_SZ, Some(bytes)) };
    r.ok().map_err(|e| e.to_string())
}

fn create_key(path: &str) -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None) };
    r.ok().map(|_| key).map_err(|e| e.to_string())
}

pub fn set_context_menu(on: bool) -> Result<(), String> {
    let verbs = [("SnapIt.Edit", "Edit with Snap It", "--edit"), ("SnapIt.Pin", "Pin with Snap It", "--pin")];
    for (verb, label, flag) in verbs {
        let path = format!("{VERBS_ROOT}\\{verb}");
        if !on {
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path));
            }
            continue;
        }
        let exe = exe().to_string_lossy().into_owned();
        let k = create_key(&path)?;
        set_string(k, None, label)?;
        set_string(k, Some("Icon"), &format!("\"{exe}\",0"))?;
        unsafe {
            let _ = windows::Win32::System::Registry::RegCloseKey(k);
        }
        let c = create_key(&format!("{path}\\command"))?;
        set_string(c, None, &format!("\"{exe}\" {flag} \"%1\""))?;
        unsafe {
            let _ = windows::Win32::System::Registry::RegCloseKey(c);
        }
    }
    Ok(())
}

// ---------------- update check ----------------

pub struct UpdateResult {
    pub result: Result<Option<(String, String)>, String>,
    pub manual: bool,
}

fn parse_version(s: &str) -> Vec<u32> {
    s.trim_start_matches(['v', 'V']).split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

/// True if `remote` is a newer version than `local` (e.g. "v1.2.0" vs "1.1.9").
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (r, l) = (parse_version(remote), parse_version(local));
    !r.is_empty() && r > l
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: Option<&str>| p.is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

fn https_get(host: &str, path: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Networking::WinHttp::*;
    unsafe {
        let agent = HSTRING::from(format!("SnapIt-UpdateCheck/{}", env!("CARGO_PKG_VERSION")));
        let session = WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let close = |h: *mut core::ffi::c_void| {
            if !h.is_null() {
                let _ = WinHttpCloseHandle(h);
            }
        };
        let conn = WinHttpConnect(session, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0);
        if conn.is_null() {
            close(session);
            return Err("could not connect".into());
        }
        let req = WinHttpOpenRequest(conn, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE);
        let out = (|| {
            if req.is_null() {
                return Err("could not open request".to_string());
            }
            let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
            WinHttpSendRequest(req, Some(&headers), None, 0, 0, 0).map_err(|e| e.to_string())?;
            WinHttpReceiveResponse(req, std::ptr::null_mut()).map_err(|e| e.to_string())?;
            let mut status: u32 = 0;
            let mut len = 4u32;
            let _ = WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut _ as *mut _), &mut len, std::ptr::null_mut());
            let mut body = Vec::new();
            loop {
                let mut avail = 0u32;
                WinHttpQueryDataAvailable(req, &mut avail).map_err(|e| e.to_string())?;
                if avail == 0 || body.len() > 4 << 20 {
                    break;
                }
                let mut buf = vec![0u8; avail as usize];
                let mut read = 0u32;
                WinHttpReadData(req, buf.as_mut_ptr() as *mut _, avail, &mut read).map_err(|e| e.to_string())?;
                body.extend_from_slice(&buf[..read as usize]);
            }
            match status {
                200 => Ok(body),
                404 => Err("No releases found for that repository.".into()),
                0 => Ok(body),
                s => Err(format!("GitHub returned HTTP {s}")),
            }
        })();
        close(req);
        close(conn);
        close(session);
        out
    }
}

fn check(repo: &str) -> Result<Option<(String, String)>, String> {
    if !valid_repo(repo) {
        return Err("Set the update repository as owner/repo in Settings > Integration.".into());
    }
    let body = https_get("api.github.com", &format!("/repos/{repo}/releases/latest"))?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("unexpected response")?.to_string();
    let url = v["html_url"].as_str().unwrap_or_default().to_string();
    Ok(is_newer(&tag, env!("CARGO_PKG_VERSION")).then_some((tag, url)))
}

/// Runs the check on a worker thread; the result arrives as WM_UPDATE_RESULT.
pub fn check_for_updates(manual: bool) {
    let repo = app::settings().integration.update_repo.trim().to_string();
    let hwnd = app::main_hwnd().0 as isize;
    std::thread::spawn(move || {
        let r = Box::new(UpdateResult { result: check(&repo), manual });
        let raw = Box::into_raw(r);
        unsafe {
            if PostMessageW(Some(HWND(hwnd as *mut _)), WM_UPDATE_RESULT, WPARAM(0), LPARAM(raw as isize)).is_err() {
                drop(Box::from_raw(raw));
            }
        }
    });
}

pub fn on_update_result(r: UpdateResult) {
    match r.result {
        Ok(Some((tag, url))) => {
            app::with(|s| s.pending_click = Some(app::Click::Url(url)));
            app::post(Command::Notify { title: format!("Snap It {tag} is available"), body: "Click to open the release page.".into() });
        }
        Ok(None) if r.manual => app::post(Command::Notify { title: "Snap It is up to date".into(), body: format!("You have version {}.", env!("CARGO_PKG_VERSION")) }),
        Ok(None) => {}
        Err(e) if r.manual => app::post(Command::Notify { title: "Update check failed".into(), body: e }),
        Err(e) => crate::log(&format!("update check: {e}")),
    }
}

/// Re-applies the integration settings (called after the settings window saves).
pub fn apply(prev: &crate::settings::Integration, next: &crate::settings::Integration) -> Vec<String> {
    let mut errors = Vec::new();
    if prev.start_with_windows != next.start_with_windows || next.start_with_windows != start_with_windows_enabled() {
        if let Err(e) = set_start_with_windows(next.start_with_windows) {
            errors.push(format!("Start with Windows: {e}"));
        }
    }
    if prev.context_menu != next.context_menu {
        if let Err(e) = set_context_menu(next.context_menu) {
            errors.push(format!("Context menu: {e}"));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(is_newer("0.1.1-beta", "0.1.0"));
    }

    #[test]
    fn repo_validation() {
        assert!(valid_repo("someone/snap-it"));
        assert!(!valid_repo("someone"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("a/b?x=1"));
        assert!(!valid_repo(""));
    }
}
