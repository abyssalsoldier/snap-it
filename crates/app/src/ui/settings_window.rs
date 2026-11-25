//! The Modern Studio Settings window: custom-drawn sidebar navigation, rounded cards,
//! toggle switches, segmented selectors, text inputs, and keycap hotkey recorders.

use std::cell::Cell;
use std::path::PathBuf;

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, Pixmap, PixmapMut, Transform};
use snapit_gfx::{text, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, IsIconic, LoadCursorW, SetCursor, SetForegroundWindow, ShowWindow, HTCLIENT,
    IDC_ARROW, IDC_HAND, IDC_IBEAM, SW_RESTORE, SW_SHOW, WM_CHAR, WM_COMMAND, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_SIZE, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU, WINDOW_EX_STYLE,
};

use super::chrome::Chrome;
use super::icons::{self, Icon};
use super::theme::{self, Theme as UiTheme};
use super::widgets::{self, BtnStyle, TextInput};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::actions::Action;
use crate::app::{self, Command};
use crate::settings::{Settings, Theme as PrefTheme, Workflow};
use snapit_gfx::io::Format;

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

/// Preserved for `--selftest` round-trip verification (`WM_COMMAND` with `ID_SAVE`).
pub const ID_SAVE: u16 = 101;

const TABS: [(&str, Icon); 6] = [
    ("General", Icon::Settings),
    ("Capture", Icon::Camera),
    ("After capture", Icon::Bolt),
    ("Hotkeys", Icon::Keyboard),
    ("Recording", Icon::Video),
    ("Integration", Icon::Plug),
];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    SaveDir,
    Pattern,
    JpegQ,
    History,
    Delay,
    Dim,
    Mp4Fps,
    Mp4Kbps,
    GifFps,
    GifMax,
    Repo,
    Hotkey(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Nav(usize),
    Toggle(u16),
    Seg(u16, usize),
    Text(Field),
    Hotkey(usize),
    Btn(u16),
}

// Logical IDs for toggles, segmented controls, and action buttons.
const BTN_SAVE: u16 = 1;
const BTN_CANCEL: u16 = 2;
const BTN_BROWSE: u16 = 3;
const BTN_WELCOME: u16 = 4;
const BTN_CLEAR_CUSTOM: u16 = 5;
const BTN_CHECK_NOW: u16 = 6;
const BTN_OPEN_CONFIG: u16 = 7;

const SEG_FORMAT: u16 = 10;
const SEG_THEME: u16 = 11;

const TOG_CURSOR: u16 = 20;
const TOG_QUICK: u16 = 21;
const TOG_DETECT_WIN: u16 = 22;
const TOG_DETECT_CTL: u16 = 23;
const TOG_UIA: u16 = 24;
const TOG_MAGNIFIER: u16 = 25;
const TOG_CROSSHAIR: u16 = 26;
const TOG_SCROLLING: u16 = 27;
const TOG_T_COPY: u16 = 30;
const TOG_T_SAVE: u16 = 31;
const TOG_T_EDITOR: u16 = 32;
const TOG_T_PIN: u16 = 33;
const TOG_T_NOTIFY: u16 = 34;
const TOG_T_SOUND: u16 = 35;
const TOG_REC_CURSOR: u16 = 40;
const TOG_STARTUP: u16 = 50;
const TOG_CTXMENU: u16 = 51;
const TOG_UPDATE: u16 = 52;

struct SettingsWin {
    /// Original settings snapshot (kept for preserving hand-edited workflows and custom_region).
    orig: Settings,
    theme: UiTheme,
    scale: f32,
    chrome: Chrome,
    bb: Option<Backbuffer>,
    client: (i32, i32),
    tab: usize,
    scroll: f32,
    content_h: f32,
    focused: Option<Field>,
    hover: Option<Hit>,
    hits: Vec<(Rect, Hit)>,

    // Editable state mirrors every Settings field cleanly.
    save_dir: TextInput,
    pattern: TextInput,
    format: usize,
    jpeg_q: TextInput,
    theme_pref: usize,
    history_limit: TextInput,

    capture_cursor: bool,
    delay: TextInput,
    quick_annotate: bool,
    detect_windows: bool,
    detect_controls: bool,
    uia_controls: bool,
    magnifier: bool,
    crosshair: bool,
    dim: TextInput,
    scrolling_capture: bool,
    custom_region: Option<Rect>,

    t_copy: bool,
    t_save: bool,
    t_editor: bool,
    t_pin: bool,
    t_notify: bool,
    t_sound: bool,

    hotkeys: Vec<String>,

    mp4_fps: TextInput,
    mp4_kbps: TextInput,
    gif_fps: TextInput,
    gif_max: TextInput,
    rec_cursor: bool,

    start_with_windows: bool,
    context_menu: bool,
    update_check: bool,
    repo: TextInput,
}

pub fn open() {
    let existing = OPEN.with(|o| o.get());
    if existing != 0 {
        unsafe {
            let h = HWND(existing as *mut _);
            let minimised = IsIconic(h).as_bool();
            let _ = ShowWindow(h, if minimised { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(h);
        }
        return;
    }
    let s = (*app::settings()).clone();
    let dir = s.save_dir.clone().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let format = match s.format {
        Format::Png => 0,
        Format::Jpg => 1,
        Format::Webp => 2,
    };
    let theme_pref = match s.theme {
        PrefTheme::System => 0,
        PrefTheme::Light => 1,
        PrefTheme::Dark => 2,
    };
    let hotkeys = HOTKEY_ACTIONS.iter().map(|a| s.hotkey_for(*a).unwrap_or_default()).collect();

    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h) = ((820.0 * sc) as i32, (640.0 * sc) as i32);
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));

    let sw = SettingsWin {
        save_dir: TextInput::new(dir, "Default (Pictures\\Snap It)", false, false),
        pattern: TextInput::new(s.filename_pattern.clone(), "%y-%mo-%d_%h-%mi-%s", false, false),
        format,
        jpeg_q: TextInput::new(s.jpeg_quality.to_string(), "90", true, false),
        theme_pref,
        history_limit: TextInput::new(s.history_limit.to_string(), "500", true, false),
        capture_cursor: s.capture_cursor,
        delay: TextInput::new(s.capture_delay_ms.to_string(), "0", true, false),
        quick_annotate: s.region.quick_annotate,
        detect_windows: s.region.detect_windows,
        detect_controls: s.region.detect_controls,
        uia_controls: s.region.uia_controls,
        magnifier: s.region.magnifier,
        crosshair: s.region.crosshair,
        dim: TextInput::new(s.region.dim_opacity.to_string(), "110", true, false),
        scrolling_capture: s.scrolling_capture,
        custom_region: s.custom_region,
        t_copy: s.tasks.copy,
        t_save: s.tasks.save,
        t_editor: s.tasks.open_editor,
        t_pin: s.tasks.pin,
        t_notify: s.tasks.notify,
        t_sound: s.tasks.sound,
        hotkeys,
        mp4_fps: TextInput::new(s.recording.mp4_fps.to_string(), "30", true, false),
        mp4_kbps: TextInput::new(s.recording.mp4_bitrate_kbps.to_string(), "8000", true, false),
        gif_fps: TextInput::new(s.recording.gif_fps.to_string(), "15", true, false),
        gif_max: TextInput::new(s.recording.gif_max_seconds.to_string(), "60", true, false),
        rec_cursor: s.recording.cursor,
        start_with_windows: s.integration.start_with_windows,
        context_menu: s.integration.context_menu,
        update_check: s.integration.update_check,
        repo: TextInput::new(s.integration.update_repo.clone(), "owner/repo", false, false),
        orig: s,
        theme: theme::current(),
        scale: sc,
        chrome: Chrome::new(false, true, false),
        bb: None,
        client: (w, h),
        tab: 0,
        scroll: 0.0,
        content_h: 0.0,
        focused: None,
        hover: None,
        hits: Vec::new(),
    };

    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    match super::create("SnapItSettings", WINDOW_EX_STYLE(0), style, "Snap It Settings", r, None, Box::new(sw)) {
        Ok(hwnd) => {
            Chrome::init(hwnd);
            OPEN.with(|o| o.set(hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("settings: {e}")),
    }
}

impl SettingsWin {
    fn sidebar_w(&self) -> i32 {
        (196.0 * self.scale).round() as i32
    }
    fn footer_h(&self) -> i32 {
        (58.0 * self.scale).round() as i32
    }
    fn content_rect(&self) -> Rect {
        let sx = self.sidebar_w();
        let ty = self.chrome.height(self.scale);
        let fh = self.footer_h();
        Rect::new(sx, ty, (self.client.0 - sx).max(1), (self.client.1 - ty - fh).max(1))
    }
    fn max_scroll(&self) -> f32 {
        (self.content_h - self.content_rect().h as f32).max(0.0)
    }

    fn set_focus(&mut self, next: Option<Field>) {
        let was_hk = matches!(self.focused, Some(Field::Hotkey(_)));
        let now_hk = matches!(next, Some(Field::Hotkey(_)));
        self.focused = next;
        if now_hk && !was_hk {
            widgets::pause_hotkeys();
        } else if !now_hk && was_hk {
            widgets::resume_hotkeys();
        }
    }

    fn text_input_mut(&mut self, f: Field) -> Option<&mut TextInput> {
        Some(match f {
            Field::SaveDir => &mut self.save_dir,
            Field::Pattern => &mut self.pattern,
            Field::JpegQ => &mut self.jpeg_q,
            Field::History => &mut self.history_limit,
            Field::Delay => &mut self.delay,
            Field::Dim => &mut self.dim,
            Field::Mp4Fps => &mut self.mp4_fps,
            Field::Mp4Kbps => &mut self.mp4_kbps,
            Field::GifFps => &mut self.gif_fps,
            Field::GifMax => &mut self.gif_max,
            Field::Repo => &mut self.repo,
            Field::Hotkey(_) => return None,
        })
    }

    fn browse(&mut self, hwnd: HWND) {
        unsafe {
            let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
            if let Ok(opts) = dlg.GetOptions() {
                let _ = dlg.SetOptions(opts | FOS_PICKFOLDERS);
            }
            if dlg.Show(Some(hwnd)).is_ok() {
                if let Ok(item) = dlg.GetResult() {
                    if let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                        if let Ok(s) = p.to_string() {
                            self.save_dir.set(s);
                        }
                        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                    }
                }
            }
        }
    }

    fn parse_num(ti: &TextInput, default: u32, lo: u32, hi: u32) -> u32 {
        ti.value.trim().parse().unwrap_or(default).clamp(lo, hi)
    }

    /// Reads the UI state into a new `Settings`. Errors describe what to fix.
    fn collect(&self) -> Result<Settings, String> {
        let mut s = self.orig.clone();
        let dir = self.save_dir.value.trim();
        s.save_dir = if dir.is_empty() { None } else { Some(PathBuf::from(dir)) };
        let pat = self.pattern.value.trim();
        s.filename_pattern = if pat.is_empty() { Settings::default().filename_pattern } else { pat.to_string() };
        s.format = [Format::Png, Format::Jpg, Format::Webp][self.format.min(2)];
        s.jpeg_quality = Self::parse_num(&self.jpeg_q, 90, 1, 100) as u8;
        s.theme = [PrefTheme::System, PrefTheme::Light, PrefTheme::Dark][self.theme_pref.min(2)];
        s.history_limit = Self::parse_num(&self.history_limit, 500, 10, 100_000) as usize;

        s.capture_cursor = self.capture_cursor;
        s.capture_delay_ms = Self::parse_num(&self.delay, 0, 0, 60_000);
        s.region.quick_annotate = self.quick_annotate;
        s.region.detect_windows = self.detect_windows;
        s.region.detect_controls = self.detect_controls;
        s.region.uia_controls = self.uia_controls;
        s.region.magnifier = self.magnifier;
        s.region.crosshair = self.crosshair;
        s.region.dim_opacity = Self::parse_num(&self.dim, 110, 0, 230) as u8;
        s.scrolling_capture = self.scrolling_capture;
        s.custom_region = self.custom_region;

        s.tasks.copy = self.t_copy;
        s.tasks.save = self.t_save;
        s.tasks.open_editor = self.t_editor;
        s.tasks.pin = self.t_pin;
        s.tasks.notify = self.t_notify;
        s.tasks.sound = self.t_sound;

        // Hotkeys: validate every field first.
        let mut chosen: Vec<(Action, crate::hotkeys::Chord)> = Vec::new();
        for (i, a) in HOTKEY_ACTIONS.iter().enumerate() {
            let text = self.hotkeys[i].trim();
            if text.is_empty() {
                continue;
            }
            let chord = crate::hotkeys::Chord::parse(text).ok_or_else(|| format!("\"{text}\" (for {}) isn't a hotkey I understand.", a.label()))?;
            if let Some((other, _)) = chosen.iter().find(|(_, c)| *c == chord) {
                return Err(format!("{} is used for both {} and {}.", chord.display(), other.label(), a.label()));
            }
            chosen.push((*a, chord));
        }
        let mut workflows = Vec::new();
        for wf in &self.orig.workflows {
            match HOTKEY_ACTIONS.iter().find(|a| a.name() == wf.action) {
                None => workflows.push(wf.clone()),
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

        s.recording.mp4_fps = Self::parse_num(&self.mp4_fps, 30, 5, 60);
        s.recording.mp4_bitrate_kbps = Self::parse_num(&self.mp4_kbps, 8000, 500, 100_000);
        s.recording.gif_fps = Self::parse_num(&self.gif_fps, 15, 2, 30);
        s.recording.gif_max_seconds = Self::parse_num(&self.gif_max, 60, 1, 600);
        s.recording.cursor = self.rec_cursor;

        s.integration.start_with_windows = self.start_with_windows;
        s.integration.context_menu = self.context_menu;
        s.integration.update_check = self.update_check;
        s.integration.update_repo = self.repo.value.trim().to_string();
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
                let theme_changed = prev.theme != next.theme;
                app::with(|st| st.settings = std::rc::Rc::new(next));
                app::post(Command::ReloadSettings);
                if theme_changed {
                    super::broadcast_theme();
                }
                if !errors.is_empty() {
                    super::dialogs::info(Some(w.hwnd), "Snap It", &errors.join("\n"));
                }
                w.close();
            }
            Err(e) => super::dialogs::info(Some(w.hwnd), "Snap It", &e),
        }
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

            // 1. Scrollable content pane (drawn first so titlebar/footer naturally clip top & bottom).
            let cr = self.content_rect();
            self.draw_content(&mut pm, cr);
            widgets::draw_scrollbar(&mut pm, &t, cr, self.scroll, self.content_h, s);

            // 2. Left navigation sidebar.
            let sw = self.sidebar_w();
            let ty = self.chrome.height(s);
            if let Some(sr) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, ty as f32, sw as f32, (self.client.1 - ty) as f32) {
                pm.fill_rect(sr, &t.sidebar.paint(), id, None);
            }
            if let Some(div) = snapit_gfx::tiny_skia::Rect::from_xywh(sw as f32 - 1.0, ty as f32, 1.0, (self.client.1 - ty) as f32) {
                pm.fill_rect(div, &t.border_subtle.paint(), id, None);
            }
            let mut ny = ty as f32 + 14.0 * s;
            for (i, (name, icon)) in TABS.iter().enumerate() {
                let nr = Rect::new((12.0 * s) as i32, ny as i32, sw - (24.0 * s) as i32, (36.0 * s) as i32);
                let active = self.tab == i;
                let hovered = self.hover == Some(Hit::Nav(i));
                if active || hovered {
                    if let Some(p) = rounded_rect(nr.x as f32, nr.y as f32, nr.w as f32, nr.h as f32, 8.0 * s) {
                        let fill = if active { t.accent_soft } else { t.hover };
                        pm.fill_path(&p, &fill.paint(), FillRule::Winding, id, None);
                    }
                }
                if active {
                    if let Some(bar) = rounded_rect(nr.x as f32, nr.y as f32 + 7.0 * s, 3.0 * s, nr.h as f32 - 14.0 * s, 1.5 * s) {
                        pm.fill_path(&bar, &t.accent_paint(nr.x as f32, nr.y as f32, nr.x as f32, nr.bottom() as f32), FillRule::Winding, id, None);
                    }
                }
                let fg = if active || hovered { t.text } else { t.subtext };
                let ic = if active { t.accent } else { fg };
                let isz = 16.0 * s;
                icons::draw(&mut pm, *icon, nr.x as f32 + 12.0 * s, nr.y as f32 + (nr.h as f32 - isz) / 2.0, isz, ic);
                let l = text::layout(name, 13.0 * s, active);
                text::draw(&mut pm, name, nr.x as f32 + 36.0 * s, nr.y as f32 + (nr.h as f32 - l.height) / 2.0, 13.0 * s, active, fg, None);
                self.hits.push((nr, Hit::Nav(i)));
                ny += 42.0 * s;
            }

            // Bottom sidebar action: Run setup wizard.
            let wiz_r = Rect::new((12.0 * s) as i32, self.client.1 - (46.0 * s) as i32, sw - (24.0 * s) as i32, (34.0 * s) as i32);
            widgets::draw_button(&mut pm, &t, wiz_r, "Setup wizard…", None, BtnStyle::Ghost, self.hover == Some(Hit::Btn(BTN_WELCOME)), false, s);
            self.hits.push((wiz_r, Hit::Btn(BTN_WELCOME)));

            // 3. Sticky footer bar on the content side.
            let fh = self.footer_h();
            let fy = self.client.1 - fh;
            if let Some(fr) = snapit_gfx::tiny_skia::Rect::from_xywh(sw as f32, fy as f32, (self.client.0 - sw) as f32, fh as f32) {
                pm.fill_rect(fr, &t.sidebar.paint(), id, None);
            }
            widgets::draw_divider(&mut pm, &t, sw as f32, self.client.0 as f32, fy as f32);

            let bw = (104.0 * s) as i32;
            let bh = (34.0 * s) as i32;
            let by = fy + (fh - bh) / 2;
            let save_r = Rect::new(self.client.0 - (20.0 * s) as i32 - bw, by, bw, bh);
            let cancel_r = Rect::new(save_r.x - (10.0 * s) as i32 - bw, by, bw, bh);
            widgets::draw_button(&mut pm, &t, cancel_r, "Cancel", None, BtnStyle::Secondary, self.hover == Some(Hit::Btn(BTN_CANCEL)), false, s);
            widgets::draw_button(&mut pm, &t, save_r, "Save", None, BtnStyle::Primary, self.hover == Some(Hit::Btn(BTN_SAVE)), false, s);
            self.hits.push((cancel_r, Hit::Btn(BTN_CANCEL)));
            self.hits.push((save_r, Hit::Btn(BTN_SAVE)));

            // 4. Integrated custom titlebar (drawn last so scrolled content stays underneath).
            let ver = concat!("v", env!("CARGO_PKG_VERSION"));
            self.chrome.draw(&mut pm, &t, hwnd, self.client.0, s, "Snap It Settings", Some(ver), None);
        }
        self.bb = Some(bb);
    }

    fn draw_content(&mut self, pm: &mut PixmapMut, cr: Rect) {
        let t = self.theme;
        let s = self.scale;
        let pad = 24.0 * s;
        let x0 = cr.x as f32 + pad;
        let cw = (cr.w as f32 - pad * 2.0 - 8.0 * s).max(200.0);
        let mut y = cr.y as f32 + 20.0 * s - self.scroll;
        let y_start = y;

        // Page title header.
        let (title, _) = TABS[self.tab];
        text::draw(pm, title, x0, y, 20.0 * s, true, t.text, None);
        y += 34.0 * s;

        match self.tab {
            0 => self.draw_general(pm, cr, x0, cw, &mut y),
            1 => self.draw_capture(pm, cr, x0, cw, &mut y),
            2 => self.draw_after(pm, cr, x0, cw, &mut y),
            3 => self.draw_hotkeys(pm, cr, x0, cw, &mut y),
            4 => self.draw_recording(pm, cr, x0, cw, &mut y),
            5 => self.draw_integration(pm, cr, x0, cw, &mut y),
            _ => {}
        }

        self.content_h = (y - y_start) + 40.0 * s;
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn push_hit_clipped(&mut self, cr: Rect, r: Rect, h: Hit) {
        if let Some(vis) = r.intersect(cr) {
            self.hits.push((vis, h));
        }
    }

    fn draw_toggle_row(&mut self, pm: &mut PixmapMut, cr: Rect, x: f32, w: f32, y: &mut f32, id: u16, label: &str, desc: Option<&str>, on: bool) {
        let s = self.scale;
        let t = self.theme;
        let tw = (42.0 * s) as i32;
        let tr = Rect::new((x + w - tw as f32) as i32, (*y - 2.0 * s) as i32, tw, (24.0 * s) as i32);
        let hovered = self.hover == Some(Hit::Toggle(id));
        text::draw(pm, label, x, *y, 13.0 * s, false, t.text, None);
        widgets::draw_toggle(pm, &t, tr, on, hovered, s);
        let row_h = if let Some(d) = desc {
            let dh = widgets::draw_wrapped(pm, d, x, *y + 20.0 * s, w - tw as f32 - 14.0 * s, 11.5 * s, false, t.subtext);
            22.0 * s + dh + 8.0 * s
        } else {
            30.0 * s
        };
        let click_r = Rect::new(x as i32, (*y - 4.0 * s) as i32, w as i32, row_h as i32);
        self.push_hit_clipped(cr, click_r, Hit::Toggle(id));
        *y += row_h;
    }

    fn draw_general(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        // Card 1: Appearance
        let c1_h = 74.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c1_h as i32), 10.0, s);
        text::draw(pm, "Appearance", ix, *y + 12.0 * s, 12.0 * s, true, t.subtext, None);
        let seg_r = Rect::new(ix as i32, (*y + 32.0 * s) as i32, iw as i32, (32.0 * s) as i32);
        let hov_t = match self.hover { Some(Hit::Seg(SEG_THEME, i)) => Some(i), _ => None };
        widgets::draw_segmented(pm, &t, seg_r, &["Follow Windows", "Light", "Dark"], self.theme_pref, hov_t, s);
        for (i, sr) in widgets::segmented_rects(seg_r, 3, s).into_iter().enumerate() {
            self.push_hit_clipped(cr, sr, Hit::Seg(SEG_THEME, i));
        }
        *y += c1_h + 14.0 * s;

        // Card 2: Storage & Naming
        let c2_h = 200.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c2_h as i32), 10.0, s);
        let mut cy = *y + 14.0 * s;
        text::draw(pm, "Save screenshots to", ix, cy, 12.5 * s, true, t.text, None);
        cy += 20.0 * s;
        let browse_w = (88.0 * s) as i32;
        let dir_r = Rect::new(ix as i32, cy as i32, iw as i32 - browse_w - (8.0 * s) as i32, (32.0 * s) as i32);
        let browse_r = Rect::new(dir_r.right() + (8.0 * s) as i32, cy as i32, browse_w, (32.0 * s) as i32);
        self.save_dir.draw(pm, &t, dir_r, self.focused == Some(Field::SaveDir), self.hover == Some(Hit::Text(Field::SaveDir)), s);
        widgets::draw_button(pm, &t, browse_r, "Browse…", None, BtnStyle::Secondary, self.hover == Some(Hit::Btn(BTN_BROWSE)), false, s);
        self.push_hit_clipped(cr, dir_r, Hit::Text(Field::SaveDir));
        self.push_hit_clipped(cr, browse_r, Hit::Btn(BTN_BROWSE));
        cy += 38.0 * s;
        let def_hint = format!("Empty = {}", crate::paths::default_save_dir().display());
        text::draw(pm, &widgets::ellipsize(&def_hint, 11.5 * s, false, iw), ix, cy, 11.5 * s, false, t.subtext, None);
        cy += 24.0 * s;

        text::draw(pm, "File name pattern", ix, cy, 12.5 * s, true, t.text, None);
        cy += 20.0 * s;
        let pat_r = Rect::new(ix as i32, cy as i32, iw as i32, (32.0 * s) as i32);
        self.pattern.draw(pm, &t, pat_r, self.focused == Some(Field::Pattern), self.hover == Some(Hit::Text(Field::Pattern)), s);
        self.push_hit_clipped(cr, pat_r, Hit::Text(Field::Pattern));
        cy += 38.0 * s;
        text::draw(pm, "%y %mo %d %h %mi %s %ms  date/time · %width %height · %title · %rnd", ix, cy, 11.0 * s, false, t.subtext, None);
        *y += c2_h + 14.0 * s;

        // Card 3: Format & History
        let c3_h = 148.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c3_h as i32), 10.0, s);
        let mut cy = *y + 14.0 * s;
        text::draw(pm, "Image format", ix, cy, 12.5 * s, true, t.text, None);
        cy += 20.0 * s;
        let fmt_r = Rect::new(ix as i32, cy as i32, iw as i32, (32.0 * s) as i32);
        let hov_f = match self.hover { Some(Hit::Seg(SEG_FORMAT, i)) => Some(i), _ => None };
        widgets::draw_segmented(pm, &t, fmt_r, &["PNG", "JPEG", "WebP (lossless)"], self.format, hov_f, s);
        for (i, sr) in widgets::segmented_rects(fmt_r, 3, s).into_iter().enumerate() {
            self.push_hit_clipped(cr, sr, Hit::Seg(SEG_FORMAT, i));
        }
        cy += 46.0 * s;
        let half = (iw - 16.0 * s) / 2.0;
        text::draw(pm, "JPEG quality (1–100)", ix, cy, 12.5 * s, false, t.text, None);
        let jq_r = Rect::new((ix + half - 76.0 * s) as i32, (cy - 5.0 * s) as i32, (76.0 * s) as i32, (30.0 * s) as i32);
        self.jpeg_q.draw(pm, &t, jq_r, self.focused == Some(Field::JpegQ), self.hover == Some(Hit::Text(Field::JpegQ)), s);
        self.push_hit_clipped(cr, jq_r, Hit::Text(Field::JpegQ));

        let rx = ix + half + 16.0 * s;
        text::draw(pm, "History entries", rx, cy, 12.5 * s, false, t.text, None);
        let hl_r = Rect::new((ix + iw - 86.0 * s) as i32, (cy - 5.0 * s) as i32, (86.0 * s) as i32, (30.0 * s) as i32);
        self.history_limit.draw(pm, &t, hl_r, self.focused == Some(Field::History), self.hover == Some(Hit::Text(Field::History)), s);
        self.push_hit_clipped(cr, hl_r, Hit::Text(Field::History));
        *y += c3_h + 14.0 * s;

        let mode = if crate::paths::portable() {
            "Portable mode: settings are stored next to snapit.exe."
        } else {
            "Settings are stored in %APPDATA%\\SnapIt. Add a file named \"portable\" next to snapit.exe for portable mode."
        };
        let mh = widgets::draw_wrapped(pm, mode, x0 + 4.0 * s, *y, cw - 8.0 * s, 11.5 * s, false, t.subtext);
        *y += mh + 10.0 * s;
    }

    fn draw_capture(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        // Card 1: General capture
        let c1_h = 104.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c1_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_CURSOR, "Include mouse pointer in screenshots", None, self.capture_cursor);
        cy += 6.0 * s;
        text::draw(pm, "Delay before capture (ms)", ix, cy, 13.0 * s, false, t.text, None);
        let dr = Rect::new((ix + iw - 90.0 * s) as i32, (cy - 5.0 * s) as i32, (90.0 * s) as i32, (30.0 * s) as i32);
        self.delay.draw(pm, &t, dr, self.focused == Some(Field::Delay), self.hover == Some(Hit::Text(Field::Delay)), s);
        self.push_hit_clipped(cr, dr, Hit::Text(Field::Delay));
        text::draw(pm, "Applies to fullscreen, monitor, window, last and custom region.", ix, cy + 22.0 * s, 11.0 * s, false, t.subtext, None);
        *y += c1_h + 14.0 * s;

        // Card 2: Region selection
        let c2_h = 266.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c2_h as i32), 10.0, s);
        let mut cy = *y + 14.0 * s;
        text::draw(pm, "Region selection", ix, cy, 12.0 * s, true, t.subtext, None);
        cy += 24.0 * s;
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_QUICK, "Show quick-annotate toolbar after selecting", None, self.quick_annotate);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_DETECT_WIN, "Snap to windows under the cursor", None, self.detect_windows);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_DETECT_CTL, "Snap to controls inside windows", None, self.detect_controls);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_UIA, "Use UI Automation for modern apps (browsers, VS Code…)", None, self.uia_controls);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_MAGNIFIER, "Show pixel magnifier loupe", None, self.magnifier);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_CROSSHAIR, "Show full-screen crosshair guides", None, self.crosshair);
        cy += 6.0 * s;
        text::draw(pm, "Backdrop dim opacity (0–230)", ix, cy, 13.0 * s, false, t.text, None);
        let dim_r = Rect::new((ix + iw - 90.0 * s) as i32, (cy - 5.0 * s) as i32, (90.0 * s) as i32, (30.0 * s) as i32);
        self.dim.draw(pm, &t, dim_r, self.focused == Some(Field::Dim), self.hover == Some(Hit::Text(Field::Dim)), s);
        self.push_hit_clipped(cr, dim_r, Hit::Text(Field::Dim));
        *y += c2_h + 14.0 * s;

        // Card 3: Scrolling & Custom region
        let c3_h = 122.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c3_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        self.draw_toggle_row(
            pm,
            cr,
            ix,
            iw,
            &mut cy,
            TOG_SCROLLING,
            "Enable scrolling capture",
            Some("Sends mouse-wheel input to the selected window to stitch long pages."),
            self.scrolling_capture,
        );
        widgets::draw_divider(pm, &t, ix, ix + iw, cy);
        cy += 12.0 * s;
        let custom = match self.custom_region {
            Some(r) => format!("Custom region: {} × {} at ({}, {})", r.w, r.h, r.x, r.y),
            None => "Custom region: not set".into(),
        };
        text::draw(pm, &custom, ix, cy + 4.0 * s, 12.5 * s, false, t.subtext, None);
        let clr_r = Rect::new((ix + iw - 78.0 * s) as i32, (cy - 2.0 * s) as i32, (78.0 * s) as i32, (28.0 * s) as i32);
        widgets::draw_button(pm, &t, clr_r, "Clear", None, BtnStyle::Secondary, self.hover == Some(Hit::Btn(BTN_CLEAR_CUSTOM)), self.custom_region.is_none(), s);
        self.push_hit_clipped(cr, clr_r, Hit::Btn(BTN_CLEAR_CUSTOM));
        *y += c3_h + 14.0 * s;
    }

    fn draw_after(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        let desc_h = widgets::draw_wrapped(
            pm,
            "Choose what runs automatically when you finish a capture (pressing Enter in the overlay, or any instant capture):",
            x0,
            *y,
            cw,
            12.5 * s,
            false,
            t.subtext,
        );
        *y += desc_h + 12.0 * s;

        let card_h = 212.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, card_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_COPY, "Copy image to the clipboard", None, self.t_copy);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_SAVE, "Save image to the screenshots folder", None, self.t_save);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_EDITOR, "Open in the Snap It editor", None, self.t_editor);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_PIN, "Pin above other windows", None, self.t_pin);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_NOTIFY, "Show a notification", None, self.t_notify);
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_T_SOUND, "Play the capture shutter sound", None, self.t_sound);
        *y += card_h + 14.0 * s;

        text::draw(pm, "Per-hotkey task chains can also be customised in settings.toml under [[workflows]].", x0 + 4.0 * s, *y, 11.5 * s, false, t.subtext, None);
        *y += 24.0 * s;
    }

    fn draw_hotkeys(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        let dh = widgets::draw_wrapped(
            pm,
            "Click any field and press your desired key combination (e.g. Ctrl+Shift+PrintScreen). Press Backspace or Delete to clear.",
            x0,
            *y,
            cw,
            12.5 * s,
            false,
            t.subtext,
        );
        *y += dh + 12.0 * s;

        let row_h = 38.0 * s;
        let card_h = HOTKEY_ACTIONS.len() as f32 * row_h + 16.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, card_h as i32), 10.0, s);
        let mut cy = *y + 10.0 * s;
        for (i, a) in HOTKEY_ACTIONS.iter().enumerate() {
            if i > 0 {
                widgets::draw_divider(pm, &t, ix, ix + iw, cy - 2.0 * s);
            }
            let label = match a {
                Action::OpenEditor => "Open image editor",
                Action::OpenHistory => "Open history",
                Action::OpenSettings => "Open settings",
                _ => a.label().trim_end_matches('…'),
            };
            text::draw(pm, label, ix, cy + 8.0 * s, 13.0 * s, false, t.text, None);
            let fw = (224.0 * s) as i32;
            let fr = Rect::new((ix + iw - fw as f32) as i32, (cy + 2.0 * s) as i32, fw, (30.0 * s) as i32);
            widgets::draw_hotkey_field(
                pm,
                &t,
                fr,
                &self.hotkeys[i],
                self.focused == Some(Field::Hotkey(i)),
                self.hover == Some(Hit::Hotkey(i)),
                s,
            );
            self.push_hit_clipped(cr, fr, Hit::Hotkey(i));
            cy += row_h;
        }
        *y += card_h + 14.0 * s;
    }

    fn draw_recording(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        let card_h = 224.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, card_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        for (lbl, field) in [
            ("MP4 frames per second (5–60)", Field::Mp4Fps),
            ("MP4 bitrate (kbit/s)", Field::Mp4Kbps),
            ("GIF frames per second (2–30)", Field::GifFps),
            ("GIF maximum length (seconds)", Field::GifMax),
        ] {
            text::draw(pm, lbl, ix, cy + 4.0 * s, 13.0 * s, false, t.text, None);
            let fr = Rect::new((ix + iw - 96.0 * s) as i32, cy as i32, (96.0 * s) as i32, (30.0 * s) as i32);
            let focused = self.focused == Some(field);
            let hovered = self.hover == Some(Hit::Text(field));
            match field {
                Field::Mp4Fps => self.mp4_fps.draw(pm, &t, fr, focused, hovered, s),
                Field::Mp4Kbps => self.mp4_kbps.draw(pm, &t, fr, focused, hovered, s),
                Field::GifFps => self.gif_fps.draw(pm, &t, fr, focused, hovered, s),
                Field::GifMax => self.gif_max.draw(pm, &t, fr, focused, hovered, s),
                _ => {}
            }
            self.push_hit_clipped(cr, fr, Hit::Text(field));
            cy += 40.0 * s;
        }
        widgets::draw_divider(pm, &t, ix, ix + iw, cy);
        cy += 12.0 * s;
        self.draw_toggle_row(pm, cr, ix, iw, &mut cy, TOG_REC_CURSOR, "Show mouse pointer in recordings", None, self.rec_cursor);
        *y += card_h + 14.0 * s;

        text::draw(pm, "Recordings never include audio. Press the record hotkey again (or Stop) to finish.", x0 + 4.0 * s, *y, 11.5 * s, false, t.subtext, None);
        *y += 24.0 * s;
    }

    fn draw_integration(&mut self, pm: &mut PixmapMut, cr: Rect, x0: f32, cw: f32, y: &mut f32) {
        let s = self.scale;
        let t = self.theme;
        let pad = 16.0 * s;
        let ix = x0 + pad;
        let iw = cw - pad * 2.0;

        let c1_h = 176.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c1_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        self.draw_toggle_row(
            pm,
            cr,
            ix,
            iw,
            &mut cy,
            TOG_STARTUP,
            "Start Snap It when I sign in",
            Some("Adds a shortcut to your Startup folder so hotkeys work right away."),
            self.start_with_windows,
        );
        self.draw_toggle_row(
            pm,
            cr,
            ix,
            iw,
            &mut cy,
            TOG_CTXMENU,
            "Add Snap It to the Explorer context menu",
            Some("Right-click image files for \"Edit with Snap It\" and \"Pin with Snap It\"."),
            self.context_menu,
        );
        *y += c1_h + 14.0 * s;

        let c2_h = 148.0 * s;
        widgets::draw_card(pm, &t, Rect::new(x0 as i32, *y as i32, cw as i32, c2_h as i32), 10.0, s);
        let mut cy = *y + 16.0 * s;
        self.draw_toggle_row(
            pm,
            cr,
            ix,
            iw,
            &mut cy,
            TOG_UPDATE,
            "Check for new versions at startup",
            Some("Asks before downloading; updates open in your browser."),
            self.update_check,
        );
        cy += 4.0 * s;
        text::draw(pm, "Release repository (owner/repo)", ix, cy, 12.5 * s, false, t.subtext, None);
        cy += 20.0 * s;
        let chk_w = (106.0 * s) as i32;
        let rr = Rect::new(ix as i32, cy as i32, iw as i32 - chk_w - (8.0 * s) as i32, (32.0 * s) as i32);
        let chk_r = Rect::new(rr.right() + (8.0 * s) as i32, cy as i32, chk_w, (32.0 * s) as i32);
        self.repo.draw(pm, &t, rr, self.focused == Some(Field::Repo), self.hover == Some(Hit::Text(Field::Repo)), s);
        widgets::draw_button(pm, &t, chk_r, "Check now", None, BtnStyle::Secondary, self.hover == Some(Hit::Btn(BTN_CHECK_NOW)), false, s);
        self.push_hit_clipped(cr, rr, Hit::Text(Field::Repo));
        self.push_hit_clipped(cr, chk_r, Hit::Btn(BTN_CHECK_NOW));
        *y += c2_h + 14.0 * s;

        let open_r = Rect::new(x0 as i32, *y as i32, (190.0 * s) as i32, (34.0 * s) as i32);
        widgets::draw_button(pm, &t, open_r, "Open settings folder", Some(Icon::Folder), BtnStyle::Secondary, self.hover == Some(Hit::Btn(BTN_OPEN_CONFIG)), false, s);
        self.push_hit_clipped(cr, open_r, Hit::Btn(BTN_OPEN_CONFIG));
        *y += 46.0 * s;
    }

    fn on_click(&mut self, w: &mut Win, x: i32, y: i32) {
        if self.chrome.release(w.hwnd, self.client.0, self.scale, x, y) {
            w.invalidate();
            return;
        }
        let hit = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
        match hit {
            Some(Hit::Nav(i)) => {
                self.tab = i;
                self.scroll = 0.0;
                self.set_focus(None);
            }
            Some(Hit::Text(f)) => self.set_focus(Some(f)),
            Some(Hit::Hotkey(i)) => self.set_focus(Some(Field::Hotkey(i))),
            Some(Hit::Seg(SEG_FORMAT, i)) => {
                self.format = i;
                self.set_focus(None);
            }
            Some(Hit::Seg(SEG_THEME, i)) => {
                self.theme_pref = i;
                self.set_focus(None);
            }
            Some(Hit::Toggle(id)) => {
                self.set_focus(None);
                match id {
                    TOG_CURSOR => self.capture_cursor = !self.capture_cursor,
                    TOG_QUICK => self.quick_annotate = !self.quick_annotate,
                    TOG_DETECT_WIN => self.detect_windows = !self.detect_windows,
                    TOG_DETECT_CTL => self.detect_controls = !self.detect_controls,
                    TOG_UIA => self.uia_controls = !self.uia_controls,
                    TOG_MAGNIFIER => self.magnifier = !self.magnifier,
                    TOG_CROSSHAIR => self.crosshair = !self.crosshair,
                    TOG_SCROLLING => self.scrolling_capture = !self.scrolling_capture,
                    TOG_T_COPY => self.t_copy = !self.t_copy,
                    TOG_T_SAVE => self.t_save = !self.t_save,
                    TOG_T_EDITOR => self.t_editor = !self.t_editor,
                    TOG_T_PIN => self.t_pin = !self.t_pin,
                    TOG_T_NOTIFY => self.t_notify = !self.t_notify,
                    TOG_T_SOUND => self.t_sound = !self.t_sound,
                    TOG_REC_CURSOR => self.rec_cursor = !self.rec_cursor,
                    TOG_STARTUP => self.start_with_windows = !self.start_with_windows,
                    TOG_CTXMENU => self.context_menu = !self.context_menu,
                    TOG_UPDATE => self.update_check = !self.update_check,
                    _ => {}
                }
            }
            Some(Hit::Btn(id)) => {
                self.set_focus(None);
                match id {
                    BTN_SAVE => self.save(w),
                    BTN_CANCEL => w.close(),
                    BTN_BROWSE => self.browse(w.hwnd),
                    BTN_CLEAR_CUSTOM => {
                        if self.custom_region.is_some() {
                            self.custom_region = None;
                            super::dialogs::info(Some(w.hwnd), "Snap It", "The custom region will be cleared when you save.");
                        }
                    }
                    BTN_CHECK_NOW => {
                        let repo = self.repo.value.trim().to_string();
                        app::update_settings(|s| s.integration.update_repo = repo);
                        crate::integration::check_for_updates(true);
                    }
                    BTN_WELCOME => {
                        app::post(Command::OpenWelcome);
                        w.close();
                    }
                    BTN_OPEN_CONFIG => crate::shell::open_folder(&crate::paths::config_dir()),
                    _ => {}
                }
            }
            _ => self.set_focus(None),
        }
        w.invalidate();
    }
}

impl Handler for SettingsWin {
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
                if matches!(self.focused, Some(Field::Hotkey(_))) {
                    self.set_focus(None);
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
                let cur = match self.hover {
                    Some(Hit::Text(_)) => IDC_IBEAM,
                    Some(_) => IDC_HAND,
                    None if self.chrome.hover.is_some() => IDC_HAND,
                    None => IDC_ARROW,
                };
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
                self.on_click(w, x, y);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) as u16 as i16) as f32;
                let next = (self.scroll - delta * 0.6 * self.scale).clamp(0.0, self.max_scroll());
                if (next - self.scroll).abs() > 0.5 {
                    self.scroll = next;
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wp.0 as u32;
                if let Some(Field::Hotkey(i)) = self.focused {
                    if vk == 0x1B {
                        self.set_focus(None);
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
                    if self.focused.is_some() {
                        self.set_focus(None);
                        w.invalidate();
                    } else {
                        w.close();
                    }
                    return Some(LRESULT(0));
                }
                if let Some(f) = self.focused {
                    if let Some(ti) = self.text_input_mut(f) {
                        if ti.on_key(vk, super::ctrl()) {
                            w.invalidate();
                        }
                    }
                }
                Some(LRESULT(0))
            }
            WM_KEYUP | WM_SYSKEYUP => {
                let vk = wp.0 as u32;
                if let Some(Field::Hotkey(i)) = self.focused {
                    if let Some(new_val) = widgets::handle_hotkey_msg(true, vk) {
                        self.hotkeys[i] = new_val;
                        w.invalidate();
                    }
                    return Some(LRESULT(0));
                }
                None
            }
            WM_CHAR => {
                if let Some(f) = self.focused {
                    if !matches!(f, Field::Hotkey(_)) {
                        if let Some(ch) = char::from_u32(wp.0 as u32) {
                            if let Some(ti) = self.text_input_mut(f) {
                                if ti.on_char(ch) {
                                    w.invalidate();
                                }
                            }
                        }
                    }
                }
                Some(LRESULT(0))
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as u16;
                if id == ID_SAVE {
                    self.save(w);
                    return Some(LRESULT(0));
                }
                None
            }
            WM_DESTROY => {
                widgets::resume_hotkeys();
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}
