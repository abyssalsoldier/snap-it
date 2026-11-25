//! Opt-in Windows integration. Everything here is off by default.
//!
//! - **Start with Windows:** a shortcut in the user's Startup folder. No `Run`
//!   registry key, no scheduled task, no service.
//! - **Explorer context menu:** "Edit with Snap It" / "Pin with Snap It" verbs for
//!   image files, under HKCU only (no admin, removable with one click).
//! - **Update check:** one HTTPS GET to the GitHub releases API through WinHTTP
//!   (system proxy and certificate store). A newer release shows a prompt whose
//!   Download button opens the matching installer in the browser; Snap It itself
//!   never downloads or runs anything. The user agent is a generic product string
//!   with no personal data.

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
/// The update prompt's "don't remind me" choice (lparam: Box<String> tag).
pub const WM_UPDATE_SKIP: u32 = WM_APP + 8;

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
        // Started at sign-in: stay in the tray instead of opening Settings.
        let _ = sl.SetArguments(&HSTRING::from(crate::BACKGROUND_ARG));
        let pf: IPersistFile = sl.cast().map_err(|e| e.to_string())?;
        pf.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| e.to_string())
    }
}

pub fn start_with_windows_enabled() -> bool {
    startup_link().is_some_and(|l| l.exists())
}

/// One-time migration: a Startup shortcut made before 0.3.2 has no `--background`, so
/// sign-in would open Settings. Rewritten only if it lacks the flag and points at this
/// exe, so the Startup folder is not written on every launch (a persistence-style write
/// EDR tools watch for).
pub fn upgrade_startup_link() {
    let Some(link) = startup_link().filter(|l| l.exists()) else { return };
    let current = unsafe {
        (|| -> Option<(String, String)> {
            let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let pf: IPersistFile = sl.cast().ok()?;
            pf.Load(&HSTRING::from(link.as_os_str()), windows::Win32::System::Com::STGM_READ).ok()?;
            let mut path = [0u16; 1024];
            sl.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
            let mut args = [0u16; 1024];
            sl.GetArguments(&mut args).ok()?;
            let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
            Some((s(&path), s(&args)))
        })()
    };
    let Some((target, args)) = current else { return };
    let ours = target.eq_ignore_ascii_case(&exe().to_string_lossy());
    if ours && !args.contains(crate::BACKGROUND_ARG) {
        if let Err(e) = set_start_with_windows(true) {
            crate::log(&format!("startup shortcut: {e}"));
        }
    }
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
    pub result: Result<Option<Release>, String>,
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

/// A newer release than the running version.
#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    /// The release page.
    pub page: String,
    /// Release notes (Markdown), as published.
    pub notes: String,
    /// (file name, download URL) of each attached file.
    pub assets: Vec<(String, String)>,
}

fn parse_release(v: &serde_json::Value) -> Result<Release, String> {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let tag = v["tag_name"].as_str().ok_or("unexpected response")?.to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string())))
                // Only GitHub's own download links are ever offered.
                .filter(|(_, url)| url.starts_with("https://github.com/"))
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { tag, page: s("html_url"), notes: s("body"), assets })
}

fn check(repo: &str) -> Result<Option<Release>, String> {
    if !valid_repo(repo) {
        return Err("Set the update repository as owner/repo in Settings > Integration.".into());
    }
    let body = https_get("api.github.com", &format!("/repos/{repo}/releases/latest"))?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let rel = parse_release(&v)?;
    Ok(is_newer(&rel.tag, env!("CARGO_PKG_VERSION")).then_some(rel))
}

/// How this copy was installed, which decides the matching download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Setup .exe (Inno Setup): its uninstaller sits next to snapit.exe.
    Setup,
    /// MSI: the package records its folder under HKCU\Software\SnapIt\Installer.
    Msi,
    Portable,
}

pub fn install_kind() -> InstallKind {
    if crate::paths::portable() {
        return InstallKind::Portable;
    }
    let dir = crate::paths::exe_dir();
    if dir.join("unins000.exe").exists() {
        return InstallKind::Setup;
    }
    let norm = |p: &str| p.trim_end_matches(['\\', '/']).to_lowercase();
    match read_hkcu_string("Software\\SnapIt\\Installer", "InstallDir") {
        Some(d) if norm(&d) == norm(&dir.to_string_lossy()) => InstallKind::Msi,
        // A copy that is neither (a dev build, a hand-copied exe): offer the installer.
        _ => InstallKind::Setup,
    }
}

fn read_hkcu_string(key: &str, value: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) };
    if r.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The download matching `kind`, by the release file naming in scripts/package.ps1.
pub fn pick_asset(assets: &[(String, String)], kind: InstallKind) -> Option<String> {
    let want = |name: &str| match kind {
        InstallKind::Setup => name.ends_with("-setup-x64.exe"),
        InstallKind::Msi => name.ends_with("-x64.msi"),
        InstallKind::Portable => name.ends_with("-portable-x64.zip"),
    };
    assets.iter().find(|(n, _)| want(n)).map(|(_, u)| u.clone())
}

/// Release notes as plain text for the prompt: the download list is dropped
/// (the prompt has its own button) and Markdown emphasis is removed.
pub fn notes_text(md: &str) -> String {
    let body = md.split("### Downloads").next().unwrap_or_default();
    let mut out = String::new();
    for line in body.lines() {
        let l = line.trim_end().replace("**", "").replace('`', "");
        let l = l.strip_prefix("### ").or_else(|| l.strip_prefix("## ")).map(str::to_string).unwrap_or(l);
        out.push_str(&l);
        out.push('\n');
    }
    let out = out.trim().to_string();
    const MAX: usize = 1800;
    match out.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…\n\n(See the release notes for the rest.)", &out[..i]),
        None => out,
    }
}

/// The release repository: the setting, or this project's own when it is left empty.
fn update_repo() -> String {
    let r = app::settings().integration.update_repo.trim().to_string();
    if r.is_empty() { crate::settings::DEFAULT_UPDATE_REPO.to_string() } else { r }
}

/// Runs the check on a worker thread; the result arrives as WM_UPDATE_RESULT.
pub fn check_for_updates(manual: bool) {
    let repo = update_repo();
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

static PROMPT_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Shows the update prompt on its own thread (so hotkeys and captures keep working).
/// "Download" opens the matching installer in the browser; Snap It itself never
/// downloads or runs anything. The installers close and replace a running copy.
fn prompt_update(rel: Release) {
    use std::sync::atomic::Ordering;
    if PROMPT_OPEN.swap(true, Ordering::SeqCst) {
        return; // one prompt at a time
    }
    let kind = install_kind();
    // Portable copies are offered the setup wizard too, so updating always means "run the wizard".
    let download = pick_asset(&rel.assets, if kind == InstallKind::Portable { InstallKind::Setup } else { kind });
    let main = app::main_hwnd().0 as isize;
    let spawned = std::thread::Builder::new().name("snapit-update-prompt".into()).spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let how = match (kind, download.is_some()) {
            (_, false) => "Download opens the release page, where you can get the installer.",
            (InstallKind::Setup, true) => "Download saves the installer in your browser. Run it to update: it closes Snap It, updates it and starts it again.",
            (InstallKind::Msi, true) => "Download saves the MSI package in your browser. Run it to update: it closes Snap It and updates it; start it again from the Start menu.",
            (InstallKind::Portable, true) => "Download saves the installer in your browser. Run it to install Snap It with a Start menu shortcut; it closes this portable copy and starts the installed one. Your portable folder is left as it is (the installed copy keeps its settings in %APPDATA%\\SnapIt).",
        };
        let notes = notes_text(&rel.notes);
        let (choice, skip) = crate::ui::dialogs::update_prompt(&rel.tag, env!("CARGO_PKG_VERSION"), how, &notes);
        crate::log(&format!("update prompt: {choice:?}, skip this version: {skip}"));
        match choice {
            crate::ui::dialogs::UpdateChoice::Download => crate::shell::open_url(download.as_deref().unwrap_or(&rel.page)),
            crate::ui::dialogs::UpdateChoice::Notes => crate::shell::open_url(&rel.page),
            crate::ui::dialogs::UpdateChoice::Later => {}
        }
        if skip {
            let raw = Box::into_raw(Box::new(rel.tag));
            unsafe {
                if PostMessageW(Some(HWND(main as *mut _)), WM_UPDATE_SKIP, WPARAM(0), LPARAM(raw as isize)).is_err() {
                    drop(Box::from_raw(raw));
                }
            }
        }
        PROMPT_OPEN.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        PROMPT_OPEN.store(false, Ordering::SeqCst);
    }
}

/// "Don't remind me about this version" (posted by the prompt thread).
pub fn on_skip_version(tag: String) {
    app::update_settings(|s| s.integration.skipped_version = tag);
}

pub fn on_update_result(r: UpdateResult) {
    match r.result {
        Ok(Some(rel)) => {
            // "Don't remind me" silences the automatic check for that version; "Check now" still shows it.
            if !r.manual && app::settings().integration.skipped_version == rel.tag {
                return;
            }
            crate::log(&format!("update available: {}", rel.tag));
            prompt_update(rel);
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

    fn release_json() -> serde_json::Value {
        let a = |n: &str, host: &str| serde_json::json!({ "name": n, "browser_download_url": format!("https://{host}/x/releases/download/v0.3.0/{n}") });
        serde_json::json!({
            "tag_name": "v0.3.0",
            "html_url": "https://github.com/x/releases/tag/v0.3.0",
            "body": "### Added\n- **Spotlight** tool (`S`)\n\n### Downloads\n- setup.exe: installer",
            "assets": [
                a("SHA256SUMS.txt", "github.com"),
                a("SnapIt-0.3.0-portable-x64.zip", "github.com"),
                a("SnapIt-0.3.0-setup-x64.exe", "github.com"),
                a("SnapIt-0.3.0-x64.msi", "github.com"),
                a("SnapIt-0.3.0-evil-setup-x64.exe", "example.com"),
            ]
        })
    }

    #[test]
    fn picks_the_download_for_each_install_kind() {
        let rel = parse_release(&release_json()).unwrap();
        assert_eq!(rel.tag, "v0.3.0");
        assert!(rel.assets.iter().all(|(_, u)| u.starts_with("https://github.com/")), "non-GitHub links are dropped");
        let pick = |k| pick_asset(&rel.assets, k).unwrap();
        assert!(pick(InstallKind::Setup).ends_with("SnapIt-0.3.0-setup-x64.exe"));
        assert!(pick(InstallKind::Msi).ends_with("SnapIt-0.3.0-x64.msi"));
        assert!(pick(InstallKind::Portable).ends_with("SnapIt-0.3.0-portable-x64.zip"));
        assert_eq!(pick_asset(&[], InstallKind::Setup), None);
    }

    /// Live check against GitHub: `cargo test -p snap-it live_update_check -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_update_check() {
        let t = std::time::Instant::now();
        let body = https_get("api.github.com", &format!("/repos/{}/releases/latest", crate::settings::DEFAULT_UPDATE_REPO));
        println!("https_get took {:?}: {}", t.elapsed(), body.as_ref().map(|b| format!("{} bytes", b.len())).unwrap_or_else(|e| e.clone()));
        let v: serde_json::Value = serde_json::from_slice(&body.unwrap()).unwrap();
        let rel = parse_release(&v).unwrap();
        println!("tag {} assets {:?}", rel.tag, rel.assets.iter().map(|a| &a.0).collect::<Vec<_>>());
    }

    #[test]
    fn release_notes_become_plain_text() {
        let t = notes_text(release_json()["body"].as_str().unwrap());
        assert_eq!(t, "Added\n- Spotlight tool (S)");
        let long = "x".repeat(5000);
        assert!(notes_text(&long).ends_with("(See the release notes for the rest.)"));
    }
}
