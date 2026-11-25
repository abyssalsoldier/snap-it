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
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetClientRect, GetCursorPos, MessageBoxW,
    PostMessageW, SetForegroundWindow, ShowWindow, TrackPopupMenu, IDYES, MB_ICONWARNING,
    MB_YESNO, MF_SEPARATOR, MF_STRING, SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONUP, WM_SIZE, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW,
};

use super::icons::{self, Icon};
use super::theme::{self, Theme};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};
use crate::history::{self as hist, Entry};

const WM_THUMB: u32 = WM_APP + 30;

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
    folder_btn: Rect,
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
        folder_btn: Rect::default(),
    };
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h_) = (((980.0 * sc) as i32).min(work.w * 9 / 10), ((700.0 * sc) as i32).min(work.h * 9 / 10));
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h_) / 2, w, h_);
    match super::create("SnapItHistory", WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, "Snap It History", r, None, Box::new(h)) {
        Ok(hwnd) => {
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
        52.0 * self.scale
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

    fn render(&mut self) {
        Backbuffer::ensure(&mut self.bb, self.client.0.max(1) as u32, self.client.1.max(1) as u32);
        let Some(mut bb) = self.bb.take() else { return };
        {
            let mut pm = bb.pixmap();
            self.draw(&mut pm);
        }
        self.bb = Some(bb);
    }

    fn draw(&mut self, pm: &mut snapit_gfx::tiny_skia::PixmapMut) {
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
            let bg = if self.selected == Some(i) { t.accent.with_alpha(60) } else if self.hover == Some(i) { t.hover } else { t.panel };
            if let Some(p) = rounded_rect(inner.x as f32, inner.y as f32, inner.w as f32, inner.h as f32, 8.0 * s) {
                pm.fill_path(&p, &bg.paint(), FillRule::Winding, id, None);
                if self.selected == Some(i) {
                    pm.stroke_path(&p, &t.accent.paint(), &Stroke { width: 2.0, ..Default::default() }, id, None);
                }
            }
            let (tw, th) = self.thumb_size();
            let tx = inner.x as f32 + (inner.w as f32 - tw as f32) / 2.0;
            let ty = inner.y as f32 + 8.0 * s;
            let e = &self.entries[i];
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
                None => {
                    if let Some(p) = rounded_rect(tx, ty, tw as f32, th as f32, 4.0 * s) {
                        pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
                    }
                }
            }
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = ellipsize(&name, 12.0 * s, inner.w as f32 - 16.0 * s);
            text::draw(pm, &name, inner.x as f32 + 8.0 * s, ty + th as f32 + 8.0 * s, 12.0 * s, false, t.text, None);
            let meta = format!("{} × {}  ·  {}", e.width, e.height, e.kind.replace('_', " "));
            text::draw(pm, &meta, inner.x as f32 + 8.0 * s, ty + th as f32 + 26.0 * s, 11.0 * s, false, t.subtext, None);
        }

        // Header on top of the scrolled grid.
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, 0.0, self.client.0 as f32, self.header_h()) {
            pm.fill_rect(r, &t.panel.paint(), id, None);
        }
        let title = format!("History  ·  {} captures", self.entries.len());
        text::draw(pm, &title, 16.0 * s, 14.0 * s, 17.0 * s, true, t.text, None);
        let hint = "Double-click to edit · right-click for more";
        let hl = text::layout(hint, 12.0 * s, false);
        let bw = 150.0 * s;
        self.folder_btn = Rect::new((self.client.0 as f32 - bw - 12.0 * s) as i32, (10.0 * s) as i32, bw as i32, (32.0 * s) as i32);
        text::draw(pm, hint, self.folder_btn.x as f32 - hl.width - 16.0 * s, 18.0 * s, 12.0 * s, false, t.subtext, None);
        let fb = self.folder_btn;
        if let Some(p) = rounded_rect(fb.x as f32, fb.y as f32, fb.w as f32, fb.h as f32, 6.0 * s) {
            pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
        }
        icons::draw(pm, Icon::Folder, fb.x as f32 + 8.0 * s, fb.y as f32 + 6.0 * s, 20.0 * s, t.text);
        text::draw(pm, "Open folder", fb.x as f32 + 34.0 * s, fb.y as f32 + 7.0 * s, 13.0 * s, false, t.text, None);
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

    fn context_menu(&mut self, hwnd: HWND, i: usize) {
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
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
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

impl Handler for History {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
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
                self.render();
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
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
                let h = self.index_at(x, y);
                if h != self.hover {
                    self.hover = h;
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (x, y) = super::lparam_xy(lp);
                if self.folder_btn.contains(x, y) {
                    crate::shell::open_folder(&app::settings().save_dir());
                } else {
                    self.selected = self.index_at(x, y);
                    w.invalidate();
                }
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
                    w.invalidate();
                    self.context_menu(w.hwnd, i);
                    self.request_visible(w.hwnd);
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                match wp.0 as u16 {
                    0x0D => {
                        if let Some(i) = self.selected {
                            self.open_entry(i);
                        }
                    }
                    0x2E => {
                        if let Some(i) = self.selected {
                            self.remove(i);
                            self.request_visible(w.hwnd);
                        }
                    }
                    0x1B => w.close(),
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
