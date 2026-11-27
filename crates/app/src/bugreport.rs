//! "Report a bug": builds a short diagnostics summary and opens a pre-filled GitHub
//! issue in the user's browser. Snap It itself sends nothing; the user reviews the
//! text and submits it there. A panic hook keeps a small local `crash.log` so the
//! report can include what went wrong last time.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use windows::core::HSTRING;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

use crate::app::{self, Command};

/// Starts a new crash.log entry.
const ENTRY_MARK: &str = "=== ";
/// crash.log is cut back to its newest `KEEP` bytes once it grows past `MAX`.
const MAX_LOG: usize = 64 * 1024;
const KEEP_LOG: usize = 32 * 1024;
/// The crash entry included in a report is cut to this many characters.
const MAX_CRASH: usize = 1500;
/// Browsers and GitHub handle URLs well below this.
const MAX_URL: usize = 7000;

pub fn crash_log() -> PathBuf {
    crate::paths::config_dir().join("crash.log")
}

/// Records every panic in crash.log, then runs the default hook. Never panics itself.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let t = crate::naming::Stamp::now();
        let entry = format!(
            "{ENTRY_MARK}{:04}-{:02}-{:02} {:02}:{:02}:{:02} | Snap It {} | thread {thread}\n{msg}\nat {at}\n",
            t.year,
            t.month,
            t.day,
            t.hour,
            t.minute,
            t.second,
            env!("CARGO_PKG_VERSION"),
        );
        let _ = append_entry(&crash_log(), &entry);
        prev(info);
    }));
}

fn append_entry(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut all = old + entry;
    if all.len() > MAX_LOG {
        all = trim_log(&all, KEEP_LOG).to_string();
    }
    std::fs::File::create(path)?.write_all(all.as_bytes())
}

/// The newest whole entries that fit in `keep` bytes (or the newest entry, cut).
fn trim_log(log: &str, keep: usize) -> &str {
    if log.len() <= keep {
        return log;
    }
    let mut start = log.len() - keep;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    match log[start..].find(ENTRY_MARK) {
        Some(i) => &log[start + i..],
        None => match log.rfind(ENTRY_MARK) {
            Some(i) => &log[i..],
            None => &log[start..],
        },
    }
}

/// The newest entry in `log`, cut to `max` characters.
fn newest_entry(log: &str, max: usize) -> Option<String> {
    let i = log.rfind(ENTRY_MARK)?;
    let e = log[i..].trim_end();
    Some(if e.chars().count() > max { e.chars().take(max).collect::<String>() + " …" } else { e.to_string() })
}

pub fn last_crash() -> Option<String> {
    newest_entry(&std::fs::read_to_string(crash_log()).ok()?, MAX_CRASH)
}

/// Seconds since the epoch when crash.log was last written, if it exists.
pub fn crash_log_time() -> Option<u64> {
    let m = std::fs::metadata(crash_log()).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn hklm_string(key: &str, value: &str) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) };
    if r.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn hklm_dword(key: &str, value: &str) -> Option<u32> {
    let mut v: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut len)) };
    r.is_ok().then_some(v)
}

/// e.g. "Windows 11 Pro 23H2 (build 22631.4460)". Read-only registry lookups.
fn windows_version() -> String {
    const NT: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let build = hklm_string(NT, "CurrentBuild").unwrap_or_default();
    let mut name = hklm_string(NT, "ProductName").unwrap_or_else(|| "Windows".into());
    // ProductName still says "Windows 10" on Windows 11.
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        name = name.replacen("Windows 10", "Windows 11", 1);
    }
    let display = hklm_string(NT, "DisplayVersion").unwrap_or_default();
    let ubr = hklm_dword(NT, "UBR").map(|u| format!(".{u}")).unwrap_or_default();
    format!("{name} {display} (build {build}{ubr})").replace("  ", " ")
}

/// The environment summary placed in the issue. Only settings on this list are
/// included: no folders, file names, hotkeys or other personal details.
pub fn diagnostics() -> String {
    let s = app::settings();
    let kind = match crate::integration::install_kind() {
        crate::integration::InstallKind::Setup => "installer",
        crate::integration::InstallKind::Msi => "MSI",
        crate::integration::InstallKind::Portable => "portable",
    };
    let mut d = String::new();
    let _ = writeln!(d, "- Snap It: {} ({kind})", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(d, "- Windows: {}", windows_version());
    let mons = snapit_capture::monitors();
    let list: Vec<String> = mons
        .iter()
        .map(|m| format!("{}×{} @ {:.0}%{}", m.rect.w, m.rect.h, m.scale() * 100.0, if m.primary { " (primary)" } else { "" }))
        .collect();
    let _ = writeln!(d, "- Monitors ({}): {}", mons.len(), list.join(", "));
    let _ = writeln!(d, "- Format: {:?}, theme: {:?}", s.format, s.theme);
    let _ = writeln!(
        d,
        "- Scrolling capture: {}, update check: {}, start with Windows: {}, context menu: {}",
        s.scrolling_capture, s.integration.update_check, s.integration.start_with_windows, s.integration.context_menu
    );
    d
}

/// A GitHub "owner/repo", or the project's own repo when `repo` isn't one.
fn safe_repo(repo: &str) -> &str {
    let ok_part = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let mut parts = repo.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), None) if ok_part(a) && ok_part(b) => repo,
        _ => crate::settings::DEFAULT_UPDATE_REPO,
    }
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn body(diag: &str, crash: Option<&str>) -> String {
    let mut b = String::from(
        "### What happened?\n\n\n### Steps to reproduce\n1. \n\n### What did you expect?\n\n\n### Environment\n",
    );
    b.push_str(diag);
    if let Some(c) = crash {
        let _ = write!(b, "\n### Last crash\n```\n{c}\n```\n");
    }
    b
}

/// The new-issue URL. The crash text is shortened (then dropped) to stay under [`MAX_URL`].
pub fn issue_url(repo: &str, diag: &str, crash: Option<&str>) -> String {
    let base = format!("https://github.com/{}/issues/new?title={}&body=", safe_repo(repo), encode("Bug: "));
    let build = |c: Option<&str>| base.clone() + &encode(&body(diag, c));
    let mut url = build(crash);
    if let Some(c) = crash {
        let mut n = c.chars().count();
        while url.len() > MAX_URL && n > 0 {
            n = n * 2 / 3;
            let cut: String = c.chars().take(n).collect::<String>() + " …";
            url = build((n > 0).then_some(cut.as_str()));
        }
    }
    if url.len() > MAX_URL {
        url = build(None);
    }
    url
}

/// Shows what will be included, then opens the issue page if the user agrees.
pub fn report() {
    let diag = diagnostics();
    let crash = last_crash();
    let mut preview = String::from(
        "This opens a new GitHub issue in your browser with the info below. \
         Nothing is sent until you submit it there.\n\n",
    );
    preview.push_str(&diag);
    if crash.is_some() {
        preview.push_str("- The last crash message (from crash.log)\n");
    }
    preview.push_str("\nContinue?");
    if crate::ui::dialogs::confirm(&preview) {
        crate::shell::open_url(&issue_url(&crate::integration::update_repo(), &diag, crash.as_deref()));
    }
}

/// At startup: if Snap It crashed since the last start, offer to report it.
pub fn check_previous_crash() {
    let Some(t) = crash_log_time() else { return };
    if t <= app::settings().last_crash_seen {
        return;
    }
    app::update_settings(|s| s.last_crash_seen = t);
    app::with(|s| s.pending_click = Some(app::Click::ReportBug));
    app::post(Command::Notify { title: "Snap It closed unexpectedly last time".into(), body: "Click here to report the problem.".into() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_reserved_and_unicode() {
        assert_eq!(encode("a b&c#d\n×"), "a%20b%26c%23d%0A%C3%97");
    }

    #[test]
    fn bad_repos_fall_back() {
        let d = crate::settings::DEFAULT_UPDATE_REPO;
        assert_eq!(safe_repo("someone/fork"), "someone/fork");
        for bad in ["", "../x", "a/b/c", "javascript:alert(1)", "a/..", "a b/c", "/x"] {
            assert_eq!(safe_repo(bad), d, "{bad}");
        }
    }

    #[test]
    fn url_stays_short_with_huge_crash() {
        let crash = "x".repeat(50_000);
        let url = issue_url("a/b", "- Snap It: 1.0", Some(&crash));
        assert!(url.starts_with("https://github.com/a/b/issues/new?title="));
        assert!(url.len() <= MAX_URL, "{}", url.len());
        assert!(url.contains("Snap%20It%3A%201.0"));
    }

    #[test]
    fn trim_keeps_newest_entry() {
        let mut log = String::new();
        for i in 0..200 {
            log += &format!("{ENTRY_MARK}entry {i}\npanic text {i}\n");
        }
        let t = trim_log(&log, 300);
        assert!(t.starts_with(ENTRY_MARK) && t.len() <= 300);
        assert!(t.ends_with("panic text 199\n"));
        assert_eq!(newest_entry(&log, 1500).unwrap(), format!("{ENTRY_MARK}entry 199\npanic text 199"));
    }

    #[test]
    fn append_rotates() {
        let dir = std::env::temp_dir().join(format!("snapit-crashlog-{}", std::process::id()));
        let p = dir.join("crash.log");
        let _ = std::fs::remove_file(&p);
        let big = "y".repeat(1000);
        for i in 0..100 {
            append_entry(&p, &format!("{ENTRY_MARK}{i}\n{big}\n")).unwrap();
        }
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.len() <= MAX_LOG);
        assert!(s.starts_with(ENTRY_MARK));
        assert!(newest_entry(&s, 10).unwrap().starts_with("=== 99"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
