//! First-launch setup: a short wizard shown the first time Snap It starts (no settings file
//! yet), so new users pick their hotkeys and decide on the opt-in features before using it.
//!
//! It opens from the app itself rather than the installers, so it works the same for the
//! setup .exe, the MSI and the portable zip, and always writes the signed-in user's settings
//! (an all-users install runs elevated, possibly as another account). Both installers end
//! with "Launch Snap It", which opens it. It can be run again from Settings > General or
//! with `snapit --welcome`.
//!
//! Steps: Welcome (key features) > Hotkeys > Features (opt-in) > After a capture > Done.
//!
//! Re-showing after updates: bump [`SETUP_VERSION`] in a release that adds something to these
//! steps (a new opt-in feature, a new hotkey), and tag the new item with that number in its
//! `since` field. Users whose `setup_version` is older see setup again once, with their
//! current settings filled in and the new items marked "(new)". Bug-fix releases leave the
//! number alone, so setup doesn't come back.

use std::cell::Cell;
use std::collections::HashMap;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, DeleteObject, HDC, HFONT};
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, SendMessageW, SetForegroundWindow,
    SetWindowTextW, ShowWindow, BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, HMENU, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SW_HIDE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_NOTIFY, WM_SETFONT,
    WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};

use super::{Handler, Win};
use crate::actions::Action;
use crate::app::{self, Command};
use crate::hotkeys::Chord;
use crate::settings::{Settings, Workflow};

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

/// The version of these setup steps. Bump it only when a release adds something to them
/// (see the module docs); every user with an older `setup_version` then sees setup again.
pub const SETUP_VERSION: u32 = 1;

const STEPS: [&str; 5] = ["Welcome to Snap It", "Choose your hotkeys", "Turn on extra features", "After a capture", "You're all set"];

/// The hotkeys offered here, with the [`SETUP_VERSION`] each was added in. The rest are in
/// Settings > Hotkeys.
const HOTKEY_ACTIONS: [(Action, u32); 8] = [
    (Action::CaptureRegion, 1),
    (Action::CaptureWindow, 1),
    (Action::CaptureActiveWindow, 1),
    (Action::CaptureFullscreen, 1),
    (Action::CaptureScrolling, 1),
    (Action::RecordMp4, 1),
    (Action::RecordGif, 1),
    (Action::Ocr, 1),
];

// Control ids.
const ID_TITLE: u16 = 100;
const ID_STEP: u16 = 101;
pub const ID_NEXT: u16 = 102;
const ID_BACK: u16 = 103;
const ID_SKIP: u16 = 104;
const ID_HOTKEY0: u16 = 200;
const ID_CHECK_KEYS: u16 = 220;
const ID_KEY_STATUS: u16 = 221;
const ID_KEYBOARD_SETTINGS: u16 = 222;
const ID_SCROLLING: u16 = 300;
const ID_UPDATE: u16 = 301;
const ID_STARTUP: u16 = 302;
const ID_CTXMENU: u16 = 303;
const ID_QUICK: u16 = 304;
const ID_T_COPY: u16 = 400;
const ID_T_SAVE: u16 = 401;
const ID_T_EDITOR: u16 = 402;
const ID_T_NOTIFY: u16 = 403;
const ID_T_SOUND: u16 = 404;
const ID_SUMMARY: u16 = 500;
const ID_TRY_NOW: u16 = 501;

/// Temporary ids for testing whether chords are free (never left registered).
const PROBE_BASE: u16 = 0xB000;

struct WelcomeWin {
    settings: Settings,
    font: HFONT,
    title_font: HFONT,
    scale: f32,
    page: usize,
    pages: Vec<Vec<HWND>>,
    ctl: HashMap<u16, HWND>,
    built: bool,
    finished: bool,
    /// Opened for a user who already has settings (after an update, or from Settings).
    returning: bool,
    nt: super::native_theme::NativeTheme,
}

/// True when Snap It has never saved settings for this user (or portable folder).
pub fn is_first_run() -> bool {
    !crate::paths::settings_file().exists()
}

/// Whether setup should open by itself: on the first run, or once after an update that
/// added setup steps this user hasn't seen.
pub fn is_due(first_run: bool, s: &Settings) -> bool {
    first_run || s.setup_version < SETUP_VERSION
}

/// Label text, marked "(new)" if the item was added after the setup this user last saw.
/// Nothing is marked for someone who has never seen setup: it's all new to them.
fn tagged(text: &str, since: u32, seen: u32) -> String {
    if seen > 0 && since > seen {
        format!("{text}  (new)")
    } else {
        text.to_string()
    }
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
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES };
        let _ = InitCommonControlsEx(&icc);
    }
    let ww = WelcomeWin {
        settings: (*app::settings()).clone(),
        font: HFONT::default(),
        title_font: HFONT::default(),
        scale: 1.0,
        page: 0,
        pages: Vec::new(),
        ctl: HashMap::new(),
        built: false,
        finished: false,
        returning: !is_first_run(),
        nt: super::native_theme::NativeTheme::new(),
    };
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((snapit_gfx::Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h) = ((620.0 * sc) as i32, (560.0 * sc) as i32);
    let r = snapit_gfx::Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));
    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPSIBLINGS;
    match super::create("SnapItWelcome", WS_EX_CONTROLPARENT, style, "Snap It Setup", r, None, Box::new(ww)) {
        Ok(hwnd) => {
            OPEN.with(|o| o.set(hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("welcome: {e}")),
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

/// Applies the hotkeys chosen here to `workflows`: an empty box removes that action's
/// hotkey, a new one replaces it (keeping any per-hotkey task chain), and workflows for
/// actions not shown here are kept as they are. Errors describe what to fix.
fn apply_hotkeys(workflows: &[Workflow], chosen: &[(Action, String)]) -> Result<Vec<Workflow>, String> {
    let mut parsed: Vec<(Action, Option<Chord>)> = Vec::new();
    for (a, text) in chosen {
        let text = text.trim();
        if text.is_empty() {
            parsed.push((*a, None));
            continue;
        }
        let c = Chord::parse(text).ok_or_else(|| format!("\"{text}\" (for {}) isn't a hotkey I understand.", a.label()))?;
        parsed.push((*a, Some(c)));
    }
    let mut out: Vec<Workflow> = Vec::new();
    for wf in workflows {
        match parsed.iter().find(|(a, _)| a.name() == wf.action) {
            None => out.push(wf.clone()),
            Some((_, None)) => {}
            Some((_, Some(c))) => {
                if !out.iter().any(|w| w.action == wf.action) {
                    let same = Chord::parse(&wf.hotkey) == Some(*c);
                    out.push(Workflow { hotkey: if same { wf.hotkey.clone() } else { c.display() }, ..wf.clone() });
                }
            }
        }
    }
    for (a, c) in &parsed {
        if let Some(c) = c {
            if !out.iter().any(|w| w.action == a.name()) {
                out.push(Workflow { hotkey: c.display(), action: a.name().into(), tasks: None });
            }
        }
    }
    let resolved: Vec<(Chord, Action)> = out.iter().filter_map(Workflow::resolve).collect();
    for (i, (c, a)) in resolved.iter().enumerate() {
        if let Some((_, b)) = resolved[..i].iter().find(|(d, _)| d == c) {
            return Err(format!("{} is used for both {} and {}.", c.display(), b.label(), a.label()));
        }
    }
    Ok(out)
}

/// Which of `chords` another app already owns. Snap It's own hotkeys are released for the
/// test and registered again afterwards.
fn unavailable(chords: &[Chord]) -> Vec<Chord> {
    let hwnd = app::main_hwnd();
    crate::suspend_hotkeys();
    let probe: Vec<(u16, Chord)> = chords.iter().enumerate().map(|(i, c)| (PROBE_BASE + i as u16, *c)).collect();
    let failed = crate::hotkeys::register_all(hwnd, &probe);
    crate::hotkeys::unregister_all(hwnd, &probe);
    let _ = crate::reregister_hotkeys();
    failed
}

impl WelcomeWin {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn make(&mut self, parent: HWND, page: Option<usize>, class: PCWSTR, text: &str, style: WINDOW_STYLE, x: i32, y: i32, w: i32, h: i32, id: u16) -> HWND {
        let visible = if page.is_none_or(|p| p == 0) { WS_VISIBLE } else { WINDOW_STYLE(0) };
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), class, &HSTRING::from(text), WS_CHILD | visible | style, self.px(x), self.px(y), self.px(w), self.px(h), Some(parent), Some(HMENU(id as usize as *mut _)), None, None)
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

    /// A label `lines` text lines tall (it wraps within its width).
    fn label(&mut self, parent: HWND, page: usize, text: &str, x: i32, y: i32, w: i32, lines: i32) -> HWND {
        self.make(parent, Some(page), w!("STATIC"), text, WINDOW_STYLE(0), x, y, w, 18 * lines + 2, 0)
    }

    fn check(&mut self, parent: HWND, page: usize, id: u16, text: &str, on: bool, x: i32, y: i32, w: i32) {
        let h = self.make(parent, Some(page), w!("BUTTON"), text, WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), x, y, w, 22, id);
        unsafe {
            SendMessageW(h, BM_SETCHECK, Some(WPARAM(on as usize)), None);
        }
    }

    fn button(&mut self, parent: HWND, page: Option<usize>, id: u16, text: &str, x: i32, y: i32, w: i32, default: bool) {
        let st = if default { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON };
        self.make(parent, page, w!("BUTTON"), text, WS_TABSTOP | WINDOW_STYLE(st as u32), x, y, w, 28, id);
    }

    fn is_checked(&self, id: u16) -> bool {
        self.ctl.get(&id).is_some_and(|&h| unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 == 1 })
    }

    fn text_of(&self, id: u16) -> String {
        self.ctl.get(&id).map(|&h| get_text(h)).unwrap_or_default().trim().to_string()
    }

    fn set(&self, id: u16, s: &str) {
        if let Some(&h) = self.ctl.get(&id) {
            set_text(h, s);
        }
    }

    fn build(&mut self, hwnd: HWND) {
        self.scale = super::dpi_scale(hwnd);
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
            if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0, dpi).is_ok() {
                self.font = CreateFontIndirectW(&ncm.lfMessageFont);
                let mut big = ncm.lfMessageFont;
                big.lfHeight = (big.lfHeight as f32 * 1.6).round() as i32;
                big.lfWeight = 600;
                self.title_font = CreateFontIndirectW(&big);
            }
        }
        self.pages = vec![Vec::new(); STEPS.len()];
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let cw = (rc.right as f32 / self.scale) as i32;
        let ch = (rc.bottom as f32 / self.scale) as i32;
        let x0 = 28;
        let tw = cw - 2 * x0;
        let s = self.settings.clone();
        let seen = s.setup_version;

        // Header, shared by every step.
        let title = self.make(hwnd, None, w!("STATIC"), STEPS[0], WINDOW_STYLE(0), x0, 18, tw - 110, 34, ID_TITLE);
        unsafe {
            SendMessageW(title, WM_SETFONT, Some(WPARAM(self.title_font.0 as usize)), Some(LPARAM(1)));
        }
        self.make(hwnd, None, w!("STATIC"), "", WINDOW_STYLE(0x0002), cw - x0 - 100, 28, 100, 20, ID_STEP); // SS_RIGHT

        // 1. Welcome: what Snap It does.
        let mut y = 64;
        let intro = if !self.returning {
            "Screenshots, annotation and screen recording that stay on this PC. This short setup takes about a minute; you can change everything later in Settings."
        } else if seen > 0 && seen < SETUP_VERSION {
            "Snap It has new features you may want to set up; they're marked (new). Your current settings are filled in and stay as they are unless you change them."
        } else {
            "A quick tour of Snap It's main settings. Your current settings are filled in and stay as they are unless you change them."
        };
        self.label(hwnd, 0, intro, x0, y, tw, 2);
        y += 50;
        for line in [
            "•  Capture a region, a window or the whole screen, snapping to windows and controls",
            "•  Annotate straight away (arrows, text, highlight, blur, numbered steps) or open the editor",
            "•  Record the screen to MP4 or GIF",
            "•  Scrolling capture for long web pages and documents",
            "•  Copy text with OCR, scan or make QR codes, pick colours, measure with a ruler",
            "•  Pin screenshots above other windows and find older ones in History",
        ] {
            self.label(hwnd, 0, line, x0 + 6, y, tw - 6, 1);
            y += 26;
        }
        y += 14;
        self.label(hwnd, 0, "Snap It runs in the notification area (system tray) next to the clock. Right-click its icon for every action, or double-click it to capture a region.", x0, y, tw, 2);
        y += 48;
        self.label(hwnd, 0, "Nothing is uploaded and no account is needed.", x0, y, tw, 1);

        // 2. Hotkeys.
        let mut y = 64;
        self.label(hwnd, 1, "Click a box and press the keys you want (for example Ctrl+Shift+S). Backspace clears it. More actions can get hotkeys in Settings > Hotkeys.", x0, y, tw, 2);
        y += 48;
        for (i, (a, since)) in HOTKEY_ACTIONS.iter().enumerate() {
            let cur = s.hotkey_for(*a).unwrap_or_default();
            self.label(hwnd, 1, &tagged(a.label().trim_end_matches('…'), *since, seen), x0, y + 3, 200, 1);
            let id = ID_HOTKEY0 + i as u16;
            let h = self.make(hwnd, Some(1), w!("EDIT"), &cur, WS_TABSTOP | WS_BORDER | WINDOW_STYLE(0x0080), 240, y, 220, 24, id); // ES_AUTOHSCROLL
            set_text(h, &cur);
            super::hotkey_field::attach(h);
            y += 30;
        }
        y += 6;
        self.button(hwnd, Some(1), ID_CHECK_KEYS, "Check hotkeys", x0, y, 130, false);
        self.make(hwnd, Some(1), w!("STATIC"), "", WINDOW_STYLE(0), x0 + 142, y + 5, tw - 142, 40, ID_KEY_STATUS);
        y += 48;
        self.label(hwnd, 1, "Windows 11 can reserve the Print Screen key for the Snipping Tool. If PrintScreen shows as taken, turn off \"Use the Print Screen key to open screen capture\".", x0, y, tw, 2);
        y += 42;
        self.button(hwnd, Some(1), ID_KEYBOARD_SETTINGS, "Open Windows keyboard settings", x0, y, 230, false);

        // 3. Opt-in features (all off unless already turned on).
        let mut y = 64;
        self.label(hwnd, 2, "These are off by default. Turn on the ones you want.", x0, y, tw, 1);
        y += 32;
        let ig = &s.integration;
        // (id, text, current value, help, SETUP_VERSION it was added in)
        for (id, text, on, help, since) in [
            (ID_SCROLLING, "Scrolling capture", s.scrolling_capture, "Captures long pages by scrolling the area you select. It sends mouse-wheel input to that window.", 1),
            (ID_UPDATE, "Check for updates when Snap It starts", ig.update_check, "Asks GitHub whether a newer version exists. Nothing is downloaded or installed without asking; downloads open in your browser.", 1),
            (ID_STARTUP, "Start Snap It when I sign in", ig.start_with_windows, "Adds a shortcut to your Startup folder, so your hotkeys work right after sign-in. Snap It starts in the tray.", 1),
            (ID_CTXMENU, "Add Snap It to the Explorer menu for images", ig.context_menu, "Right-click an image file for \"Edit with Snap It\" and \"Pin with Snap It\".", 1),
            (ID_QUICK, "Show the quick-annotate toolbar after selecting a region", s.region.quick_annotate, "Draw, blur or add text before you copy or save. Turn off to finish a capture as soon as you release the mouse.", 1),
        ] {
            self.check(hwnd, 2, id, &tagged(text, since, seen), on, x0, y, tw);
            y += 24;
            self.label(hwnd, 2, help, x0 + 20, y, tw - 20, 2);
            y += 48;
        }

        // 4. After a capture.
        let mut y = 64;
        self.label(hwnd, 3, "What happens when you finish a capture (press Enter after selecting, or use an instant capture such as the full screen):", x0, y, tw, 2);
        y += 50;
        let t = s.tasks;
        for (id, text, on, since) in [
            (ID_T_COPY, "Copy the image to the clipboard", t.copy, 1),
            (ID_T_SAVE, "Save it to the screenshots folder", t.save, 1),
            (ID_T_EDITOR, "Open it in the editor", t.open_editor, 1),
            (ID_T_NOTIFY, "Show a notification (click it to open the capture)", t.notify, 1),
            (ID_T_SOUND, "Play the capture sound", t.sound, 1),
        ] {
            self.check(hwnd, 3, id, &tagged(text, since, seen), on, x0 + 6, y, tw - 6);
            y += 30;
        }
        y += 10;
        self.label(hwnd, 3, &format!("Screenshots are saved to {}", s.save_dir().display()), x0, y, tw, 2);
        y += 40;
        self.label(hwnd, 3, "The quick-annotate toolbar also has its own Copy, Save, Pin and Edit buttons for one-off choices.", x0, y, tw, 2);

        // 5. Done.
        let y = 64;
        self.make(hwnd, Some(4), w!("STATIC"), "", WINDOW_STYLE(0), x0, y, tw, 260, ID_SUMMARY);
        self.check(hwnd, 4, ID_TRY_NOW, "Take a screenshot when I click Finish", false, x0, y + 280, tw);

        // Footer.
        self.button(hwnd, None, ID_SKIP, "Skip setup", x0, ch - 46, 110, false);
        self.button(hwnd, None, ID_BACK, "< Back", cw - x0 - 200, ch - 46, 95, false);
        self.button(hwnd, None, ID_NEXT, "Next >", cw - x0 - 95, ch - 46, 95, true);
        self.built = true;
        self.show_page(0);
    }

    fn show_page(&mut self, page: usize) {
        self.page = page;
        for (i, ctrls) in self.pages.iter().enumerate() {
            for &c in ctrls {
                unsafe {
                    let _ = ShowWindow(c, if i == page { SW_SHOW } else { SW_HIDE });
                }
            }
        }
        self.set(ID_TITLE, STEPS[page]);
        self.set(ID_STEP, &format!("Step {} of {}", page + 1, STEPS.len()));
        let last = page + 1 == STEPS.len();
        self.set(ID_NEXT, if last { "Finish" } else { "Next >" });
        if let Some(&h) = self.ctl.get(&ID_BACK) {
            unsafe {
                let _ = EnableWindow(h, page > 0);
            }
        }
        if last {
            self.set(ID_SUMMARY, &self.summary());
        }
    }

    fn chosen_hotkeys(&self) -> Vec<(Action, String)> {
        HOTKEY_ACTIONS.iter().enumerate().map(|(i, (a, _))| (*a, self.text_of(ID_HOTKEY0 + i as u16))).collect()
    }

    /// Tests the chosen hotkeys and shows the result. Returns the ones that are taken.
    fn check_keys(&self) -> Vec<Chord> {
        let chords: Vec<Chord> = self.chosen_hotkeys().iter().filter_map(|(_, t)| Chord::parse(t)).collect();
        let taken = unavailable(&chords);
        let msg = if chords.is_empty() {
            "No hotkeys set. You can still use the tray icon.".to_string()
        } else if taken.is_empty() {
            "All of these hotkeys are free.".to_string()
        } else {
            let names: Vec<String> = taken.iter().map(Chord::display).collect();
            format!("In use by another app: {}. Pick other keys, or close that app.", names.join(", "))
        };
        self.set(ID_KEY_STATUS, &msg);
        taken
    }

    fn summary(&self) -> String {
        let mut lines = Vec::new();
        match self.chosen_hotkeys().into_iter().find(|(a, t)| *a == Action::CaptureRegion && !t.is_empty()) {
            Some((_, t)) => lines.push(format!("Press {t} to capture a region.")),
            None => lines.push("Capture a region by double-clicking the Snap It tray icon.".into()),
        }
        lines.push("While selecting: drag to choose an area, click to take a whole window, press Enter to finish or Esc to cancel.".into());
        let mut on = Vec::new();
        for (id, name) in [(ID_SCROLLING, "scrolling capture"), (ID_UPDATE, "update checks"), (ID_STARTUP, "start at sign-in"), (ID_CTXMENU, "Explorer menu")] {
            if self.is_checked(id) {
                on.push(name);
            }
        }
        lines.push(if on.is_empty() { "Extra features: none turned on.".into() } else { format!("Turned on: {}.", on.join(", ")) });
        lines.push("To change anything later, right-click the tray icon and choose Settings.".into());
        lines.join("\n\n")
    }

    /// Reads every step into a new Settings.
    fn collect(&self) -> Result<Settings, String> {
        let mut s = self.settings.clone();
        s.workflows = apply_hotkeys(&s.workflows, &self.chosen_hotkeys())?;
        s.scrolling_capture = self.is_checked(ID_SCROLLING);
        s.integration.update_check = self.is_checked(ID_UPDATE);
        s.integration.start_with_windows = self.is_checked(ID_STARTUP);
        s.integration.context_menu = self.is_checked(ID_CTXMENU);
        s.region.quick_annotate = self.is_checked(ID_QUICK);
        s.tasks.copy = self.is_checked(ID_T_COPY);
        s.tasks.save = self.is_checked(ID_T_SAVE);
        s.tasks.open_editor = self.is_checked(ID_T_EDITOR);
        s.tasks.notify = self.is_checked(ID_T_NOTIFY);
        s.tasks.sound = self.is_checked(ID_T_SOUND);
        s.setup_version = s.setup_version.max(SETUP_VERSION);
        Ok(s)
    }

    fn next(&mut self, w: &mut Win) {
        if self.page == 1 {
            // Leaving the hotkeys step: stop on mistakes, warn about keys another app owns.
            if let Err(e) = apply_hotkeys(&self.settings.workflows, &self.chosen_hotkeys()) {
                return super::dialogs::info(Some(w.hwnd), "Snap It", &e);
            }
            let taken = self.check_keys();
            if !taken.is_empty() && !super::headless() {
                let names: Vec<String> = taken.iter().map(Chord::display).collect();
                if !super::dialogs::confirm(&format!("{} won't work while another app uses it.\n\nContinue anyway?", names.join(", "))) {
                    return;
                }
            }
        }
        if self.page + 1 < STEPS.len() {
            return self.show_page(self.page + 1);
        }
        self.finish(w);
    }

    fn finish(&mut self, w: &mut Win) {
        let next = match self.collect() {
            Ok(n) => n,
            Err(e) => return super::dialogs::info(Some(w.hwnd), "Snap It", &e),
        };
        let prev = app::settings();
        let errors = crate::integration::apply(&prev.integration, &next.integration);
        if let Err(e) = next.save() {
            return super::dialogs::info(Some(w.hwnd), "Snap It", &format!("Could not save settings.\n{e}"));
        }
        let check_updates = next.integration.update_check && !prev.integration.update_check;
        app::with(|st| st.settings = std::rc::Rc::new(next));
        app::post(Command::ReloadSettings);
        if check_updates {
            crate::integration::check_for_updates(false);
        }
        if !errors.is_empty() {
            super::dialogs::info(Some(w.hwnd), "Snap It", &errors.join("\n"));
        }
        if self.is_checked(ID_TRY_NOW) {
            app::post(Command::Run(Action::CaptureRegion));
        } else {
            app::post(Command::Notify { title: "Snap It is ready".into(), body: "It's in the notification area. Right-click the icon for every action.".into() });
        }
        self.finished = true;
        w.close();
    }
}

impl Handler for WelcomeWin {
    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if !self.built && msg == windows::Win32::UI::WindowsAndMessaging::WM_SHOWWINDOW {
            self.build(w.hwnd);
            self.nt.style_children(w.hwnd);
        }
        if msg == super::WM_SNAPIT_THEME {
            self.nt.refresh();
            self.nt.style_children(w.hwnd);
            return None;
        }
        match msg {
            WM_ERASEBKGND => {
                self.nt.erase(w.hwnd, HDC(wp.0 as *mut _));
                Some(LRESULT(1))
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => self.nt.ctlcolor(msg, wp),
            WM_NOTIFY => self.nt.custom_draw(lp),
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as u16;
                let code = ((wp.0 >> 16) & 0xFFFF) as u32;
                if code != BN_CLICKED {
                    return None;
                }
                match id {
                    ID_NEXT => self.next(w),
                    ID_BACK if self.page > 0 => self.show_page(self.page - 1),
                    ID_SKIP => w.close(),
                    ID_CHECK_KEYS => {
                        if let Err(e) = apply_hotkeys(&self.settings.workflows, &self.chosen_hotkeys()) {
                            self.set(ID_KEY_STATUS, &e);
                        } else {
                            self.check_keys();
                        }
                    }
                    ID_KEYBOARD_SETTINGS => crate::shell::open_keyboard_settings(),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                super::hotkey_field::resume_hotkeys();
                // Skipped or closed: change nothing but the setup version, so setup doesn't
                // come back at every start (it can be run again from Settings). On the first
                // run this also writes the defaults.
                if !self.finished && (is_first_run() || app::settings().setup_version < SETUP_VERSION) {
                    app::update_settings(|s| s.setup_version = SETUP_VERSION);
                }
                for f in [self.font, self.title_font] {
                    if !f.is_invalid() {
                        unsafe {
                            let _ = DeleteObject(f.into());
                        }
                    }
                }
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(hotkey: &str, action: Action) -> Workflow {
        Workflow::new(hotkey, action)
    }

    #[test]
    fn hotkeys_replace_clear_and_keep_others() {
        let mut tasks_wf = wf("Shift+PrintScreen", Action::CaptureActiveWindow);
        tasks_wf.tasks = Some(crate::settings::Tasks { pin: true, ..Default::default() });
        let old = vec![wf("PrintScreen", Action::CaptureRegion), tasks_wf, wf("Ctrl+Alt+H", Action::OpenHistory)];
        let chosen = vec![
            (Action::CaptureRegion, "Ctrl+Shift+S".to_string()),
            (Action::CaptureActiveWindow, "shift + prtsc".to_string()),
            (Action::CaptureFullscreen, String::new()),
            (Action::RecordGif, "Ctrl+Alt+G".to_string()),
        ];
        let out = apply_hotkeys(&old, &chosen).unwrap();
        let find = |a: Action| out.iter().find(|w| w.action == a.name());
        assert_eq!(find(Action::CaptureRegion).unwrap().hotkey, "Ctrl+Shift+S");
        // Same chord: original text and task chain kept.
        let aw = find(Action::CaptureActiveWindow).unwrap();
        assert_eq!(aw.hotkey, "Shift+PrintScreen");
        assert!(aw.tasks.unwrap().pin);
        assert_eq!(find(Action::OpenHistory).unwrap().hotkey, "Ctrl+Alt+H");
        assert_eq!(find(Action::RecordGif).unwrap().hotkey, "Ctrl+Alt+G");
        assert!(find(Action::CaptureFullscreen).is_none());

        // Clearing removes the workflow.
        let out = apply_hotkeys(&old, &[(Action::CaptureRegion, String::new())]).unwrap();
        assert!(!out.iter().any(|w| w.action == "capture_region"));
    }

    #[test]
    fn setup_reappears_only_for_newer_steps() {
        let mut s = Settings::default();
        assert!(is_due(true, &s), "first run");
        assert!(is_due(false, &s), "settings from before setup existed");
        s.setup_version = SETUP_VERSION;
        assert!(!is_due(false, &s), "bug-fix update: setup already seen");
        s.setup_version = SETUP_VERSION + 1; // went back to an older build
        assert!(!is_due(false, &s));
        // "(new)" only for items added after the setup the user last saw.
        assert_eq!(tagged("Ruler", 3, 2), "Ruler  (new)");
        assert_eq!(tagged("Ruler", 2, 2), "Ruler");
        assert_eq!(tagged("Ruler", 3, 0), "Ruler");
    }

    #[test]
    fn hotkey_mistakes_are_reported() {
        let old = vec![wf("Ctrl+Alt+H", Action::OpenHistory)];
        assert!(apply_hotkeys(&old, &[(Action::CaptureRegion, "Ctrl+Nope".into())]).unwrap_err().contains("Ctrl+Nope"));
        // Clashes with a hotkey not shown in the wizard are caught too.
        let e = apply_hotkeys(&old, &[(Action::CaptureRegion, "Ctrl+Alt+H".into())]).unwrap_err();
        assert!(e.contains("History") && e.contains("Region"), "{e}");
        let e = apply_hotkeys(&[], &[(Action::CaptureRegion, "F9".into()), (Action::RecordMp4, "F9".into())]).unwrap_err();
        assert!(e.contains("F9"));
    }
}
