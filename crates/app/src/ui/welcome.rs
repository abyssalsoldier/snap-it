//! First-launch setup: a Modern Studio 5-step wizard shown the first time Snap It starts
//! (no settings file yet), so new users pick their hotkeys and decide on opt-in features.
//!
//! Steps: Welcome (key features) > Hotkeys > Features (opt-in) > After a capture > Done.
//!
//! Re-showing after updates: bump [`SETUP_VERSION`] in a release that adds something to these
//! steps (a new opt-in feature, a new hotkey), and tag the new item with that number in its
//! `since` field.

use std::cell::Cell;

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, Pixmap, PixmapMut, PixmapPaint, Transform};
use snapit_gfx::{text, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, LoadCursorW, SetCursor, SetForegroundWindow, ShowWindow, HTCLIENT, IDC_ARROW,
    IDC_HAND, SW_SHOW, WM_COMMAND, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_SIZE,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU, WINDOW_EX_STYLE,
};

use super::chrome::Chrome;
use super::icons::{self, Icon};
use super::theme::{self, Theme};
use super::widgets::{self, BtnStyle};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::actions::Action;
use crate::app::{self, Command};
use crate::hotkeys::Chord;
use crate::settings::{Settings, Workflow};

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

/// The version of these setup steps. Bump it only when a release adds something to them.
pub const SETUP_VERSION: u32 = 2;

/// Setup version that added the "Use Print Screen for Snap It" option.
const PRTSC_SINCE: u32 = 2;

/// Preserved for `--selftest` (`WM_COMMAND` with `ID_NEXT`).
pub const ID_NEXT: u16 = 102;
/// `--selftest` only: show the Hotkeys step with the Print Screen offer visible.
pub const ID_TEST_PRTSC: u16 = 103;

const STEPS: [&str; 5] = [
    "Welcome to Snap It",
    "Choose your hotkeys",
    "Turn on extra features",
    "After a capture",
    "You're all set",
];

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

/// Temporary ids for testing whether chords are free (never left registered).
const PROBE_BASE: u16 = 0xB000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Next,
    Back,
    Skip,
    CheckKeys,
    KeyboardSettings,
    Hotkey(usize),
    Toggle(u16),
}

const TOG_SCROLLING: u16 = 1;
const TOG_UPDATE: u16 = 2;
const TOG_STARTUP: u16 = 3;
const TOG_CTXMENU: u16 = 4;
const TOG_QUICK: u16 = 5;
const TOG_PRTSC: u16 = 6;
const TOG_T_COPY: u16 = 10;
const TOG_T_SAVE: u16 = 11;
const TOG_T_EDITOR: u16 = 12;
const TOG_T_NOTIFY: u16 = 13;
const TOG_T_SOUND: u16 = 14;
const TOG_TRY_NOW: u16 = 20;

struct WelcomeWin {
    orig: Settings,
    theme: Theme,
    scale: f32,
    chrome: Chrome,
    bb: Option<Backbuffer>,
    client: (i32, i32),
    page: usize,
    finished: bool,
    returning: bool,
    focused_hk: Option<usize>,
    hover: Option<Hit>,
    hits: Vec<(Rect, Hit)>,
    brand_cache: Option<(u32, Pixmap)>,

    hotkeys: Vec<String>,
    key_status: Option<(bool, String)>, // (is_ok, message)

    scrolling_capture: bool,
    update_check: bool,
    start_with_windows: bool,
    context_menu: bool,
    quick_annotate: bool,

    t_copy: bool,
    t_save: bool,
    t_editor: bool,
    t_notify: bool,
    t_sound: bool,
    try_now: bool,

    /// Windows currently gives Print Screen to its own screen capture, so offer to switch it.
    prtsc_offer: bool,
    /// The user opted in to turning that Windows setting off (applied on Finish).
    prtsc_take: bool,
}

/// True when Snap It has never saved settings for this user (or portable folder).
pub fn is_first_run() -> bool {
    !crate::paths::settings_file().exists()
}

/// Whether setup should open by itself: on the first run, or once after an update that
/// added setup steps this user hasn't seen.
pub fn is_due(first_run: bool, s: &Settings) -> bool {
    is_due_with(first_run, s, crate::integration::print_screen_opens_snipping() == Some(true))
}

/// [`is_due`] with the Windows Print Screen setting passed in (`prtsc_useful`: it is on).
fn is_due_with(first_run: bool, s: &Settings, prtsc_useful: bool) -> bool {
    if first_run {
        return true;
    }
    if s.setup_version >= SETUP_VERSION {
        return false;
    }
    // When the Print Screen option is the only thing new since this user's last setup,
    // only re-show setup where that option would actually do something.
    let only_prtsc_is_new = s.setup_version + 1 == PRTSC_SINCE && PRTSC_SINCE == SETUP_VERSION;
    !only_prtsc_is_new || prtsc_useful
}

/// Plain Print Screen, the key Windows' screen-capture setting takes.
fn print_screen() -> Option<Chord> {
    Chord::parse("PrintScreen")
}

/// Whether to offer turning off Windows' Print Screen → screen capture setting.
fn print_screen_offer() -> bool {
    match crate::integration::print_screen_opens_snipping() {
        Some(on) => on,
        // Never changed: the default depends on the Windows build, so test the key itself.
        None => !super::headless() && print_screen().is_some_and(|p| !unavailable(&[p]).is_empty()),
    }
}

/// Label text, marked "(new)" if the item was added after the setup this user last saw.
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
    let s = (*app::settings()).clone();
    let hotkeys = HOTKEY_ACTIONS.iter().map(|(a, _)| s.hotkey_for(*a).unwrap_or_default()).collect();
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h) = ((700.0 * sc) as i32, (600.0 * sc) as i32);
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));

    let ww = WelcomeWin {
        scrolling_capture: s.scrolling_capture,
        update_check: s.integration.update_check,
        start_with_windows: s.integration.start_with_windows,
        context_menu: s.integration.context_menu,
        quick_annotate: s.region.quick_annotate,
        t_copy: s.tasks.copy,
        t_save: s.tasks.save,
        t_editor: s.tasks.open_editor,
        t_notify: s.tasks.notify,
        t_sound: s.tasks.sound,
        try_now: false,
        prtsc_offer: print_screen_offer(),
        prtsc_take: false,
        orig: s,
        theme: theme::current(),
        scale: sc,
        chrome: Chrome::new(false, true, false),
        bb: None,
        client: (w, h),
        page: 0,
        finished: false,
        returning: !is_first_run(),
        focused_hk: None,
        hover: None,
        hits: Vec::new(),
        brand_cache: None,
        hotkeys,
        key_status: None,
    };

    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    match super::create("SnapItWelcome", WINDOW_EX_STYLE(0), style, "Snap It Setup", r, None, Box::new(ww)) {
        Ok(hwnd) => {
            Chrome::init(hwnd);
            OPEN.with(|o| o.set(hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("welcome: {e}")),
    }
}

/// Applies the hotkeys chosen here to `workflows`.
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
    fn set_hk_focus(&mut self, idx: Option<usize>) {
        let was = self.focused_hk.is_some();
        let now = idx.is_some();
        self.focused_hk = idx;
        if now && !was {
            widgets::pause_hotkeys();
        } else if !now && was {
            widgets::resume_hotkeys();
        }
    }

    fn chosen_hotkeys(&self) -> Vec<(Action, String)> {
        HOTKEY_ACTIONS.iter().enumerate().map(|(i, (a, _))| (*a, self.hotkeys[i].trim().to_string())).collect()
    }

    fn check_keys(&mut self) -> Vec<Chord> {
        let chords: Vec<Chord> = self.chosen_hotkeys().iter().filter_map(|(_, t)| Chord::parse(t)).collect();
        let mut taken = unavailable(&chords);
        // Print Screen is about to be handed over by Windows, so it doesn't count as taken.
        let before = taken.len();
        if self.prtsc_offer && self.prtsc_take {
            taken.retain(|c| Some(*c) != print_screen());
        }
        let freed = taken.len() < before;
        self.key_status = Some(if chords.is_empty() {
            (true, "No hotkeys set. You can still use the tray icon.".into())
        } else if taken.is_empty() && freed {
            (true, "Print Screen will switch to Snap It on Finish; the rest are free.".into())
        } else if taken.is_empty() {
            (true, "All of these hotkeys are free.".into())
        } else {
            let names: Vec<String> = taken.iter().map(Chord::display).collect();
            (false, format!("In use by another app: {}.", names.join(", ")))
        });
        taken
    }

    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.chosen_hotkeys().into_iter().find(|(a, t)| *a == Action::CaptureRegion && !t.is_empty()) {
            Some((_, t)) => lines.push(format!("Press {t} to capture a region.")),
            None => lines.push("Capture a region by double-clicking the Snap It tray icon.".into()),
        }
        lines.push("While selecting: drag to choose an area, click to capture a window, press Enter to finish or Esc to cancel.".into());
        let mut on = Vec::new();
        for (v, name) in [
            (self.scrolling_capture, "scrolling capture"),
            (self.update_check, "update checks"),
            (self.start_with_windows, "start at sign-in"),
            (self.context_menu, "Explorer menu"),
            (self.prtsc_offer && self.prtsc_take, "Print Screen for Snap It"),
        ] {
            if v {
                on.push(name);
            }
        }
        lines.push(if on.is_empty() { "Extra features: none turned on.".into() } else { format!("Turned on: {}.", on.join(", ")) });
        lines.push("Change anything later by right-clicking the Snap It tray icon and choosing Settings.".into());
        lines
    }

    fn collect(&self) -> Result<Settings, String> {
        let mut s = self.orig.clone();
        s.workflows = apply_hotkeys(&s.workflows, &self.chosen_hotkeys())?;
        s.scrolling_capture = self.scrolling_capture;
        s.integration.update_check = self.update_check;
        s.integration.start_with_windows = self.start_with_windows;
        s.integration.context_menu = self.context_menu;
        s.region.quick_annotate = self.quick_annotate;
        s.tasks.copy = self.t_copy;
        s.tasks.save = self.t_save;
        s.tasks.open_editor = self.t_editor;
        s.tasks.notify = self.t_notify;
        s.tasks.sound = self.t_sound;
        s.setup_version = s.setup_version.max(SETUP_VERSION);
        Ok(s)
    }

    fn next(&mut self, w: &mut Win) {
        self.set_hk_focus(None);
        if self.page == 1 {
            if let Err(e) = apply_hotkeys(&self.orig.workflows, &self.chosen_hotkeys()) {
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
            self.page += 1;
            w.invalidate();
            return;
        }
        self.finish(w);
    }

    fn finish(&mut self, w: &mut Win) {
        let next = match self.collect() {
            Ok(n) => n,
            Err(e) => return super::dialogs::info(Some(w.hwnd), "Snap It", &e),
        };
        let prev = app::settings();
        let mut errors = crate::integration::apply(&prev.integration, &next.integration);
        if let Err(e) = next.save() {
            return super::dialogs::info(Some(w.hwnd), "Snap It", &format!("Could not save settings.\n{e}"));
        }
        let check_updates = next.integration.update_check && !prev.integration.update_check;
        app::with(|st| st.settings = std::rc::Rc::new(next));
        if self.prtsc_offer && self.prtsc_take && !super::headless() {
            // Opt-in only: turn off Windows' Print Screen → screen capture for this user.
            // Verified on Windows 11 23H2 (22631): Windows releases the key immediately, no
            // sign-out. The re-probe below is a fallback for builds that might not.
            match crate::integration::set_print_screen_opens_snipping(false) {
                Err(e) => errors.push(format!("Could not change the Windows Print Screen setting: {e}")),
                Ok(()) => {
                    if print_screen().is_some_and(|p| !unavailable(&[p]).is_empty()) {
                        errors.push(
                            "Windows is still holding the Print Screen key. Sign out and back in to finish the switch.".into(),
                        );
                    }
                }
            }
        }
        app::post(Command::ReloadSettings);
        if check_updates {
            crate::integration::check_for_updates(false);
        }
        if !errors.is_empty() {
            super::dialogs::info(Some(w.hwnd), "Snap It", &errors.join("\n"));
        }
        if self.try_now {
            app::post(Command::Run(Action::CaptureRegion));
        } else {
            app::post(Command::Notify {
                title: "Snap It is ready".into(),
                body: "It's in the notification area. Right-click the icon for every action.".into(),
            });
        }
        self.finished = true;
        w.close();
    }

    // ---------- Rendering ----------

    fn render(&mut self, hwnd: HWND) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        self.client = (rc.right.max(1), rc.bottom.max(1));
        self.scale = super::dpi_scale(hwnd);
        Backbuffer::ensure(&mut self.bb, self.client.0 as u32, self.client.1 as u32);
        let Some(mut bb) = self.bb.take() else { return };
        self.hits.clear();
        {
            let mut pm = bb.pixmap();
            let t = self.theme;
            let s = self.scale;
            let id = Transform::identity();
            pm.fill(t.bg.color());

            let step_badge = format!("Step {} of {}", self.page + 1, STEPS.len());
            self.chrome.draw(&mut pm, &t, hwnd, self.client.0, s, "Snap It Setup", Some(&step_badge), None);

            let pad = 28.0 * s;
            let cw = self.client.0 as f32 - pad * 2.0;
            let mut y = self.chrome.height(s) as f32 + 16.0 * s;

            // Segmented step progress bar.
            let seg_gap = 6.0 * s;
            let seg_w = (cw - seg_gap * (STEPS.len() - 1) as f32) / STEPS.len() as f32;
            let seg_h = 4.5 * s;
            for i in 0..STEPS.len() {
                let sx = pad + i as f32 * (seg_w + seg_gap);
                if let Some(p) = rounded_rect(sx, y, seg_w, seg_h, seg_h / 2.0) {
                    if i <= self.page {
                        pm.fill_path(&p, &t.accent_paint(sx, y, sx + seg_w, y), FillRule::Winding, id, None);
                    } else {
                        pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
                    }
                }
            }
            y += 18.0 * s;

            // Step headline.
            text::draw(&mut pm, STEPS[self.page], pad, y, 22.0 * s, true, t.text, None);
            y += 34.0 * s;

            match self.page {
                0 => self.draw_step_welcome(&mut pm, pad, cw, y),
                1 => self.draw_step_hotkeys(&mut pm, pad, cw, y),
                2 => self.draw_step_features(&mut pm, pad, cw, y),
                3 => self.draw_step_after(&mut pm, pad, cw, y),
                4 => self.draw_step_done(&mut pm, pad, cw, y),
                _ => {}
            }

            // Sticky footer bar.
            let fh = (60.0 * s) as i32;
            let fy = self.client.1 - fh;
            if let Some(fr) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, fy as f32, self.client.0 as f32, fh as f32) {
                pm.fill_rect(fr, &t.sidebar.paint(), id, None);
            }
            widgets::draw_divider(&mut pm, &t, 0.0, self.client.0 as f32, fy as f32);

            let bh = (36.0 * s) as i32;
            let by = fy + (fh - bh) / 2;
            let skip_r = Rect::new(pad as i32, by, (108.0 * s) as i32, bh);
            let next_w = (112.0 * s) as i32;
            let back_w = (96.0 * s) as i32;
            let next_r = Rect::new(self.client.0 - pad as i32 - next_w, by, next_w, bh);
            let back_r = Rect::new(next_r.x - (10.0 * s) as i32 - back_w, by, back_w, bh);

            widgets::draw_button(&mut pm, &t, skip_r, "Skip setup", None, BtnStyle::Ghost, self.hover == Some(Hit::Skip), false, s);
            widgets::draw_button(&mut pm, &t, back_r, "Back", Some(Icon::ChevronLeft), BtnStyle::Secondary, self.hover == Some(Hit::Back), self.page == 0, s);
            let last = self.page + 1 == STEPS.len();
            widgets::draw_button(
                &mut pm,
                &t,
                next_r,
                if last { "Finish" } else { "Next" },
                if last { Some(Icon::Done) } else { Some(Icon::ChevronRight) },
                BtnStyle::Primary,
                self.hover == Some(Hit::Next),
                false,
                s,
            );
            self.hits.push((skip_r, Hit::Skip));
            if self.page > 0 {
                self.hits.push((back_r, Hit::Back));
            }
            self.hits.push((next_r, Hit::Next));
        }
        self.bb = Some(bb);
    }

    fn draw_step_welcome(&mut self, pm: &mut PixmapMut, pad: f32, cw: f32, mut y: f32) {
        let t = self.theme;
        let s = self.scale;
        let seen = self.orig.setup_version;

        // Hero card with brand icon.
        let hero_h = 84.0 * s;
        widgets::draw_card(pm, &t, Rect::new(pad as i32, y as i32, cw as i32, hero_h as i32), 12.0, s);
        let isz = (48.0 * s).round() as u32;
        if self.brand_cache.as_ref().is_none_or(|(sz, _)| *sz != isz) {
            self.brand_cache = Some((isz, snapit_gfx::brand::icon(isz)));
        }
        if let Some((_, ref ico)) = self.brand_cache {
            pm.draw_pixmap((pad + 18.0 * s) as i32, (y + (hero_h - isz as f32) / 2.0) as i32, ico.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        let intro = if !self.returning {
            "Fast screenshots, annotation, and screen recording that stay 100% on this PC. This quick setup takes about a minute."
        } else if seen > 0 && seen < SETUP_VERSION {
            "Snap It has new features marked (new). Your existing settings stay intact unless you change them."
        } else {
            "A quick tour of Snap It's main settings. Your current settings are filled in and stay unchanged unless edited."
        };
        widgets::draw_wrapped(pm, intro, pad + 82.0 * s, y + 18.0 * s, cw - 98.0 * s, 13.0 * s, false, t.text);
        y += hero_h + 16.0 * s;

        // 2x3 feature highlight grid.
        let features = [
            (Icon::Camera, "Smart Snapping", "Region, window & control detection"),
            (Icon::Editor, "Quick Annotate", "Arrows, text, blur, highlight & steps"),
            (Icon::Video, "Screen Recording", "Capture crisp MP4 & GIF clips"),
            (Icon::ZoomFit, "Scrolling Capture", "Stitch long web pages & docs"),
            (Icon::Effects, "Built-in Tools", "OCR, QR codes, colour picker & ruler"),
            (Icon::Pin, "Pin & History", "Float captures on top or browse past ones"),
        ];
        let gap = 12.0 * s;
        let card_w = (cw - gap) / 2.0;
        let card_h = 68.0 * s;
        for (i, (ic, title, desc)) in features.iter().enumerate() {
            let col = i % 2;
            let row = i / 2;
            let cx = pad + col as f32 * (card_w + gap);
            let cy = y + row as f32 * (card_h + gap);
            widgets::draw_card(pm, &t, Rect::new(cx as i32, cy as i32, card_w as i32, card_h as i32), 10.0, s);
            let ib = 34.0 * s;
            if let Some(p) = rounded_rect(cx + 12.0 * s, cy + (card_h - ib) / 2.0, ib, ib, 8.0 * s) {
                pm.fill_path(&p, &t.accent_soft.paint(), FillRule::Winding, Transform::identity(), None);
            }
            icons::draw(pm, *ic, cx + 19.0 * s, cy + (card_h - 20.0 * s) / 2.0, 20.0 * s, t.accent);
            text::draw(pm, title, cx + 56.0 * s, cy + 14.0 * s, 13.0 * s, true, t.text, None);
            text::draw(pm, desc, cx + 56.0 * s, cy + 34.0 * s, 11.5 * s, false, t.subtext, None);
        }
        y += 3.0 * (card_h + gap) + 6.0 * s;

        // Privacy / tray footer note.
        let note = "Runs quietly in the system tray next to the clock · Nothing is uploaded · No account needed";
        let nl = text::layout(note, 11.5 * s, false);
        text::draw(pm, note, pad + (cw - nl.width) / 2.0, y, 11.5 * s, false, t.subtext, None);
    }

    fn draw_step_hotkeys(&mut self, pm: &mut PixmapMut, pad: f32, cw: f32, mut y: f32) {
        let t = self.theme;
        let s = self.scale;
        let seen = self.orig.setup_version;

        text::draw(pm, "Click a box and press your keys. Backspace clears it. More actions are in Settings > Hotkeys.", pad, y, 12.5 * s, false, t.subtext, None);
        y += 24.0 * s;

        let row_h = 34.0 * s;
        let card_h = HOTKEY_ACTIONS.len() as f32 * row_h + 12.0 * s;
        widgets::draw_card(pm, &t, Rect::new(pad as i32, y as i32, cw as i32, card_h as i32), 10.0, s);
        let ix = pad + 14.0 * s;
        let iw = cw - 28.0 * s;
        let mut cy = y + 7.0 * s;
        for (i, (a, since)) in HOTKEY_ACTIONS.iter().enumerate() {
            if i > 0 {
                widgets::draw_divider(pm, &t, ix, ix + iw, cy - 1.0 * s);
            }
            let lbl = tagged(a.label().trim_end_matches('…'), *since, seen);
            text::draw(pm, &lbl, ix, cy + 7.0 * s, 12.5 * s, false, t.text, None);
            let fw = (220.0 * s) as i32;
            let fr = Rect::new((ix + iw - fw as f32) as i32, (cy + 2.0 * s) as i32, fw, (28.0 * s) as i32);
            widgets::draw_hotkey_field(pm, &t, fr, &self.hotkeys[i], self.focused_hk == Some(i), self.hover == Some(Hit::Hotkey(i)), s);
            self.hits.push((fr, Hit::Hotkey(i)));
            cy += row_h;
        }
        y += card_h + 12.0 * s;

        let chk_r = Rect::new(pad as i32, y as i32, (132.0 * s) as i32, (32.0 * s) as i32);
        widgets::draw_button(pm, &t, chk_r, "Check hotkeys", None, BtnStyle::Secondary, self.hover == Some(Hit::CheckKeys), false, s);
        self.hits.push((chk_r, Hit::CheckKeys));

        let kbd_w = (224.0 * s) as i32;
        let kbd_r = Rect::new((pad + cw) as i32 - kbd_w, y as i32, kbd_w, (32.0 * s) as i32);
        widgets::draw_button(pm, &t, kbd_r, "Windows keyboard settings", None, BtnStyle::Ghost, self.hover == Some(Hit::KeyboardSettings), false, s);
        self.hits.push((kbd_r, Hit::KeyboardSettings));

        if let Some((ok, ref msg)) = self.key_status {
            let c = if ok { t.success } else { t.danger };
            let clipped = widgets::ellipsize(msg, 12.0 * s, false, (kbd_r.x - chk_r.right()) as f32 - 16.0 * s);
            text::draw(pm, &clipped, chk_r.right() as f32 + 12.0 * s, y + 8.0 * s, 12.0 * s, false, c, None);
        }
        y += 42.0 * s;
        if !self.prtsc_offer {
            widgets::draw_wrapped(
                pm,
                "Tip: Windows 11 may reserve Print Screen for Snipping Tool. If taken, disable \"Use the Print Screen key to open screen capture\" in Windows settings.",
                pad,
                y,
                cw,
                11.0 * s,
                false,
                t.subtext,
            );
            return;
        }

        // Windows currently gives Print Screen to its own screen capture: offer to switch it.
        let ch = 56.0 * s;
        let cr = Rect::new(pad as i32, y as i32, cw as i32, ch as i32);
        widgets::draw_card(pm, &t, cr, 10.0, s);
        let title = "Use Print Screen for Snap It";
        text::draw(pm, title, pad + 16.0 * s, y + 10.0 * s, 13.5 * s, true, t.text, None);
        if seen > 0 && PRTSC_SINCE > seen {
            let tw = text::layout(title, 13.5 * s, true).width;
            widgets::draw_badge(pm, &t, pad + 24.0 * s + tw, y + 9.0 * s, "NEW", true, s);
        }
        let help = "Turns off Windows' Print Screen shortcut to Snipping Tool. Win+Shift+S still opens it.";
        let help = widgets::ellipsize(help, 11.5 * s, false, cw - 90.0 * s);
        text::draw(pm, &help, pad + 16.0 * s, y + 32.0 * s, 11.5 * s, false, t.subtext, None);
        let tr = Rect::new((pad + cw - 56.0 * s) as i32, (y + (ch - 24.0 * s) / 2.0) as i32, (42.0 * s) as i32, (24.0 * s) as i32);
        widgets::draw_toggle(pm, &t, tr, self.prtsc_take, self.hover == Some(Hit::Toggle(TOG_PRTSC)), s);
        self.hits.push((cr, Hit::Toggle(TOG_PRTSC)));
    }

    fn draw_step_features(&mut self, pm: &mut PixmapMut, pad: f32, cw: f32, mut y: f32) {
        let t = self.theme;
        let s = self.scale;
        let seen = self.orig.setup_version;

        text::draw(pm, "These optional features are off by default. Turn on the ones you want.", pad, y, 12.5 * s, false, t.subtext, None);
        y += 24.0 * s;

        let items = [
            (TOG_SCROLLING, "Scrolling capture", self.scrolling_capture, "Captures long pages by sending mouse-wheel input to the window you select.", 1),
            (TOG_UPDATE, "Check for updates at startup", self.update_check, "Checks GitHub for newer releases. Never downloads without asking.", 1),
            (TOG_STARTUP, "Start Snap It when I sign in", self.start_with_windows, "Adds a Startup shortcut so your hotkeys work right after sign-in.", 1),
            (TOG_CTXMENU, "Add Snap It to the Explorer menu", self.context_menu, "Right-click images for \"Edit with Snap It\" and \"Pin with Snap It\".", 1),
            (TOG_QUICK, "Show quick-annotate after selecting", self.quick_annotate, "Draw, blur, or add text before copying or saving a region.", 1),
        ];
        for (id, title, on, help, since) in items {
            let ch = 66.0 * s;
            let cr = Rect::new(pad as i32, y as i32, cw as i32, ch as i32);
            widgets::draw_card(pm, &t, cr, 10.0, s);
            let is_new = seen > 0 && since > seen;
            text::draw(pm, title, pad + 16.0 * s, y + 12.0 * s, 13.5 * s, true, t.text, None);
            if is_new {
                let tw = text::layout(title, 13.5 * s, true).width;
                widgets::draw_badge(pm, &t, pad + 24.0 * s + tw, y + 11.0 * s, "NEW", true, s);
            }
            text::draw(pm, help, pad + 16.0 * s, y + 36.0 * s, 11.5 * s, false, t.subtext, None);
            let tr = Rect::new((pad + cw - 56.0 * s) as i32, (y + (ch - 24.0 * s) / 2.0) as i32, (42.0 * s) as i32, (24.0 * s) as i32);
            widgets::draw_toggle(pm, &t, tr, on, self.hover == Some(Hit::Toggle(id)), s);
            self.hits.push((cr, Hit::Toggle(id)));
            y += ch + 10.0 * s;
        }
    }

    fn draw_step_after(&mut self, pm: &mut PixmapMut, pad: f32, cw: f32, mut y: f32) {
        let t = self.theme;
        let s = self.scale;
        let seen = self.orig.setup_version;

        text::draw(pm, "Choose what happens automatically when you finish a capture:", pad, y, 12.5 * s, false, t.subtext, None);
        y += 26.0 * s;

        let card_h = 200.0 * s;
        widgets::draw_card(pm, &t, Rect::new(pad as i32, y as i32, cw as i32, card_h as i32), 10.0, s);
        let ix = pad + 16.0 * s;
        let iw = cw - 32.0 * s;
        let mut cy = y + 14.0 * s;
        for (idx, (id, title, on, since)) in [
            (TOG_T_COPY, "Copy the image to the clipboard", self.t_copy, 1),
            (TOG_T_SAVE, "Save it to the screenshots folder", self.t_save, 1),
            (TOG_T_EDITOR, "Open it in the editor", self.t_editor, 1),
            (TOG_T_NOTIFY, "Show a notification (click to open)", self.t_notify, 1),
            (TOG_T_SOUND, "Play the capture sound", self.t_sound, 1),
        ]
        .into_iter()
        .enumerate()
        {
            if idx > 0 {
                widgets::draw_divider(pm, &t, ix, ix + iw, cy - 5.0 * s);
            }
            let lbl = tagged(title, since, seen);
            text::draw(pm, &lbl, ix, cy + 2.0 * s, 13.0 * s, false, t.text, None);
            let tr = Rect::new((ix + iw - 42.0 * s) as i32, cy as i32, (42.0 * s) as i32, (24.0 * s) as i32);
            widgets::draw_toggle(pm, &t, tr, on, self.hover == Some(Hit::Toggle(id)), s);
            self.hits.push((Rect::new(ix as i32, (cy - 4.0 * s) as i32, iw as i32, (34.0 * s) as i32), Hit::Toggle(id)));
            cy += 36.0 * s;
        }
        y += card_h + 16.0 * s;

        let save_card_h = 68.0 * s;
        widgets::draw_card(pm, &t, Rect::new(pad as i32, y as i32, cw as i32, save_card_h as i32), 10.0, s);
        text::draw(pm, &format!("Save folder: {}", self.orig.save_dir().display()), pad + 16.0 * s, y + 14.0 * s, 12.0 * s, true, t.text, None);
        text::draw(pm, "The quick-annotate toolbar also has Copy, Save, Pin and Edit buttons for one-off choices.", pad + 16.0 * s, y + 38.0 * s, 11.5 * s, false, t.subtext, None);
    }

    fn draw_step_done(&mut self, pm: &mut PixmapMut, pad: f32, cw: f32, mut y: f32) {
        let t = self.theme;
        let s = self.scale;

        let card_h = 240.0 * s;
        widgets::draw_card(pm, &t, Rect::new(pad as i32, y as i32, cw as i32, card_h as i32), 12.0, s);
        let mut cy = y + 20.0 * s;
        for line in self.summary_lines() {
            let lh = widgets::draw_wrapped(pm, &line, pad + 20.0 * s, cy, cw - 40.0 * s, 13.0 * s, false, t.text);
            cy += lh + 14.0 * s;
        }
        y += card_h + 18.0 * s;

        let tr_card = Rect::new(pad as i32, y as i32, cw as i32, (56.0 * s) as i32);
        widgets::draw_card(pm, &t, tr_card, 10.0, s);
        text::draw(pm, "Take a screenshot when I click Finish", pad + 16.0 * s, y + 19.0 * s, 13.5 * s, true, t.text, None);
        let tr = Rect::new((pad + cw - 58.0 * s) as i32, (y + 16.0 * s) as i32, (42.0 * s) as i32, (24.0 * s) as i32);
        widgets::draw_toggle(pm, &t, tr, self.try_now, self.hover == Some(Hit::Toggle(TOG_TRY_NOW)), s);
        self.hits.push((tr_card, Hit::Toggle(TOG_TRY_NOW)));
    }
}

impl Handler for WelcomeWin {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == super::WM_SNAPIT_THEME {
            self.theme = theme::current();
            w.invalidate();
            return Some(LRESULT(0));
        }
        if let Some((r, dirty)) = self.chrome.nc_message(w.hwnd, msg, wp) {
            if dirty {
                w.invalidate();
            }
            return Some(r);
        }
        match msg {
            WM_NCCALCSIZE => Chrome::handle_nccalcsize(w.hwnd, wp, lp),
            WM_NCHITTEST => Some(self.chrome.hit_test(w.hwnd, lp, self.scale, &[])),
            WM_KILLFOCUS => {
                // Never leave the global hotkeys paused while another window has the keyboard.
                if self.focused_hk.is_some() {
                    self.set_hk_focus(None);
                    w.invalidate();
                }
                None
            }
            WM_MOUSELEAVE => {
                self.chrome.mouse_leave();
                self.hover = None;
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_SIZE => {
                self.scale = w.dpi_scale();
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_PAINT => {
                self.render(w.hwnd);
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                let cur = if self.hover.is_some() || self.chrome.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                unsafe {
                    if let Ok(c) = LoadCursorW(None, cur) {
                        SetCursor(Some(c));
                    }
                }
                Some(LRESULT(1))
            }
            WM_MOUSEMOVE => {
                let (x, y) = super::lparam_xy(lp);
                let mut dirty = self.chrome.set_hover(w.hwnd, self.client.0, self.scale, x, y);
                let h = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
                if h != self.hover {
                    self.hover = h;
                    dirty = true;
                }
                if dirty {
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (x, y) = super::lparam_xy(lp);
                self.chrome.press(self.client.0, self.scale, x, y);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = super::lparam_xy(lp);
                if self.chrome.release(w.hwnd, self.client.0, self.scale, x, y) {
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                let hit = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
                match hit {
                    Some(Hit::Next) => self.next(w),
                    Some(Hit::Back) if self.page > 0 => {
                        self.set_hk_focus(None);
                        self.page -= 1;
                    }
                    Some(Hit::Skip) => w.close(),
                    Some(Hit::CheckKeys) => {
                        self.set_hk_focus(None);
                        if let Err(e) = apply_hotkeys(&self.orig.workflows, &self.chosen_hotkeys()) {
                            self.key_status = Some((false, e));
                        } else {
                            self.check_keys();
                        }
                    }
                    Some(Hit::KeyboardSettings) => crate::shell::open_keyboard_settings(),
                    Some(Hit::Hotkey(i)) => self.set_hk_focus(Some(i)),
                    Some(Hit::Toggle(id)) => {
                        self.set_hk_focus(None);
                        match id {
                            TOG_SCROLLING => self.scrolling_capture = !self.scrolling_capture,
                            TOG_UPDATE => self.update_check = !self.update_check,
                            TOG_STARTUP => self.start_with_windows = !self.start_with_windows,
                            TOG_CTXMENU => self.context_menu = !self.context_menu,
                            TOG_QUICK => self.quick_annotate = !self.quick_annotate,
                            TOG_PRTSC => {
                                self.prtsc_take = !self.prtsc_take;
                                // A previous "in use" result may no longer apply.
                                self.key_status = None;
                            }
                            TOG_T_COPY => self.t_copy = !self.t_copy,
                            TOG_T_SAVE => self.t_save = !self.t_save,
                            TOG_T_EDITOR => self.t_editor = !self.t_editor,
                            TOG_T_NOTIFY => self.t_notify = !self.t_notify,
                            TOG_T_SOUND => self.t_sound = !self.t_sound,
                            TOG_TRY_NOW => self.try_now = !self.try_now,
                            _ => {}
                        }
                    }
                    _ => self.set_hk_focus(None),
                }
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wp.0 as u32;
                if let Some(i) = self.focused_hk {
                    if vk == 0x1B {
                        self.set_hk_focus(None);
                        w.invalidate();
                        return Some(LRESULT(0));
                    }
                    if let Some(new_val) = widgets::handle_hotkey_msg(false, vk) {
                        self.hotkeys[i] = new_val;
                        w.invalidate();
                    }
                    return Some(LRESULT(0));
                }
                if msg == WM_SYSKEYDOWN {
                    // Let DefWindowProc handle Alt+F4, Alt+Space, F10, etc.
                    return None;
                }
                if vk == 0x1B {
                    w.close();
                    return Some(LRESULT(0));
                }
                Some(LRESULT(0))
            }
            WM_KEYUP | WM_SYSKEYUP => {
                let vk = wp.0 as u32;
                if let Some(i) = self.focused_hk {
                    if let Some(new_val) = widgets::handle_hotkey_msg(true, vk) {
                        self.hotkeys[i] = new_val;
                        w.invalidate();
                    }
                    return Some(LRESULT(0));
                }
                None
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as u16;
                if id == ID_NEXT {
                    self.next(w);
                    return Some(LRESULT(0));
                }
                if id == ID_TEST_PRTSC && super::headless() {
                    self.prtsc_offer = true;
                    self.prtsc_take = true;
                    self.page = 1;
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                None
            }
            WM_DESTROY => {
                widgets::resume_hotkeys();
                if !self.finished && (is_first_run() || app::settings().setup_version < SETUP_VERSION) {
                    app::update_settings(|s| s.setup_version = SETUP_VERSION);
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
        let aw = find(Action::CaptureActiveWindow).unwrap();
        assert_eq!(aw.hotkey, "Shift+PrintScreen");
        assert!(aw.tasks.unwrap().pin);
        assert_eq!(find(Action::OpenHistory).unwrap().hotkey, "Ctrl+Alt+H");
        assert_eq!(find(Action::RecordGif).unwrap().hotkey, "Ctrl+Alt+G");
        assert!(find(Action::CaptureFullscreen).is_none());

        let out = apply_hotkeys(&old, &[(Action::CaptureRegion, String::new())]).unwrap();
        assert!(!out.iter().any(|w| w.action == "capture_region"));
    }

    #[test]
    fn setup_reappears_only_for_newer_steps() {
        let mut s = Settings::default();
        assert!(is_due_with(true, &s, false), "first run");
        assert!(is_due_with(false, &s, false), "settings from before setup existed");
        s.setup_version = SETUP_VERSION;
        assert!(!is_due_with(false, &s, true), "bug-fix update: setup already seen");
        s.setup_version = SETUP_VERSION + 1;
        assert!(!is_due_with(false, &s, true));
        // Setup 2 only added the Print Screen option: re-show only where it helps.
        s.setup_version = PRTSC_SINCE - 1;
        assert!(is_due_with(false, &s, true), "Windows owns Print Screen");
        assert!(!is_due_with(false, &s, false), "nothing new for this user");
        assert_eq!(tagged("Ruler", 3, 2), "Ruler  (new)");
        assert_eq!(tagged("Ruler", 2, 2), "Ruler");
        assert_eq!(tagged("Ruler", 3, 0), "Ruler");
    }

    #[test]
    fn hotkey_mistakes_are_reported() {
        let old = vec![wf("Ctrl+Alt+H", Action::OpenHistory)];
        assert!(apply_hotkeys(&old, &[(Action::CaptureRegion, "Ctrl+Nope".into())]).unwrap_err().contains("Ctrl+Nope"));
        let e = apply_hotkeys(&old, &[(Action::CaptureRegion, "Ctrl+Alt+H".into())]).unwrap_err();
        assert!(e.contains("History") && e.contains("Region"), "{e}");
        let e = apply_hotkeys(&[], &[(Action::CaptureRegion, "F9".into()), (Action::RecordMp4, "F9".into())]).unwrap_err();
        assert!(e.contains("F9"));
    }
}
