//! History browser: a thumbnail grid of past captures (local files only).
//! Thumbnails are decoded on a worker thread and posted back as they finish.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, Pixmap, PixmapPaint, Stroke, Transform};
use snapit_gfx::{text, Rect};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetClientRect, GetCursorPos, LoadCursorW,
    MessageBoxW, PostMessageW, SetCursor, SetForegroundWindow, ShowWindow, TrackPopupMenu,
    HTCLIENT, IDC_ARROW, IDC_HAND, IDYES, MB_ICONWARNING, MB_YESNO, MF_SEPARATOR, MF_STRING,
    SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE,
    WM_CONTEXTMENU, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE, WS_EX_APPWINDOW,
    WS_OVERLAPPEDWINDOW,
};

use super::a11y::{vk::*, Access, Action as AxAction, Node, Role};
use super::chrome::Chrome;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use super::icons::Icon;
use super::theme::{self, Theme};
use super::widgets::{self, BtnStyle};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};
use crate::history::{self as hist, Entry};

const WM_THUMB: u32 = WM_APP + 30;

/// Accessibility / focus keys: the Open folder button, and capture `i` at `ENTRY_KEY + i`
/// (above the title-bar buttons' keys, so any history length fits).
const K_FOLDER: u32 = 1;
const ENTRY_KEY: u32 = 0x1_0000;
const VK_APPS: u32 = 0x5D;

fn entry_key(i: usize) -> u32 {
    ENTRY_KEY + i as u32
}

fn entry_of(k: u32) -> Option<usize> {
    k.checked_sub(ENTRY_KEY).map(|i| i as usize)
}

thread_local! {
    static OPEN: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

struct Thumb {
    gen: u32,
    idx: usize,
    img: Option<Pixmap>,
}

struct History {
    theme: Theme,
    scale: f32,
    chrome: Chrome,
    entries: Vec<Entry>,
    thumbs: HashMap<usize, Option<Pixmap>>,
    requested: HashSet<usize>,
    jobs: Option<Sender<(u32, usize, PathBuf, u32, u32)>>,
    gen: u32,
    scroll: f32,
    client: (i32, i32),
    bb: Option<Backbuffer>,
    selected: Option<usize>,
    hover: Option<usize>,
    hover_folder: bool,
    folder_pressed: bool,
    folder_btn: Rect,
    /// Keyboard focus, Tab order and the UI Automation tree.
    ax: Access,
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
    let s = app::settings();
    let entries = hist::load(&crate::paths::history_file(), s.history_limit);
    let h = History {
        theme: theme::current(),
        scale: 1.0,
        chrome: Chrome::new(true, true, true),
        entries,
        thumbs: HashMap::new(),
        requested: HashSet::new(),
        jobs: None,
        gen: 0,
        scroll: 0.0,
        client: (0, 0),
        bb: None,
        selected: None,
        hover: None,
        hover_folder: false,
        folder_pressed: false,
        folder_btn: Rect::default(),
        ax: Access::default(),
    };
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h_) = (((980.0 * sc) as i32).min(work.w * 9 / 10), ((700.0 * sc) as i32).min(work.h * 9 / 10));
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h_) / 2, w, h_);
    match super::create("SnapItHistory", WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, "Snap It History", r, None, Box::new(h)) {
        Ok(hwnd) => {
            Chrome::init(hwnd);
            OPEN.with(|o| o.set(hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("history: {e}")),
    }
}

fn start_worker(hwnd: HWND) -> Sender<(u32, usize, PathBuf, u32, u32)> {
    let (tx, rx) = mpsc::channel::<(u32, usize, PathBuf, u32, u32)>();
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        for (gen, idx, path, tw, th) in rx {
            let img = snapit_gfx::io::load(&path).ok().and_then(|p| {
                let k = (tw as f32 / p.width() as f32).min(th as f32 / p.height() as f32).min(1.0);
                snapit_gfx::effects::resize(&p, ((p.width() as f32 * k) as u32).max(1), ((p.height() as f32 * k) as u32).max(1))
            });
            let raw = Box::into_raw(Box::new(Thumb { gen, idx, img }));
            unsafe {
                if PostMessageW(Some(HWND(h as *mut _)), WM_THUMB, WPARAM(0), LPARAM(raw as isize)).is_err() {
                    drop(Box::from_raw(raw));
                    break; // window gone
                }
            }
        }
    });
    tx
}

impl History {
    fn header_h(&self) -> f32 {
        self.chrome.height(self.scale) as f32
    }
    fn cell(&self) -> (f32, f32) {
        (230.0 * self.scale, 204.0 * self.scale)
    }
    fn thumb_size(&self) -> (u32, u32) {
        ((210.0 * self.scale) as u32, (130.0 * self.scale) as u32)
    }
    fn cols(&self) -> usize {
        let (cw, _) = self.cell();
        (((self.client.0 as f32 - 16.0 * self.scale) / cw).floor() as usize).max(1)
    }
    fn content_h(&self) -> f32 {
        let rows = self.entries.len().div_ceil(self.cols());
        rows as f32 * self.cell().1 + 16.0 * self.scale
    }
    fn max_scroll(&self) -> f32 {
        (self.content_h() - (self.client.1 as f32 - self.header_h())).max(0.0)
    }
    fn cell_rect(&self, i: usize) -> Rect {
        let (cw, ch) = self.cell();
        let cols = self.cols();
        let left = (self.client.0 as f32 - cols as f32 * cw) / 2.0;
        let x = left + (i % cols) as f32 * cw;
        let y = self.header_h() + 8.0 * self.scale + (i / cols) as f32 * ch - self.scroll;
        Rect::new(x as i32, y as i32, cw as i32, ch as i32)
    }
    fn index_at(&self, x: i32, y: i32) -> Option<usize> {
        if (y as f32) < self.header_h() {
            return None;
        }
        (0..self.entries.len()).find(|&i| self.cell_rect(i).inflate(-(4.0 * self.scale) as i32).contains(x, y))
    }

    fn request_visible(&mut self, hwnd: HWND) {
        if self.jobs.is_none() {
            self.jobs = Some(start_worker(hwnd));
        }
        let (tw, th) = self.thumb_size();
        let visible: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let r = self.cell_rect(i);
                r.bottom() > 0 && r.y < self.client.1 + r.h // one row of lookahead
            })
            .collect();
        for i in visible {
            if self.requested.insert(i) {
                if let Some(tx) = &self.jobs {
                    let _ = tx.send((self.gen, i, self.entries[i].path.clone(), tw, th));
                }
            }
        }
    }

    fn render(&mut self, hwnd: HWND) {
        Backbuffer::ensure(&mut self.bb, self.client.0.max(1) as u32, self.client.1.max(1) as u32);
        let Some(mut bb) = self.bb.take() else { return };
        {
            let mut pm = bb.pixmap();
            self.draw(&mut pm, hwnd);
            self.build_nodes(hwnd);
            if let Some(r) = self.ax.ring_rect() {
                widgets::draw_focus_ring(&mut pm, &self.theme, r, 10.0, self.scale);
            }
        }
        self.bb = Some(bb);
        self.ax.publish(hwnd);
    }

    /// The accessible tree: the Open folder button, then the captures as list items. Only
    /// captures near the visible area (and the selected one) are listed, so a long history
    /// stays fast; arrow keys scroll the rest into view.
    fn build_nodes(&mut self, hwnd: HWND) {
        self.ax.begin();
        self.ax.push(Node::new(K_FOLDER, self.folder_btn, Role::Button, "Open screenshots folder"));
        let hh = self.header_h() as i32;
        let near = self.cell().1 as i32 * 2;
        // The grid is one Tab stop: the selected capture, else the first one shown.
        let mut tab_stop = self.selected;
        for i in 0..self.entries.len() {
            let r = self.cell_rect(i).inflate(-(6.0 * self.scale) as i32);
            let visible = r.bottom() > hh && r.y < self.client.1;
            if !(visible || r.bottom() > hh - near && r.y < self.client.1 + near || self.selected == Some(i)) {
                continue;
            }
            if tab_stop.is_none() && visible {
                tab_stop = Some(i);
            }
            let e = &self.entries[i];
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let kind = e.kind.strip_prefix("capture_").unwrap_or(&e.kind).replace('_', " ");
            let mut n = Node::new(entry_key(i), r, Role::ListItem, name)
                .help(format!("{} × {}, {kind}. Enter opens it in the editor; Delete removes it from history.", e.width, e.height))
                .checked(self.selected == Some(i));
            n.offscreen = !visible;
            n.tab_stop = tab_stop == Some(i);
            self.ax.push(n);
        }
        let chrome_nodes = self.chrome.a11y_nodes(hwnd, self.client.0, self.scale);
        self.ax.nodes.extend(chrome_nodes);
    }

    /// Selects capture `i` with keyboard focus and scrolls it into view.
    fn select(&mut self, hwnd: HWND, i: usize, ring: bool) {
        if self.entries.is_empty() {
            return;
        }
        let i = i.min(self.entries.len() - 1);
        self.selected = Some(i);
        self.ax.focus = Some(entry_key(i));
        self.ax.ring = ring;
        let r = self.cell_rect(i);
        let top = self.header_h() as i32;
        if r.y < top {
            self.scroll -= (top - r.y) as f32;
        } else if r.bottom() > self.client.1 {
            self.scroll += (r.bottom() - self.client.1) as f32;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        self.request_visible(hwnd);
    }

    /// Arrow / paging keys in the grid. Returns `true` if used.
    fn grid_key(&mut self, hwnd: HWND, vk: u32) -> bool {
        let n = self.entries.len();
        if n == 0 {
            return false;
        }
        let cols = self.cols();
        let page = (((self.client.1 as f32 - self.header_h()) / self.cell().1).floor() as usize).max(1) * cols;
        let cur = self.selected.unwrap_or(0);
        let next = match vk {
            VK_LEFT => cur.saturating_sub(1),
            VK_RIGHT => (cur + 1).min(n - 1),
            VK_UP => cur.saturating_sub(cols),
            VK_DOWN => (cur + cols).min(n - 1),
            VK_PRIOR => cur.saturating_sub(page),
            VK_NEXT => (cur + page).min(n - 1),
            VK_HOME => 0,
            VK_END => n - 1,
            _ => return false,
        };
        // The first arrow press just selects the capture that has focus.
        self.select(hwnd, if self.selected.is_none() { cur } else { next }, true);
        true
    }

    fn draw(&mut self, pm: &mut snapit_gfx::tiny_skia::PixmapMut, hwnd: HWND) {
        let t = self.theme;
        let s = self.scale;
        let id = Transform::identity();
        pm.fill(t.bg.color());

        for i in 0..self.entries.len() {
            let r = self.cell_rect(i);
            if r.bottom() < self.header_h() as i32 || r.y > self.client.1 {
                continue;
            }
            let inner = r.inflate(-(6.0 * s) as i32);
            let sel = self.selected == Some(i);
            let hov = self.hover == Some(i);
            let bg = if sel { t.accent_soft } else if hov { t.raised } else { t.panel };
            if let Some(p) = rounded_rect(inner.x as f32, inner.y as f32, inner.w as f32, inner.h as f32, 10.0 * s) {
                pm.fill_path(&p, &bg.paint(), FillRule::Winding, id, None);
                if sel {
                    pm.stroke_path(
                        &p,
                        &t.accent_paint(inner.x as f32, inner.y as f32, inner.right() as f32, inner.bottom() as f32),
                        &Stroke { width: 2.0, ..Default::default() },
                        id,
                        None,
                    );
                } else {
                    let bc = if hov { t.border } else { t.border_subtle };
                    pm.stroke_path(&p, &bc.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
                }
            }
            let (tw, th) = self.thumb_size();
            let tx = inner.x as f32 + (inner.w as f32 - tw as f32) / 2.0;
            let ty = inner.y as f32 + 8.0 * s;
            let e = &self.entries[i];
            if let Some(well) = rounded_rect(tx, ty, tw as f32, th as f32, 6.0 * s) {
                pm.fill_path(&well, &t.input_bg.paint(), FillRule::Winding, id, None);
            }
            match self.thumbs.get(&i) {
                Some(Some(img)) => {
                    let ox = tx + (tw as f32 - img.width() as f32) / 2.0;
                    let oy = ty + (th as f32 - img.height() as f32) / 2.0;
                    pm.draw_pixmap(ox as i32, oy as i32, img.as_ref(), &PixmapPaint::default(), id, None);
                }
                Some(None) => {
                    let msg = "File missing";
                    let l = text::layout(msg, 12.0 * s, false);
                    text::draw(pm, msg, tx + (tw as f32 - l.width) / 2.0, ty + th as f32 / 2.0 - l.height / 2.0, 12.0 * s, false, t.subtext, None);
                }
                None => {}
            }
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = ellipsize(&name, 12.0 * s, inner.w as f32 - 16.0 * s);
            text::draw(pm, &name, inner.x as f32 + 8.0 * s, ty + th as f32 + 8.0 * s, 12.0 * s, true, t.text, None);
            let meta = format!("{} × {}", e.width, e.height);
            text::draw(pm, &meta, inner.x as f32 + 8.0 * s, ty + th as f32 + 27.0 * s, 11.0 * s, false, t.subtext, None);
            let kind_lbl = e.kind.strip_prefix("capture_").unwrap_or(&e.kind).replace('_', " ");
            let kl = text::layout(&kind_lbl, 10.5 * s, true);
            widgets::draw_badge(pm, &t, inner.right() as f32 - kl.width - 20.0 * s, ty + th as f32 + 24.0 * s, &kind_lbl, false, s);
        }

        // Custom scrollbar.
        let hh = self.header_h() as i32;
        widgets::draw_scrollbar(pm, &t, Rect::new(0, hh, self.client.0, (self.client.1 - hh).max(1)), self.scroll, self.content_h(), s);

        // Integrated custom titlebar with count badge and Open folder button.
        let count_badge = format!("{} captures", self.entries.len());
        let bw = (132.0 * s) as i32;
        // Keep the title and badge clear of the Open folder button.
        self.chrome.reserve_right = bw + (12.0 * s) as i32;
        self.chrome.draw(pm, &t, hwnd, self.client.0, s, "History", Some(&count_badge), None);

        let bh = (28.0 * s) as i32;
        let bx = self.chrome.buttons_left(self.client.0, s) - bw - (12.0 * s) as i32;
        let by = (hh - bh) / 2;
        self.folder_btn = Rect::new(bx, by, bw, bh);
        widgets::draw_button(pm, &t, self.folder_btn, "Open folder", Some(Icon::Folder), BtnStyle::Secondary, self.hover_folder, false, s);

        let hint = "Double-click to edit · right-click for more";
        let hl = text::layout(hint, 11.5 * s, false);
        let hx = self.folder_btn.x as f32 - hl.width - 14.0 * s;
        if hx > 240.0 * s {
            text::draw(pm, hint, hx, (hh as f32 - hl.height) / 2.0, 11.5 * s, false, t.subtext, None);
        }

        if self.entries.is_empty() {
            let msg = "No captures yet. Saved screenshots will appear here.";
            let l = text::layout(msg, 14.0 * s, false);
            text::draw(pm, msg, (self.client.0 as f32 - l.width) / 2.0, self.client.1 as f32 / 2.0, 14.0 * s, false, t.subtext, None);
        }
    }

    fn open_entry(&self, i: usize) {
        let p = &self.entries[i].path;
        match snapit_gfx::io::load(p) {
            Ok(image) => app::post(Command::OpenEditor { image, path: Some(p.clone()) }),
            Err(e) => app::post(Command::Notify { title: "Could not open".into(), body: e }),
        }
    }

    fn remove(&mut self, i: usize) {
        self.entries.remove(i);
        self.thumbs.clear();
        self.requested.clear();
        self.gen += 1; // results still in flight for old indices are ignored
        self.selected = None;
        let _ = hist::rewrite(&crate::paths::history_file(), &self.entries);
    }

    /// Shows the right-click menu for capture `i` at `at` (screen coordinates), or at the
    /// mouse pointer.
    fn context_menu(&mut self, hwnd: HWND, i: usize, at: Option<POINT>) {
        const OPEN: usize = 1;
        const COPY: usize = 2;
        const PIN: usize = 3;
        const FOLDER: usize = 4;
        const PATH: usize = 5;
        const FORGET: usize = 6;
        const DELETE: usize = 7;
        let id = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let add = |id: usize, s: &str| {
                let _ = AppendMenuW(m, MF_STRING, id, &HSTRING::from(s));
            };
            add(OPEN, "Open in editor");
            add(COPY, "Copy image");
            add(PIN, "Pin to screen");
            add(FOLDER, "Show in folder");
            add(PATH, "Copy file path");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            add(FORGET, "Remove from history");
            add(DELETE, "Delete file…");
            let p = at.unwrap_or_else(|| {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                p
            });
            let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, hwnd, None);
            let _ = DestroyMenu(m);
            r.0 as usize
        };
        let path = self.entries[i].path.clone();
        match id {
            OPEN => self.open_entry(i),
            COPY | PIN => match snapit_gfx::io::load(&path) {
                Ok(img) if id == COPY => {
                    let _ = crate::clipboard::set_image(&img);
                }
                Ok(img) => app::post(Command::Pin { image: img, at: None }),
                Err(e) => app::post(Command::Notify { title: "Could not open".into(), body: e }),
            },
            FOLDER => crate::shell::show_in_folder(&path),
            PATH => {
                let _ = crate::clipboard::set_text(&path.to_string_lossy());
            }
            FORGET => self.remove(i),
            DELETE => {
                let msg = format!("Delete this file permanently?\n\n{}", path.display());
                let r = unsafe { MessageBoxW(Some(hwnd), &HSTRING::from(msg), &HSTRING::from("Snap It"), MB_YESNO | MB_ICONWARNING) };
                if r == IDYES {
                    match std::fs::remove_file(&path) {
                        Ok(()) => self.remove(i),
                        Err(e) => super::dialogs::info(Some(hwnd), "Snap It", &format!("Could not delete the file.\n{e}")),
                    }
                }
            }
            _ => {}
        }
    }
}

fn ellipsize(s: &str, size: f32, max_w: f32) -> String {
    if text::layout(s, size, false).width <= max_w {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while chars.len() > 4 {
        chars.remove(chars.len() / 2);
        let t: String = chars[..chars.len() / 2].iter().chain(['…'].iter()).chain(chars[chars.len() / 2..].iter()).collect();
        if text::layout(&t, size, false).width <= max_w {
            return t;
        }
    }
    s.chars().take(4).collect()
}

impl History {
    /// Context menu for the selected capture, opened from the keyboard (at the capture).
    fn keyboard_menu(&mut self, w: &mut Win) {
        let Some(i) = self.selected.filter(|&i| i < self.entries.len()) else { return };
        let r = self.cell_rect(i);
        let mut p = POINT { x: r.x + r.w / 2, y: r.y + r.h / 2 };
        unsafe {
            let _ = ClientToScreen(w.hwnd, &mut p);
        }
        self.context_menu(w.hwnd, i, Some(p));
        self.request_visible(w.hwnd);
        w.invalidate();
    }
}

impl Handler for History {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn a11y_action(&mut self, w: &mut Win, k: u32, action: AxAction) {
        if Chrome::a11y_invoke(w.hwnd, k) {
            return;
        }
        if k == K_FOLDER {
            self.ax.focus = Some(K_FOLDER);
            if action == AxAction::Invoke {
                crate::shell::open_folder(&app::settings().save_dir());
            }
        } else if let Some(i) = entry_of(k).filter(|&i| i < self.entries.len()) {
            self.select(w.hwnd, i, true);
            if action == AxAction::Invoke {
                self.open_entry(i);
            }
        }
        w.invalidate();
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == super::WM_SNAPIT_THEME {
            self.theme = super::theme::current();
        }
        if let Some((r, dirty)) = self.chrome.nc_message(w.hwnd, msg, wp) {
            if dirty {
                w.invalidate();
            }
            return Some(r);
        }
        match msg {
            WM_NCCALCSIZE => Chrome::handle_nccalcsize(w.hwnd, wp, lp),
            WM_NCHITTEST => Some(self.chrome.hit_test(w.hwnd, lp, self.scale, &[self.folder_btn])),
            WM_SIZE => {
                let mut rc = Default::default();
                unsafe {
                    let _ = GetClientRect(w.hwnd, &mut rc);
                }
                self.client = (rc.right, rc.bottom);
                self.scale = w.dpi_scale();
                theme::apply_titlebar(w.hwnd, &self.theme);
                self.scroll = self.scroll.min(self.max_scroll());
                self.request_visible(w.hwnd);
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_PAINT => {
                self.render(w.hwnd);
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                let cur = if self.hover_folder || self.chrome.hover.is_some() || self.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                unsafe {
                    if let Ok(c) = LoadCursorW(None, cur) {
                        SetCursor(Some(c));
                    }
                }
                Some(LRESULT(1))
            }
            WM_THUMB => {
                let th = unsafe { Box::from_raw(lp.0 as *mut Thumb) };
                if th.gen == self.gen && th.idx < self.entries.len() {
                    self.thumbs.insert(th.idx, th.img);
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                self.scroll = (self.scroll - delta as f32 / 120.0 * 80.0 * self.scale).clamp(0.0, self.max_scroll());
                self.request_visible(w.hwnd);
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = super::lparam_xy(lp);
                let mut dirty = self.chrome.set_hover(w.hwnd, self.client.0, self.scale, x, y);
                let hf = self.folder_btn.contains(x, y);
                if hf != self.hover_folder {
                    self.hover_folder = hf;
                    dirty = true;
                }
                let h = self.index_at(x, y);
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
                self.folder_pressed = self.folder_btn.contains(x, y);
                if self.folder_pressed {
                    self.ax.set_focus(Some(K_FOLDER));
                }
                if (y as f32) >= self.header_h() {
                    self.selected = self.index_at(x, y);
                    self.ax.set_focus(self.selected.map(entry_key));
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = super::lparam_xy(lp);
                let folder_pressed = std::mem::take(&mut self.folder_pressed);
                if self.chrome.release(w.hwnd, self.client.0, self.scale, x, y) {
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                if folder_pressed && self.folder_btn.contains(x, y) {
                    crate::shell::open_folder(&app::settings().save_dir());
                }
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.chrome.mouse_leave();
                self.hover_folder = false;
                self.hover = None;
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = super::lparam_xy(lp);
                if let Some(i) = self.index_at(x, y) {
                    self.open_entry(i);
                }
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                let (x, y) = super::lparam_xy(lp);
                if let Some(i) = self.index_at(x, y) {
                    self.selected = Some(i);
                    self.ax.set_focus(Some(entry_key(i)));
                    w.invalidate();
                    self.context_menu(w.hwnd, i, None);
                    self.request_visible(w.hwnd);
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            // Shift+F10 (DefWindowProc turns it into this with lParam -1): menu for the selection.
            WM_CONTEXTMENU if lp.0 as i32 == -1 => {
                self.keyboard_menu(w);
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                let vk = wp.0 as u32;
                let shift = super::shift();
                let on_folder = self.ax.focus == Some(K_FOLDER);
                match vk {
                    VK_TAB => {
                        self.ax.focus = self.ax.next(shift);
                        self.ax.ring = true;
                        if let Some(i) = self.ax.focus.and_then(entry_of) {
                            self.select(w.hwnd, i, true);
                        }
                    }
                    VK_ESCAPE => w.close(),
                    VK_RETURN | VK_SPACE if on_folder => crate::shell::open_folder(&app::settings().save_dir()),
                    VK_RETURN => {
                        if let Some(i) = self.selected {
                            self.open_entry(i);
                        }
                    }
                    VK_DELETE => {
                        if let Some(i) = self.selected {
                            self.remove(i);
                            // Keep going from the same place.
                            if !self.entries.is_empty() {
                                self.select(w.hwnd, i, true);
                            }
                            self.request_visible(w.hwnd);
                        }
                    }
                    VK_APPS => self.keyboard_menu(w),
                    _ if !on_folder => {
                        self.grid_key(w.hwnd, vk);
                    }
                    _ => {}
                }
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                self.jobs = None;
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}
