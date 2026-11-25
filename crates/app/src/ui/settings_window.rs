//! The settings window, built from standard Win32 controls.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, FillRect, GetSysColor, GetSysColorBrush, SetBkColor,
    COLOR_WINDOW, HBRUSH, HDC, HFONT,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, NMHDR,
    TCIF_TEXT, TCITEMW, TCM_GETCURSEL, TCM_INSERTITEMW, TCN_SELCHANGE, WC_TABCONTROLW,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, SendMessageW,
    SetForegroundWindow, SetWindowTextW, ShowWindow, BM_GETCHECK, BM_SETCHECK, BN_CLICKED,
    BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CBS_DROPDOWNLIST, CB_ADDSTRING,
    CB_GETCURSEL, CB_SETCURSEL, ES_AUTOHSCROLL, ES_NUMBER, HMENU, NONCLIENTMETRICSW,
    SPI_GETNONCLIENTMETRICS, SW_HIDE, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND,
    WM_NOTIFY, WM_SETFONT, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::{Handler, Win};
use crate::actions::Action;
use crate::app::{self, Command};
use crate::settings::{Settings, Theme, Workflow};
use snapit_gfx::io::Format;

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

const TABS: [&str; 6] = ["General", "Capture", "After capture", "Hotkeys", "Recording", "Integration"];

/// Actions that can have a hotkey, in display order.
const HOTKEY_ACTIONS: [Action; 17] = [
    Action::CaptureRegion,
    Action::CaptureWindow,
    Action::CaptureActiveWindow,
    Action::CaptureMonitor,
    Action::CaptureFullscreen,
    Action::CaptureLastRegion,
    Action::CaptureCustomRegion,
    Action::CaptureScrolling,
    Action::RecordMp4,
    Action::RecordGif,
    Action::Ocr,
    Action::ScanQr,
    Action::ColorPicker,
    Action::Ruler,
    Action::OpenEditor,
    Action::OpenHistory,
    Action::OpenSettings,
];

// Control ids.
const ID_TAB: u16 = 100;
pub const ID_SAVE: u16 = 101;
const ID_CANCEL: u16 = 102;
const ID_SAVE_DIR: u16 = 200;
const ID_BROWSE: u16 = 201;
const ID_PATTERN: u16 = 202;
const ID_FORMAT: u16 = 203;
const ID_JPEG_Q: u16 = 204;
const ID_THEME: u16 = 205;
const ID_HISTORY: u16 = 206;
const ID_CURSOR: u16 = 300;
const ID_DELAY: u16 = 301;
const ID_QUICK: u16 = 302;
const ID_DETECT_WIN: u16 = 303;
const ID_DETECT_CTL: u16 = 304;
const ID_MAGNIFIER: u16 = 305;
const ID_CROSSHAIR: u16 = 306;
const ID_DIM: u16 = 307;
const ID_SCROLLING: u16 = 308;
const ID_CLEAR_CUSTOM: u16 = 309;
const ID_T_COPY: u16 = 400;
const ID_T_SAVE: u16 = 401;
const ID_T_EDITOR: u16 = 402;
const ID_T_PIN: u16 = 403;
const ID_T_NOTIFY: u16 = 404;
const ID_T_SOUND: u16 = 405;
const ID_HOTKEY0: u16 = 500;
const ID_MP4_FPS: u16 = 600;
const ID_MP4_KBPS: u16 = 601;
const ID_GIF_FPS: u16 = 602;
const ID_GIF_MAX: u16 = 603;
const ID_REC_CURSOR: u16 = 604;
const ID_STARTUP: u16 = 700;
const ID_CTXMENU: u16 = 701;
const ID_UPDATE: u16 = 702;
const ID_REPO: u16 = 703;
const ID_CHECK_NOW: u16 = 704;
const ID_OPEN_CONFIG: u16 = 705;

struct SettingsWin {
    settings: Settings,
    font: HFONT,
    scale: f32,
    tab: HWND,
    pages: Vec<Vec<HWND>>,
    ctl: HashMap<u16, HWND>,
    built: bool,
}

pub fn open() {
    let existing = OPEN.with(|o| o.get());
    if existing != 0 {
        unsafe {
            let h = HWND(existing as *mut _);
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
        return;
    }
    unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_TAB_CLASSES | ICC_STANDARD_CLASSES };
        let _ = InitCommonControlsEx(&icc);
    }
    let sw = SettingsWin { settings: (*app::settings()).clone(), font: HFONT::default(), scale: 1.0, tab: HWND::default(), pages: Vec::new(), ctl: HashMap::new(), built: false };
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((snapit_gfx::Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h) = ((640.0 * sc) as i32, (700.0 * sc) as i32);
    let r = snapit_gfx::Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));
    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPSIBLINGS;
    match super::create("SnapItSettings", WS_EX_CONTROLPARENT, style, "Snap It Settings", r, None, Box::new(sw)) {
        Ok(hwnd) => {
            OPEN.with(|o| o.set(hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("settings: {e}")),
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

impl SettingsWin {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    fn make(&mut self, parent: HWND, page: Option<usize>, class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32, w: i32, h: i32, id: u16) -> HWND {
        let visible = if page.is_none_or(|p| p == 0) { WS_VISIBLE } else { WINDOW_STYLE(0) };
        let hwnd = unsafe {
            CreateWindowExW(ex, class, &HSTRING::from(text), WS_CHILD | visible | style, self.px(x), self.px(y), self.px(w), self.px(h), Some(parent), Some(HMENU(id as usize as *mut _)), None, None)
        }
        .unwrap_or_default();
        unsafe {
            SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
        }
        if let Some(p) = page {
            self.pages[p].push(hwnd);
        }
        if id != 0 {
            self.ctl.insert(id, hwnd);
        }
        hwnd
    }

    fn label(&mut self, parent: HWND, page: usize, text: &str, x: i32, y: i32, w: i32) {
        self.make(parent, Some(page), w!("STATIC"), text, WINDOW_STYLE(0), WINDOW_EX_STYLE(0), x, y + 3, w, 20, 0);
    }

    fn edit(&mut self, parent: HWND, page: usize, id: u16, value: &str, x: i32, y: i32, w: i32, numeric: bool) {
        let mut st = WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        if numeric {
            st |= WINDOW_STYLE(ES_NUMBER as u32);
        }
        let h = self.make(parent, Some(page), w!("EDIT"), value, st, WINDOW_EX_STYLE(0), x, y, w, 24, id);
        set_text(h, value);
    }

    fn check(&mut self, parent: HWND, page: usize, id: u16, text: &str, on: bool, x: i32, y: i32, w: i32) {
        let h = self.make(parent, Some(page), w!("BUTTON"), text, WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0), x, y, w, 22, id);
        unsafe {
            SendMessageW(h, BM_SETCHECK, Some(WPARAM(on as usize)), None);
        }
    }

    fn button(&mut self, parent: HWND, page: Option<usize>, id: u16, text: &str, x: i32, y: i32, w: i32, default: bool) {
        let st = if default { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON };
        self.make(parent, page, w!("BUTTON"), text, WS_TABSTOP | WINDOW_STYLE(st as u32), WINDOW_EX_STYLE(0), x, y, w, 28, id);
    }

    fn combo(&mut self, parent: HWND, page: usize, id: u16, items: &[&str], sel: usize, x: i32, y: i32, w: i32) {
        let h = self.make(parent, Some(page), w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32), WINDOW_EX_STYLE(0), x, y, w, 200, id);
        for it in items {
            unsafe {
                SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(HSTRING::from(*it).as_ptr() as isize)));
            }
        }
        unsafe {
            SendMessageW(h, CB_SETCURSEL, Some(WPARAM(sel)), None);
        }
    }

    fn is_checked(&self, id: u16) -> bool {
        self.ctl.get(&id).is_some_and(|&h| unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 == 1 })
    }

    fn combo_sel(&self, id: u16) -> usize {
        self.ctl.get(&id).map_or(0, |&h| unsafe { SendMessageW(h, CB_GETCURSEL, None, None).0.max(0) as usize })
    }

    fn text_of(&self, id: u16) -> String {
        self.ctl.get(&id).map(|&h| get_text(h)).unwrap_or_default().trim().to_string()
    }

    fn num(&self, id: u16, default: u32, lo: u32, hi: u32) -> u32 {
        self.text_of(id).parse().unwrap_or(default).clamp(lo, hi)
    }

    fn build(&mut self, hwnd: HWND) {
        self.scale = super::dpi_scale(hwnd);
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
            if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0, dpi).is_ok() {
                self.font = CreateFontIndirectW(&ncm.lfMessageFont);
            }
        }
        self.pages = vec![Vec::new(); TABS.len()];
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let cw = (rc.right as f32 / self.scale) as i32;
        let ch = (rc.bottom as f32 / self.scale) as i32;

        self.tab = self.make(hwnd, None, WC_TABCONTROLW, "", WS_CLIPSIBLINGS | WS_TABSTOP, WINDOW_EX_STYLE(0), 10, 10, cw - 20, ch - 64, ID_TAB);
        for (i, name) in TABS.iter().enumerate() {
            let label = HSTRING::from(*name);
            let item = TCITEMW { mask: TCIF_TEXT, pszText: windows::core::PWSTR(label.as_ptr() as *mut _), ..Default::default() };
            unsafe {
                SendMessageW(self.tab, TCM_INSERTITEMW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
            }
        }
        let s = self.settings.clone();
        let (x0, lw, fx) = (30, 170, 210);
        let fw = cw - fx - 40;

        // General
        let mut y = 50;
        self.label(hwnd, 0, "Save screenshots to", x0, y, lw);
        let dir = s.save_dir.clone().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.edit(hwnd, 0, ID_SAVE_DIR, &dir, fx, y, fw - 90, false);
        self.button(hwnd, Some(0), ID_BROWSE, "Browse…", fx + fw - 84, y - 2, 84, false);
        y += 26;
        self.label(hwnd, 0, &format!("Empty = {}", crate::paths::default_save_dir().display()), fx, y, fw);
        y += 34;
        self.label(hwnd, 0, "File name pattern", x0, y, lw);
        self.edit(hwnd, 0, ID_PATTERN, &s.filename_pattern, fx, y, fw, false);
        y += 26;
        self.label(hwnd, 0, "%y %mo %d %h %mi %s %ms  date/time · %width %height · %title · %rnd", fx, y, fw);
        y += 34;
        self.label(hwnd, 0, "Image format", x0, y, lw);
        let fsel = match s.format {
            Format::Png => 0,
            Format::Jpg => 1,
            Format::Webp => 2,
        };
        self.combo(hwnd, 0, ID_FORMAT, &["PNG", "JPEG", "WebP (lossless)"], fsel, fx, y, 160);
        y += 36;
        self.label(hwnd, 0, "JPEG quality (1–100)", x0, y, lw);
        self.edit(hwnd, 0, ID_JPEG_Q, &s.jpeg_quality.to_string(), fx, y, 60, true);
        y += 36;
        self.label(hwnd, 0, "Theme", x0, y, lw);
        let tsel = match s.theme {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        };
        self.combo(hwnd, 0, ID_THEME, &["Follow Windows", "Light", "Dark"], tsel, fx, y, 160);
        y += 36;
        self.label(hwnd, 0, "History entries kept", x0, y, lw);
        self.edit(hwnd, 0, ID_HISTORY, &s.history_limit.to_string(), fx, y, 80, true);
        y += 40;
        let mode = if crate::paths::portable() { "Portable mode: settings are stored next to snapit.exe." } else { "Settings are stored in %APPDATA%\\SnapIt. Put a file named \"portable\" next to snapit.exe for portable mode." };
        self.label(hwnd, 0, mode, x0, y, cw - 60);

        // Capture
        let mut y = 50;
        self.check(hwnd, 1, ID_CURSOR, "Include the mouse pointer in screenshots", s.capture_cursor, x0, y, cw - 60);
        y += 30;
        self.label(hwnd, 1, "Delay before capture (ms)", x0, y, 200);
        self.edit(hwnd, 1, ID_DELAY, &s.capture_delay_ms.to_string(), 240, y, 80, true);
        y += 26;
        self.label(hwnd, 1, "Applies to fullscreen, monitor, window, last and custom region captures.", x0, y, cw - 60);
        y += 36;
        self.label(hwnd, 1, "Region selection", x0, y, 200);
        y += 26;
        self.check(hwnd, 1, ID_QUICK, "Show the quick-annotate toolbar after selecting a region", s.region.quick_annotate, x0 + 10, y, cw - 70);
        y += 26;
        self.check(hwnd, 1, ID_DETECT_WIN, "Snap to windows under the cursor", s.region.detect_windows, x0 + 10, y, cw - 70);
        y += 26;
        self.check(hwnd, 1, ID_DETECT_CTL, "Snap to controls inside windows (buttons, panels…)", s.region.detect_controls, x0 + 10, y, cw - 70);
        y += 26;
        self.check(hwnd, 1, ID_MAGNIFIER, "Show the magnifier", s.region.magnifier, x0 + 10, y, cw - 70);
        y += 26;
        self.check(hwnd, 1, ID_CROSSHAIR, "Show full-screen crosshair lines", s.region.crosshair, x0 + 10, y, cw - 70);
        y += 30;
        self.label(hwnd, 1, "Dim amount (0–230)", x0 + 10, y, 200);
        self.edit(hwnd, 1, ID_DIM, &s.region.dim_opacity.to_string(), 240, y, 80, true);
        y += 40;
        self.check(hwnd, 1, ID_SCROLLING, "Enable scrolling capture", s.scrolling_capture, x0, y, cw - 60);
        y += 24;
        self.label(hwnd, 1, "Scrolling capture sends mouse-wheel input to the window you select. Off by default.", x0 + 18, y, cw - 80);
        y += 36;
        let custom = match s.custom_region {
            Some(r) => format!("Custom region: {} × {} at {}, {}", r.w, r.h, r.x, r.y),
            None => "Custom region: not set (you'll be asked to draw it the first time)".into(),
        };
        self.label(hwnd, 1, &custom, x0, y, cw - 160);
        self.button(hwnd, Some(1), ID_CLEAR_CUSTOM, "Clear", cw - 120, y - 3, 80, false);

        // After capture
        let mut y = 50;
        self.label(hwnd, 2, "After a capture finishes (Enter in the region overlay, or any instant capture):", x0, y, cw - 60);
        y += 32;
        let t = s.tasks;
        for (id, text, on) in [
            (ID_T_COPY, "Copy the image to the clipboard", t.copy),
            (ID_T_SAVE, "Save the image to the screenshots folder", t.save),
            (ID_T_EDITOR, "Open it in the editor", t.open_editor),
            (ID_T_PIN, "Pin it to the screen", t.pin),
            (ID_T_NOTIFY, "Show a notification", t.notify),
            (ID_T_SOUND, "Play the capture sound", t.sound),
        ] {
            self.check(hwnd, 2, id, text, on, x0 + 10, y, cw - 70);
            y += 28;
        }
        y += 10;
        self.label(hwnd, 2, "Per-hotkey task chains can be set in settings.toml under [[workflows]].", x0, y, cw - 60);

        // Hotkeys
        let mut y = 46;
        self.label(hwnd, 3, "Type a hotkey such as Ctrl+Shift+PrintScreen or Alt+F1. Leave empty for none.", x0, y, cw - 60);
        y += 26;
        for (i, a) in HOTKEY_ACTIONS.iter().enumerate() {
            let cur = s.hotkey_for(*a).unwrap_or_default();
            let label = match a {
                Action::OpenEditor => "Open image editor",
                Action::OpenHistory => "Open history",
                Action::OpenSettings => "Open settings",
                _ => a.label(),
            };
            self.label(hwnd, 3, label.trim_end_matches('…'), x0, y, 200);
            self.edit(hwnd, 3, ID_HOTKEY0 + i as u16, &cur, 240, y, 220, false);
            y += 28;
        }

        // Recording
        let mut y = 50;
        let r = &s.recording;
        for (id, text, v) in [
            (ID_MP4_FPS, "MP4 frames per second (5–60)", r.mp4_fps),
            (ID_MP4_KBPS, "MP4 bitrate (kbit/s)", r.mp4_bitrate_kbps),
            (ID_GIF_FPS, "GIF frames per second (2–30)", r.gif_fps),
            (ID_GIF_MAX, "GIF maximum length (seconds)", r.gif_max_seconds),
        ] {
            self.label(hwnd, 4, text, x0, y, 220);
            self.edit(hwnd, 4, id, &v.to_string(), 260, y, 80, true);
            y += 34;
        }
        self.check(hwnd, 4, ID_REC_CURSOR, "Show the mouse pointer in recordings", r.cursor, x0, y, cw - 60);
        y += 34;
        self.label(hwnd, 4, "Recordings never include audio. Press the record hotkey again (or the Stop button) to finish.", x0, y, cw - 60);

        // Integration
        let mut y = 50;
        let ig = &s.integration;
        self.check(hwnd, 5, ID_STARTUP, "Start Snap It when I sign in (adds a shortcut to the Startup folder)", ig.start_with_windows, x0, y, cw - 60);
        y += 30;
        self.check(hwnd, 5, ID_CTXMENU, "Add \"Edit with Snap It\" and \"Pin with Snap It\" to the Explorer menu for images", ig.context_menu, x0, y, cw - 60);
        y += 30;
        self.check(hwnd, 5, ID_UPDATE, "Check for new versions at startup (notify only, nothing is downloaded)", ig.update_check, x0, y, cw - 60);
        y += 32;
        self.label(hwnd, 5, "Release repository (owner/repo)", x0 + 18, y, 220);
        self.edit(hwnd, 5, ID_REPO, &ig.update_repo, 260, y, 200, false);
        self.button(hwnd, Some(5), ID_CHECK_NOW, "Check now", 470, y - 2, 100, false);
        y += 50;
        self.button(hwnd, Some(5), ID_OPEN_CONFIG, "Open settings folder", x0, y, 170, false);

        // Bottom buttons
        self.button(hwnd, None, ID_SAVE, "Save", cw - 200, ch - 44, 90, true);
        self.button(hwnd, None, ID_CANCEL, "Cancel", cw - 100, ch - 44, 90, false);
        let _ = WS_EX_CLIENTEDGE;
        self.built = true;
    }

    fn show_page(&self, page: usize) {
        for (i, ctrls) in self.pages.iter().enumerate() {
            for &c in ctrls {
                unsafe {
                    let _ = ShowWindow(c, if i == page { SW_SHOW } else { SW_HIDE });
                }
            }
        }
    }

    fn browse(&self, hwnd: HWND) {
        unsafe {
            let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
            if let Ok(opts) = dlg.GetOptions() {
                let _ = dlg.SetOptions(opts | FOS_PICKFOLDERS);
            }
            if dlg.Show(Some(hwnd)).is_ok() {
                if let Ok(item) = dlg.GetResult() {
                    if let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                        if let (Ok(s), Some(&h)) = (p.to_string(), self.ctl.get(&ID_SAVE_DIR)) {
                            set_text(h, &s);
                        }
                        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                    }
                }
            }
        }
    }

    /// Reads the controls into a new Settings. Errors describe what to fix.
    fn collect(&self) -> Result<Settings, String> {
        let mut s = self.settings.clone();
        let dir = self.text_of(ID_SAVE_DIR);
        s.save_dir = if dir.is_empty() { None } else { Some(PathBuf::from(dir)) };
        let pat = self.text_of(ID_PATTERN);
        s.filename_pattern = if pat.is_empty() { Settings::default().filename_pattern } else { pat };
        s.format = [Format::Png, Format::Jpg, Format::Webp][self.combo_sel(ID_FORMAT).min(2)];
        s.jpeg_quality = self.num(ID_JPEG_Q, 90, 1, 100) as u8;
        s.theme = [Theme::System, Theme::Light, Theme::Dark][self.combo_sel(ID_THEME).min(2)];
        s.history_limit = self.num(ID_HISTORY, 500, 10, 100_000) as usize;

        s.capture_cursor = self.is_checked(ID_CURSOR);
        s.capture_delay_ms = self.num(ID_DELAY, 0, 0, 60_000);
        s.region.quick_annotate = self.is_checked(ID_QUICK);
        s.region.detect_windows = self.is_checked(ID_DETECT_WIN);
        s.region.detect_controls = self.is_checked(ID_DETECT_CTL);
        s.region.magnifier = self.is_checked(ID_MAGNIFIER);
        s.region.crosshair = self.is_checked(ID_CROSSHAIR);
        s.region.dim_opacity = self.num(ID_DIM, 110, 0, 230) as u8;
        s.scrolling_capture = self.is_checked(ID_SCROLLING);

        s.tasks.copy = self.is_checked(ID_T_COPY);
        s.tasks.save = self.is_checked(ID_T_SAVE);
        s.tasks.open_editor = self.is_checked(ID_T_EDITOR);
        s.tasks.pin = self.is_checked(ID_T_PIN);
        s.tasks.notify = self.is_checked(ID_T_NOTIFY);
        s.tasks.sound = self.is_checked(ID_T_SOUND);

        // Hotkeys: validate every field first.
        let mut chosen: Vec<(Action, crate::hotkeys::Chord)> = Vec::new();
        for (i, a) in HOTKEY_ACTIONS.iter().enumerate() {
            let text = self.text_of(ID_HOTKEY0 + i as u16);
            if text.is_empty() {
                continue;
            }
            let chord = crate::hotkeys::Chord::parse(&text).ok_or_else(|| format!("\"{text}\" (for {}) isn't a hotkey I understand.", a.label()))?;
            if let Some((other, _)) = chosen.iter().find(|(_, c)| *c == chord) {
                return Err(format!("{} is used for both {} and {}.", chord.display(), other.label(), a.label()));
            }
            chosen.push((*a, chord));
        }
        // Then update workflows in place (keeping their order and task overrides),
        // drop cleared ones, and append newly assigned actions.
        let mut workflows = Vec::new();
        for wf in &self.settings.workflows {
            match HOTKEY_ACTIONS.iter().find(|a| a.name() == wf.action) {
                None => workflows.push(wf.clone()), // hand-edited extras are kept as-is
                Some(a) => {
                    if let Some((_, chord)) = chosen.iter().find(|(x, _)| x == a) {
                        if !workflows.iter().any(|w: &Workflow| w.action == wf.action) {
                            let same = crate::hotkeys::Chord::parse(&wf.hotkey) == Some(*chord);
                            let hotkey = if same { wf.hotkey.clone() } else { chord.display() };
                            workflows.push(Workflow { hotkey, ..wf.clone() });
                        }
                    }
                }
            }
        }
        for (a, chord) in &chosen {
            if !workflows.iter().any(|w| w.action == a.name()) {
                workflows.push(Workflow { hotkey: chord.display(), action: a.name().into(), tasks: None });
            }
        }
        s.workflows = workflows;

        s.recording.mp4_fps = self.num(ID_MP4_FPS, 30, 5, 60);
        s.recording.mp4_bitrate_kbps = self.num(ID_MP4_KBPS, 8000, 500, 100_000);
        s.recording.gif_fps = self.num(ID_GIF_FPS, 15, 2, 30);
        s.recording.gif_max_seconds = self.num(ID_GIF_MAX, 60, 1, 600);
        s.recording.cursor = self.is_checked(ID_REC_CURSOR);

        s.integration.start_with_windows = self.is_checked(ID_STARTUP);
        s.integration.context_menu = self.is_checked(ID_CTXMENU);
        s.integration.update_check = self.is_checked(ID_UPDATE);
        s.integration.update_repo = self.text_of(ID_REPO);
        Ok(s)
    }

    fn save(&mut self, w: &mut Win) {
        match self.collect() {
            Ok(next) => {
                let prev = app::settings();
                let errors = crate::integration::apply(&prev.integration, &next.integration);
                if let Err(e) = next.save() {
                    super::dialogs::info(Some(w.hwnd), "Snap It", &format!("Could not save settings.\n{e}"));
                    return;
                }
                app::with(|st| st.settings = std::rc::Rc::new(next));
                app::post(Command::ReloadSettings);
                if !errors.is_empty() {
                    super::dialogs::info(Some(w.hwnd), "Snap It", &errors.join("\n"));
                }
                w.close();
            }
            Err(e) => super::dialogs::info(Some(w.hwnd), "Snap It", &e),
        }
    }
}

impl Handler for SettingsWin {
    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if !self.built && msg == windows::Win32::UI::WindowsAndMessaging::WM_SHOWWINDOW {
            self.build(w.hwnd);
            super::theme::apply_titlebar(w.hwnd, &super::theme::LIGHT);
        }
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(w.hwnd, &mut rc);
                    FillRect(HDC(wp.0 as *mut _), &rc, GetSysColorBrush(COLOR_WINDOW));
                }
                Some(LRESULT(1))
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => unsafe {
                SetBkColor(HDC(wp.0 as *mut _), windows::Win32::Foundation::COLORREF(GetSysColor(COLOR_WINDOW)));
                let b: HBRUSH = GetSysColorBrush(COLOR_WINDOW);
                Some(LRESULT(b.0 as isize))
            },
            WM_NOTIFY => {
                let hdr = unsafe { &*(lp.0 as *const NMHDR) };
                if hdr.idFrom == ID_TAB as usize && hdr.code == TCN_SELCHANGE {
                    let page = unsafe { SendMessageW(self.tab, TCM_GETCURSEL, None, None).0.max(0) as usize };
                    self.show_page(page);
                }
                None
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as u16;
                let code = ((wp.0 >> 16) & 0xFFFF) as u32;
                if code != BN_CLICKED {
                    return None;
                }
                match id {
                    ID_SAVE => self.save(w),
                    ID_CANCEL => w.close(),
                    ID_BROWSE => self.browse(w.hwnd),
                    ID_CLEAR_CUSTOM => {
                        self.settings.custom_region = None;
                        super::dialogs::info(Some(w.hwnd), "Snap It", "The custom region will be cleared when you save.");
                    }
                    ID_CHECK_NOW => {
                        let repo = self.text_of(ID_REPO);
                        app::update_settings(|s| s.integration.update_repo = repo);
                        crate::integration::check_for_updates(true);
                    }
                    ID_OPEN_CONFIG => crate::shell::open_folder(&crate::paths::config_dir()),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                if !self.font.is_invalid() {
                    unsafe {
                        let _ = DeleteObject(self.font.into());
                    }
                }
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}
