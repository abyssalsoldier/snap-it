//! The full image editor.
//!
//! Annotations stay editable (select, move, resize, restyle, delete) until the
//! image is saved, copied or an effect flattens them. Crop and cut-out shift the
//! annotations rather than flattening them. Every change is undoable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use snapit_annotate::{self as ann, Bx, History, Item, Shape, Sticker, Style, Tool};
use snapit_gfx::effects::{self, BEAUTIFY_PRESETS};
use snapit_gfx::tiny_skia::{FilterQuality, Pattern, PathBuilder, Pixmap, PixmapMut, PixmapPaint, SpreadMode, Stroke, StrokeDash, Transform, Paint, FillRule};
use snapit_gfx::{blit_rect, io, pixel_at, text, PointF, Rect, Rgba};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetClientRect, LoadCursorW,
    PostMessageW, SetCursor, SetForegroundWindow, SetWindowTextW, TrackPopupMenu,
    HTCLIENT, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENWSE,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MINMAXINFO, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
    WM_CHAR, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WM_SIZE, WS_EX_APPWINDOW,
    WS_OVERLAPPEDWINDOW,
};

use super::dialogs::{self, Answer};
use super::icons::Icon;
use super::theme::{self, Theme};
use super::toolbar::{self, StickerPopup, StyleHit, StylePopup, Toolbar};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};

pub const WM_EDITOR_INSERT: u32 = WM_APP + 20;
/// Self-test: save the current image (same as the Save button).
pub const WM_EDITOR_TEST_SAVE: u32 = WM_APP + 21;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static EDITORS: RefCell<HashMap<u64, isize>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ETool {
    Ann(Tool),
    Crop,
    CutOut,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EBtn {
    Tool(ETool),
    Stickers,
    Image,
    Style,
    Undo,
    Redo,
    Effects,
    Fit,
    Copy,
    Save,
    SaveAs,
    Pin,
}

#[derive(Clone, Copy, PartialEq)]
enum Popup {
    None,
    Style,
    Stickers,
}

enum Drag {
    None,
    Pan { start: (i32, i32), off0: (f32, f32) },
    Draw { start: PointF, before: Vec<Item> },
    MoveItem { idx: usize, last: PointF, before: Vec<Item> },
    ResizeItem { idx: usize, corner: usize, before: Vec<Item> },
    Crop { a: PointF, b: PointF },
    Cut { a: PointF, b: PointF },
}

type Snapshot = (Rc<Pixmap>, Vec<Item>);

struct Editor {
    id: u64,
    theme: Theme,
    scale: f32,
    base: Rc<Pixmap>,
    items: Vec<Item>,
    history: History<Snapshot>,
    path: Option<PathBuf>,
    dirty: bool,
    comp: Option<Pixmap>,
    comp_dirty: bool,
    version: u64,
    hq: Option<(f32, u64, Pixmap)>,
    zoom: f32,
    off: (f32, f32),
    fit: bool,
    client: (i32, i32),
    bb: Option<Backbuffer>,
    top: Toolbar<EBtn>,
    palette: Toolbar<EBtn>,
    popup: Popup,
    tool: ETool,
    style: Style,
    sticker: Sticker,
    drag: Drag,
    editing: Option<usize>,
    edit_before: Option<Vec<Item>>,
    selected: Option<usize>,
    cursor: (i32, i32),
    space: bool,
    /// Highlighter colour (separate from the pen colour).
    marker: Rgba,
}

pub fn open(image: Pixmap, path: Option<PathBuf>) {
    open_with(image, path, Vec::new());
}

pub fn open_with(image: Pixmap, path: Option<PathBuf>, items: Vec<Item>) {
    let s = app::settings();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let style = Style { color: s.annotate.color, width: s.annotate.width, font_size: s.annotate.font_size, fill: false };
    let (iw, ih) = (image.width() as i32, image.height() as i32);
    let ed = Editor {
        id,
        theme: theme::current(),
        scale: 1.0,
        base: Rc::new(image),
        dirty: !items.is_empty(),
        items,
        history: History::new(),
        path,
        comp: None,
        comp_dirty: true,
        version: 0,
        hq: None,
        zoom: 1.0,
        off: (0.0, 0.0),
        fit: true,
        client: (0, 0),
        bb: None,
        top: Toolbar::new(top_buttons(), 1.0),
        palette: Toolbar::new(palette_buttons(), 1.0),
        popup: Popup::None,
        tool: ETool::Ann(Tool::Arrow),
        style,
        sticker: Sticker::Check,
        drag: Drag::None,
        editing: None,
        edit_before: None,
        selected: None,
        cursor: (0, 0),
        space: false,
        marker: s.annotate.highlight,
    };

    // Size the window to the image, within 85% of the monitor work area.
    let (cx, cy) = {
        let mut p = POINT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        }
        (p.x, p.y)
    };
    let mons = snapit_capture::monitors();
    let mon = snapit_capture::monitor_at(&mons, cx, cy).cloned();
    let (work, sc) = mon.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let chrome_w = (70.0 * sc) as i32 + 16;
    let chrome_h = (90.0 * sc) as i32 + 40;
    let w = (iw + chrome_w).clamp((900.0 * sc) as i32, work.w * 85 / 100);
    let h = (ih + chrome_h).clamp((640.0 * sc) as i32, work.h * 85 / 100);
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h);

    match super::create("SnapItEditor", WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, "Snap It Editor", r, None, Box::new(ed)) {
        Ok(hwnd) => {
            EDITORS.with(|m| m.borrow_mut().insert(id, hwnd.0 as isize));
            super::show(hwnd, true);
        }
        Err(e) => crate::log(&format!("editor: {e}")),
    }
}

pub fn open_from_file_or_clipboard() {
    if let Some(p) = dialogs::open_image(None) {
        match io::load(&p) {
            Ok(img) => open(img, Some(p)),
            Err(e) => dialogs::info(None, "Snap It", &format!("Could not open the image.\n{e}")),
        }
    }
}

/// Delivers an image to editor `id` (posted, so it never re-enters the editor's handler).
pub fn insert(id: u64, image: Pixmap) {
    let Some(h) = EDITORS.with(|m| m.borrow().get(&id).copied()) else { return };
    let raw = Box::into_raw(Box::new(image));
    unsafe {
        if PostMessageW(Some(HWND(h as *mut _)), WM_EDITOR_INSERT, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

fn palette_buttons() -> Vec<toolbar::Button<EBtn>> {
    use toolbar::{b, sep};
    let t = |x| EBtn::Tool(ETool::Ann(x));
    vec![
        b(t(Tool::Pointer), Icon::Pointer, "Select, move & resize  (V)"),
        sep(t(Tool::Rect), Icon::Rect, "Rectangle  (R)"),
        b(t(Tool::Ellipse), Icon::Ellipse, "Ellipse  (O)"),
        b(t(Tool::Line), Icon::Line, "Line  (L)"),
        b(t(Tool::Arrow), Icon::Arrow, "Arrow  (A)"),
        b(t(Tool::Pen), Icon::Pen, "Pen  (D)"),
        b(t(Tool::Highlight), Icon::Highlight, "Highlight  (H)"),
        b(t(Tool::Text), Icon::Text, "Text  (T)"),
        b(t(Tool::Balloon), Icon::Balloon, "Speech balloon  (K)"),
        b(t(Tool::Step), Icon::Step, "Step number  (N)"),
        b(t(Tool::Blur), Icon::Blur, "Blur  (B)"),
        b(t(Tool::Pixelate), Icon::Pixelate, "Pixelate  (X)"),
        b(t(Tool::Eraser), Icon::Eraser, "Smart eraser  (G)"),
        b(t(Tool::Magnify), Icon::Magnify, "Magnify  (M)"),
        b(EBtn::Stickers, Icon::Sticker, "Stickers  (I)"),
        b(EBtn::Image, Icon::Image, "Insert image…"),
        sep(EBtn::Tool(ETool::Crop), Icon::Crop, "Crop  (C)"),
        b(EBtn::Tool(ETool::CutOut), Icon::CutOut, "Cut out a strip  (U)"),
    ]
}

fn top_buttons() -> Vec<toolbar::Button<EBtn>> {
    use toolbar::{b, sep};
    vec![
        b(EBtn::Style, Icon::Fill, "Colour, width & text size"),
        sep(EBtn::Undo, Icon::Undo, "Undo  (Ctrl+Z)"),
        b(EBtn::Redo, Icon::Redo, "Redo  (Ctrl+Y)"),
        sep(EBtn::Effects, Icon::Effects, "Effects…"),
        b(EBtn::Fit, Icon::ZoomFit, "Fit to window  (Ctrl+0)"),
        sep(EBtn::Copy, Icon::Copy, "Copy  (Ctrl+C)"),
        b(EBtn::Save, Icon::Save, "Save  (Ctrl+S)"),
        b(EBtn::SaveAs, Icon::SaveAs, "Save as…  (Ctrl+Shift+S)"),
        b(EBtn::Pin, Icon::Pin, "Pin to screen  (P)"),
    ]
}

fn key_tool(k: u8) -> Option<ETool> {
    Some(match k {
        b'V' => ETool::Ann(Tool::Pointer),
        b'R' => ETool::Ann(Tool::Rect),
        b'O' => ETool::Ann(Tool::Ellipse),
        b'L' => ETool::Ann(Tool::Line),
        b'A' => ETool::Ann(Tool::Arrow),
        b'D' => ETool::Ann(Tool::Pen),
        b'H' => ETool::Ann(Tool::Highlight),
        b'T' => ETool::Ann(Tool::Text),
        b'K' => ETool::Ann(Tool::Balloon),
        b'N' => ETool::Ann(Tool::Step),
        b'B' => ETool::Ann(Tool::Blur),
        b'X' => ETool::Ann(Tool::Pixelate),
        b'G' => ETool::Ann(Tool::Eraser),
        b'M' => ETool::Ann(Tool::Magnify),
        b'C' => ETool::Crop,
        b'U' => ETool::CutOut,
        _ => return None,
    })
}

// Effects menu ids.
const FX_BORDER: usize = 1;
const FX_SHADOW: usize = 2;
const FX_ROUND: usize = 3;
const FX_BEAUTIFY: usize = 10; // +preset index
const FX_RESIZE: usize = 20; // +index into RESIZE
const FX_ROT_CW: usize = 30;
const FX_ROT_CCW: usize = 31;
const FX_FLIP_H: usize = 32;
const FX_FLIP_V: usize = 33;
const RESIZE: [(f32, &str); 5] = [(0.25, "25%"), (0.5, "50%"), (0.75, "75%"), (1.5, "150%"), (2.0, "200%")];
const IMG_FILE: usize = 1;
const IMG_CLIP: usize = 2;
const IMG_SCREEN: usize = 3;

impl Editor {
    fn s(&self) -> f32 {
        self.scale
    }

    fn top_h(&self) -> i32 {
        (46.0 * self.s()) as i32
    }

    fn status_h(&self) -> i32 {
        (24.0 * self.s()) as i32
    }

    fn canvas(&self) -> Rect {
        let left = self.palette.rect().right() + (6.0 * self.s()) as i32;
        Rect::new(left, self.top_h(), (self.client.0 - left).max(1), (self.client.1 - self.top_h() - self.status_h()).max(1))
    }

    fn layout(&mut self) {
        let s = self.s();
        self.top.scale = s;
        self.top.x = 8.0 * s;
        self.top.y = 3.0 * s;
        // Shrink the tool palette if the window is too short for it.
        self.palette.vertical = true;
        self.palette.scale = s;
        let avail = (self.client.1 - self.top_h() - self.status_h()) as f32 - 8.0 * s;
        let need = self.palette.size().1;
        if need > avail && avail > 100.0 {
            self.palette.scale = s * avail / need;
        }
        self.palette.x = 6.0 * s;
        self.palette.y = self.top_h() as f32 + 2.0 * s;
        if self.fit {
            self.fit_view();
        }
    }

    fn fit_view(&mut self) {
        let c = self.canvas();
        let (iw, ih) = (self.base.width() as f32, self.base.height() as f32);
        let pad = 24.0 * self.s();
        self.zoom = ((c.w as f32 - pad) / iw).min((c.h as f32 - pad) / ih).clamp(0.02, 1.0);
        self.center();
    }

    fn center(&mut self) {
        let c = self.canvas();
        let (iw, ih) = (self.base.width() as f32 * self.zoom, self.base.height() as f32 * self.zoom);
        self.off = (((c.w as f32 - iw) / 2.0).round(), ((c.h as f32 - ih) / 2.0).round());
    }

    fn zoom_at(&mut self, factor: f32, x: i32, y: i32) {
        let c = self.canvas();
        let p = self.to_img(x, y);
        let z = (self.zoom * factor).clamp(0.02, 32.0);
        self.zoom = z;
        self.off = (x as f32 - c.x as f32 - p.x * z, y as f32 - c.y as f32 - p.y * z);
        self.fit = false;
    }

    fn to_img(&self, x: i32, y: i32) -> PointF {
        let c = self.canvas();
        PointF::new((x as f32 - c.x as f32 - self.off.0) / self.zoom, (y as f32 - c.y as f32 - self.off.1) / self.zoom)
    }

    fn to_screen(&self, p: PointF) -> (f32, f32) {
        let c = self.canvas();
        (c.x as f32 + self.off.0 + p.x * self.zoom, c.y as f32 + self.off.1 + p.y * self.zoom)
    }

    fn composite(&mut self) -> &Pixmap {
        if self.comp_dirty || self.comp.is_none() {
            let mut c = (*self.base).clone();
            ann::render(&mut c.as_mut(), &self.items, 0.0, 0.0);
            self.comp = Some(c);
            self.comp_dirty = false;
            self.version += 1;
        }
        self.comp.as_ref().expect("just built")
    }

    fn changed(&mut self) {
        self.comp_dirty = true;
        self.dirty = true;
    }

    fn undo_state(&self) -> Snapshot {
        (self.base.clone(), self.items.clone())
    }

    fn push_items(&mut self, before: Vec<Item>) {
        self.history.push((self.base.clone(), before));
    }

    fn update_title(&self, hwnd: HWND) {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let t = format!("{}{} — Snap It Editor", name, if self.dirty { " •" } else { "" });
        unsafe {
            let _ = SetWindowTextW(hwnd, &HSTRING::from(t));
        }
    }

    // ---------- editing operations ----------

    fn commit_text(&mut self) {
        let Some(i) = self.editing.take() else { return };
        let before = self.edit_before.take();
        if i < self.items.len() {
            let empty = self.items[i].text_mut().is_some_and(|t| t.trim().is_empty());
            if empty && matches!(self.items[i].shape, Shape::Text { .. }) {
                self.items.remove(i);
            } else if let Some(b) = before {
                self.push_items(b);
                self.dirty = true;
            }
        }
        self.comp_dirty = true;
    }

    fn undo(&mut self) {
        self.commit_text();
        if let Some((b, i)) = self.history.undo(self.undo_state()) {
            self.base = b;
            self.items = i;
            self.selected = None;
            self.changed();
            self.layout();
        }
    }

    fn redo(&mut self) {
        if let Some((b, i)) = self.history.redo(self.undo_state()) {
            self.base = b;
            self.items = i;
            self.selected = None;
            self.changed();
            self.layout();
        }
    }

    /// Replaces the base image (crop, effects…), recording undo.
    fn set_base(&mut self, img: Pixmap, items: Vec<Item>) {
        self.history.push(self.undo_state());
        self.base = Rc::new(img);
        self.items = items;
        self.selected = None;
        self.changed();
        self.fit = true;
        self.layout();
    }

    fn flattened(&mut self) -> Pixmap {
        self.commit_text();
        self.composite().clone()
    }

    fn apply_crop(&mut self, a: PointF, b: PointF) {
        let r = Bx::new(a, b).to_rect().intersect(Rect::new(0, 0, self.base.width() as i32, self.base.height() as i32));
        let Some(r) = r.filter(|r| r.w > 2 && r.h > 2) else { return };
        if let Some(img) = snapit_gfx::crop(&self.base, r) {
            let mut items = self.items.clone();
            items.iter_mut().for_each(|i| i.translate(-r.x as f32, -r.y as f32));
            self.set_base(img, items);
        }
    }

    fn apply_cut(&mut self, a: PointF, b: PointF) {
        let vertical_band = (b.x - a.x).abs() >= (b.y - a.y).abs();
        let (lo, hi) = if vertical_band { (a.x.min(b.x), a.x.max(b.x)) } else { (a.y.min(b.y), a.y.max(b.y)) };
        let (lo, hi) = (lo.max(0.0) as u32, hi.max(0.0) as u32);
        if hi <= lo + 1 {
            return;
        }
        if let Some(img) = effects::cut_out(&self.base, lo, hi, vertical_band) {
            let cut = (hi - lo) as f32;
            let mut items = self.items.clone();
            for it in &mut items {
                let bnd = it.bounds();
                let start = if vertical_band { bnd.x0 } else { bnd.y0 };
                if start >= hi as f32 {
                    if vertical_band {
                        it.translate(-cut, 0.0)
                    } else {
                        it.translate(0.0, -cut)
                    }
                }
            }
            self.set_base(img, items);
        }
    }

    fn effect(&mut self, id: usize) {
        let flat = self.flattened();
        let s = app::settings();
        let out = match id {
            FX_BORDER => effects::border(&flat, (self.style.width * 2.0).max(2.0) as u32, self.style.color),
            FX_SHADOW => effects::drop_shadow(&flat, 12),
            FX_ROUND => effects::round_corners(&flat, 16.0),
            FX_ROT_CW => effects::rotate90(&flat, true),
            FX_ROT_CCW => effects::rotate90(&flat, false),
            FX_FLIP_H => Some(effects::flip(&flat, true)),
            FX_FLIP_V => Some(effects::flip(&flat, false)),
            i if (FX_BEAUTIFY..FX_BEAUTIFY + BEAUTIFY_PRESETS.len()).contains(&i) => {
                let mut b = BEAUTIFY_PRESETS[i - FX_BEAUTIFY];
                b.padding = s.beautify.padding;
                b.radius = s.beautify.radius;
                b.shadow = s.beautify.shadow;
                effects::beautify(&flat, &b)
            }
            i if (FX_RESIZE..FX_RESIZE + RESIZE.len()).contains(&i) => {
                let f = RESIZE[i - FX_RESIZE].0;
                let (w, h) = ((flat.width() as f32 * f).round() as u32, (flat.height() as f32 * f).round() as u32);
                effects::resize(&flat, w.max(1), h.max(1))
            }
            _ => None,
        };
        if let Some(img) = out {
            self.set_base(img, Vec::new());
        }
    }

    fn insert_image(&mut self, img: Pixmap) {
        self.commit_text();
        let (bw, bh) = (self.base.width() as f32, self.base.height() as f32);
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let k = (bw * 0.6 / iw).min(bh * 0.6 / ih).min(1.0);
        let (w, h) = (iw * k, ih * k);
        let c = self.canvas();
        let mid = self.to_img(c.x + c.w / 2, c.y + c.h / 2);
        let (cx, cy) = (mid.x.clamp(w / 2.0, (bw - w / 2.0).max(w / 2.0)), mid.y.clamp(h / 2.0, (bh - h / 2.0).max(h / 2.0)));
        let r = Bx { x0: cx - w / 2.0, y0: cy - h / 2.0, x1: cx + w / 2.0, y1: cy + h / 2.0 };
        let before = self.items.clone();
        self.items.push(Item { shape: Shape::Image { r, img: Arc::new(img) }, style: self.style });
        self.push_items(before);
        self.selected = Some(self.items.len() - 1);
        self.tool = ETool::Ann(Tool::Pointer);
        self.changed();
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        let img = self.flattened();
        let s = app::settings();
        let fmt = io::Format::from_path(&path).unwrap_or(s.format);
        match io::save(&img, &path, fmt, s.jpeg_quality) {
            Ok(()) => {
                let e = crate::history::Entry::new(path.clone(), img.width(), img.height(), "editor");
                let _ = crate::history::append(&crate::paths::history_file(), &e);
                app::with(|st| st.last_saved = Some(path.clone()));
                self.path = Some(path);
                self.dirty = false;
                true
            }
            Err(e) => {
                dialogs::info(None, "Snap It", &format!("Could not save.\n{e}"));
                false
            }
        }
    }

    fn save(&mut self, hwnd: HWND) -> bool {
        if let Some(p) = self.path.clone().filter(|p| io::Format::from_path(p).is_some()) {
            return self.save_to(p);
        }
        let s = app::settings();
        let img = self.composite();
        let name = crate::naming::expand(
            &s.filename_pattern,
            &crate::naming::Vars { stamp: crate::naming::Stamp::now(), width: img.width(), height: img.height(), title: "Edited" },
        );
        let path = crate::naming::unique_path(&s.save_dir(), &name, s.format.ext());
        let ok = self.save_to(path.clone());
        if ok {
            app::post(Command::Notify { title: "Saved".into(), body: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() });
        }
        self.update_title(hwnd);
        ok
    }

    fn save_as(&mut self, hwnd: HWND) -> bool {
        let s = app::settings();
        let suggested = self.path.clone().unwrap_or_else(|| s.save_dir().join(format!("Snap.{}", s.format.ext())));
        let ok = match dialogs::save_image(Some(hwnd), &suggested) {
            Some(p) => self.save_to(p),
            None => false,
        };
        self.update_title(hwnd);
        ok
    }

    fn popup_menu(&self, hwnd: HWND, at: Rect, build: impl FnOnce(windows::Win32::UI::WindowsAndMessaging::HMENU)) -> usize {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return 0 };
            build(menu);
            let mut p = POINT { x: at.x, y: at.bottom() };
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
            let r = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, hwnd, None);
            let _ = DestroyMenu(menu);
            r.0 as usize
        }
    }

    fn effects_menu(&mut self, hwnd: HWND) {
        let at = self.top.button_bounds(EBtn::Effects).unwrap_or_default();
        let add = |m, id: usize, s: &str| unsafe {
            let _ = AppendMenuW(m, MF_STRING, id, &HSTRING::from(s));
        };
        let id = self.popup_menu(hwnd, at, |m| unsafe {
            if let Ok(b) = CreatePopupMenu() {
                for (i, name) in ["Indigo → pink", "Teal → blue", "Sunset", "Graphite"].iter().enumerate() {
                    add(b, FX_BEAUTIFY + i, name);
                }
                let _ = AppendMenuW(m, MF_POPUP, b.0 as usize, &HSTRING::from("Beautify"));
            }
            add(m, FX_BORDER, "Add border (current colour)");
            add(m, FX_SHADOW, "Drop shadow");
            add(m, FX_ROUND, "Rounded corners");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            if let Ok(r) = CreatePopupMenu() {
                for (i, (_, name)) in RESIZE.iter().enumerate() {
                    add(r, FX_RESIZE + i, name);
                }
                let _ = AppendMenuW(m, MF_POPUP, r.0 as usize, &HSTRING::from("Resize"));
            }
            add(m, FX_ROT_CW, "Rotate right");
            add(m, FX_ROT_CCW, "Rotate left");
            add(m, FX_FLIP_H, "Flip horizontal");
            add(m, FX_FLIP_V, "Flip vertical");
        });
        if id != 0 {
            self.effect(id);
        }
    }

    fn image_menu(&mut self, hwnd: HWND) {
        let at = self.palette.button_bounds(EBtn::Image).unwrap_or_default();
        let id = self.popup_menu(hwnd, Rect::new(at.right(), at.y - at.h, 0, at.h), |m| unsafe {
            let _ = AppendMenuW(m, MF_STRING, IMG_FILE, &HSTRING::from("From file…"));
            let _ = AppendMenuW(m, MF_STRING, IMG_CLIP, &HSTRING::from("From clipboard"));
            let _ = AppendMenuW(m, MF_STRING, IMG_SCREEN, &HSTRING::from("From screen…"));
        });
        match id {
            IMG_FILE => {
                if let Some(p) = dialogs::open_image(Some(hwnd)) {
                    match io::load(&p) {
                        Ok(img) => self.insert_image(img),
                        Err(e) => dialogs::info(Some(hwnd), "Snap It", &e),
                    }
                }
            }
            IMG_CLIP => match crate::clipboard::get_image() {
                Some(img) => self.insert_image(img),
                None => dialogs::info(Some(hwnd), "Snap It", "The clipboard doesn't contain an image."),
            },
            IMG_SCREEN => crate::ui::overlay::open(crate::ui::overlay::Mode::PickForEditor(self.id)),
            _ => {}
        }
    }

    fn on_button(&mut self, w: &mut Win, b: EBtn) {
        match b {
            EBtn::Tool(t) => {
                self.commit_text();
                self.tool = t;
                self.selected = None;
                self.popup = Popup::None;
            }
            EBtn::Stickers => {
                self.popup = if self.popup == Popup::Stickers { Popup::None } else { Popup::Stickers };
                self.tool = ETool::Ann(Tool::Sticker(self.sticker));
            }
            EBtn::Image => self.image_menu(w.hwnd),
            EBtn::Style => self.popup = if self.popup == Popup::Style { Popup::None } else { Popup::Style },
            EBtn::Undo => self.undo(),
            EBtn::Redo => self.redo(),
            EBtn::Effects => self.effects_menu(w.hwnd),
            EBtn::Fit => {
                self.fit = true;
                self.fit_view();
            }
            EBtn::Copy => {
                let img = self.flattened();
                match crate::clipboard::set_image(&img) {
                    Ok(()) => app::post(Command::Notify { title: "Copied to clipboard".into(), body: format!("{} × {}", img.width(), img.height()) }),
                    Err(e) => dialogs::info(Some(w.hwnd), "Snap It", &e),
                }
            }
            EBtn::Save => {
                self.save(w.hwnd);
            }
            EBtn::SaveAs => {
                self.save_as(w.hwnd);
            }
            EBtn::Pin => {
                let img = self.flattened();
                app::post(Command::Pin { image: img, at: None });
            }
        }
        self.update_title(w.hwnd);
    }

    fn style_popup(&self) -> StylePopup {
        let r = self.top.button_bounds(EBtn::Style).unwrap_or_default();
        StylePopup { x: r.x as f32, y: self.top_h() as f32 + 2.0, scale: self.s(), palette: app::settings().annotate.palette.clone() }
    }

    fn sticker_popup(&self) -> StickerPopup {
        let r = self.palette.button_bounds(EBtn::Stickers).unwrap_or_default();
        StickerPopup { x: r.right() as f32 + 8.0, y: r.y as f32, scale: self.s() }
    }

    fn popup_rect(&self) -> Option<Rect> {
        match self.popup {
            Popup::None => None,
            Popup::Style => Some(self.style_popup().rect()),
            Popup::Stickers => Some(self.sticker_popup().rect()),
        }
    }

    /// True when colour changes should go to the highlighter's own colour.
    fn marker_context(&self) -> bool {
        self.tool == ETool::Ann(Tool::Highlight) || self.selected.and_then(|i| self.items.get(i)).is_some_and(|it| matches!(it.shape, Shape::Highlight(_)))
    }

    fn shown_color(&self) -> Rgba {
        if self.marker_context() { self.marker } else { self.style.color }
    }

    fn apply_style(&mut self, h: StyleHit) {
        let marker = self.marker_context();
        match h {
            StyleHit::Color(c) if marker => self.marker = c,
            StyleHit::Color(c) => self.style.color = c,
            StyleHit::Width(v) => self.style.width = v,
            StyleHit::FontSize(v) => self.style.font_size = v,
            StyleHit::ToggleFill => self.style.fill = !self.style.fill,
        }
        let st = self.style;
        let mk = self.marker;
        app::update_settings(|s| {
            s.annotate.color = st.color;
            s.annotate.highlight = mk;
            s.annotate.width = st.width;
            s.annotate.font_size = st.font_size;
        });
        if let Some(i) = self.editing.or(self.selected).filter(|&i| i < self.items.len()) {
            let before = self.items.clone();
            let it = &mut self.items[i];
            let fill = if h == StyleHit::ToggleFill { !it.style.fill } else { it.style.fill };
            let color = if matches!(it.shape, Shape::Highlight(_)) { mk } else { st.color };
            it.style = Style { fill, color, ..st };
            if let (StyleHit::ToggleFill, Shape::Text { background, .. }) = (h, &mut it.shape) {
                *background = !*background;
            }
            if self.editing.is_none() {
                self.push_items(before);
            }
            self.changed();
        }
    }

    /// Corner handles (screen space) of the selected box-shaped item.
    fn item_handles(&self) -> Option<[(f32, f32); 4]> {
        let b = self.items.get(self.selected?)?.boxed()?;
        let a = self.to_screen(PointF::new(b.x0, b.y0));
        let c = self.to_screen(PointF::new(b.x1, b.y1));
        Some([(a.0, a.1), (c.0, a.1), (c.0, c.1), (a.0, c.1)])
    }

    fn handle_at(&self, x: i32, y: i32) -> Option<usize> {
        let tol = 7.0 * self.s();
        self.item_handles()?.iter().position(|(hx, hy)| (x as f32 - hx).abs() <= tol && (y as f32 - hy).abs() <= tol)
    }

    // ---------- input ----------

    fn mouse_down(&mut self, w: &mut Win, x: i32, y: i32) {
        if let Some(pr) = self.popup_rect() {
            if pr.contains(x, y) {
                match self.popup {
                    Popup::Style => {
                        if let Some(h) = self.style_popup().hit(x, y) {
                            self.apply_style(h);
                        }
                    }
                    Popup::Stickers => {
                        if let Some(k) = self.sticker_popup().hit(x, y) {
                            self.sticker = k;
                            self.tool = ETool::Ann(Tool::Sticker(k));
                            self.popup = Popup::None;
                        }
                    }
                    Popup::None => {}
                }
                return;
            }
            self.popup = Popup::None;
        }
        if let Some(b) = self.top.hit(x, y).or_else(|| self.palette.hit(x, y)) {
            return self.on_button(w, b);
        }
        if !self.canvas().contains(x, y) {
            return;
        }
        unsafe {
            SetCapture(w.hwnd);
        }
        if self.space {
            self.drag = Drag::Pan { start: (x, y), off0: self.off };
            return;
        }
        self.commit_text();
        let p = self.to_img(x, y);
        match self.tool {
            ETool::Crop => self.drag = Drag::Crop { a: p, b: p },
            ETool::CutOut => self.drag = Drag::Cut { a: p, b: p },
            ETool::Ann(Tool::Pointer) => {
                if let (Some(c), Some(i)) = (self.handle_at(x, y), self.selected) {
                    self.drag = Drag::ResizeItem { idx: i, corner: c, before: self.items.clone() };
                } else if let Some(i) = ann::hit_test(&self.items, p) {
                    self.selected = Some(i);
                    self.drag = Drag::MoveItem { idx: i, last: p, before: self.items.clone() };
                } else {
                    self.selected = None;
                    self.drag = Drag::Pan { start: (x, y), off0: self.off };
                }
            }
            ETool::Ann(tool) => {
                let sample = pixel_at(&self.base, p.x as i32, p.y as i32).unwrap_or(Rgba::WHITE);
                let fill = self.style.fill && matches!(tool, Tool::Rect | Tool::Ellipse | Tool::Text);
                let color = if tool == Tool::Highlight { self.marker } else { self.style.color };
                if let Some(item) = Item::begin(tool, p, Style { fill, color, ..self.style }, &self.items, sample) {
                    let before = self.items.clone();
                    self.items.push(item);
                    self.selected = None;
                    self.comp_dirty = true;
                    match tool {
                        Tool::Text => {
                            self.editing = Some(self.items.len() - 1);
                            self.edit_before = Some(before);
                        }
                        Tool::Step => {
                            self.push_items(before);
                            self.dirty = true;
                        }
                        _ => self.drag = Drag::Draw { start: p, before },
                    }
                }
            }
        }
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
        self.top.set_hover(x, y);
        self.palette.set_hover(x, y);
        let p = self.to_img(x, y);
        let shift = super::shift();
        match &mut self.drag {
            Drag::None => {}
            Drag::Pan { start, off0 } => {
                self.off = (off0.0 + (x - start.0) as f32, off0.1 + (y - start.1) as f32);
                self.fit = false;
            }
            Drag::Draw { start, .. } => {
                let s = *start;
                if let Some(it) = self.items.last_mut() {
                    it.drag(s, p, shift);
                    self.comp_dirty = true;
                }
            }
            Drag::MoveItem { idx, last, .. } => {
                if let Some(it) = self.items.get_mut(*idx) {
                    it.translate(p.x - last.x, p.y - last.y);
                    *last = p;
                    self.comp_dirty = true;
                }
            }
            Drag::ResizeItem { idx, corner, .. } => {
                let c = *corner;
                if let Some(b) = self.items.get_mut(*idx).and_then(|i| i.boxed_mut()) {
                    let n = b.norm();
                    let (mut x0, mut y0, mut x1, mut y1) = (n.x0, n.y0, n.x1, n.y1);
                    match c {
                        0 => (x0, y0) = (p.x, p.y),
                        1 => (x1, y0) = (p.x, p.y),
                        2 => (x1, y1) = (p.x, p.y),
                        _ => (x0, y1) = (p.x, p.y),
                    }
                    if shift {
                        // Keep aspect ratio.
                        let ar = n.w() / n.h().max(1.0);
                        let w = (x1 - x0).abs().max(4.0);
                        let h = w / ar;
                        if c == 0 || c == 1 {
                            y0 = y1 - h * (y1 - y0).signum().max(1.0);
                        } else {
                            y1 = y0 + h;
                        }
                    }
                    *b = Bx { x0, y0, x1, y1 };
                    self.comp_dirty = true;
                }
            }
            Drag::Crop { b, .. } | Drag::Cut { b, .. } => *b = p,
        }
    }

    fn mouse_up(&mut self, _w: &mut Win) {
        unsafe {
            let _ = ReleaseCapture();
        }
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Draw { before, .. } => {
                let last = self.items.last().cloned();
                match last {
                    Some(it) if matches!(it.shape, Shape::Balloon { .. }) => {
                        if it.bounds().w() < 12.0 {
                            let p = match it.shape {
                                Shape::Balloon { r, .. } => PointF::new(r.x0, r.y0),
                                _ => PointF::default(),
                            };
                            if let Some(l) = self.items.last_mut() {
                                l.drag(p, p.offset(180.0, 60.0), false);
                            }
                        }
                        self.editing = Some(self.items.len() - 1);
                        self.edit_before = Some(before);
                    }
                    Some(it) if it.is_degenerate() => {
                        self.items.pop();
                    }
                    Some(_) => {
                        self.push_items(before);
                        self.dirty = true;
                    }
                    None => {}
                }
                self.comp_dirty = true;
            }
            Drag::MoveItem { before, .. } | Drag::ResizeItem { before, .. } => {
                if before != self.items {
                    self.push_items(before);
                    self.dirty = true;
                }
            }
            Drag::Crop { a, b } => self.apply_crop(a, b),
            Drag::Cut { a, b } => self.apply_cut(a, b),
            _ => {}
        }
        self.hq = None; // recompute the high-quality view now that interaction stopped
    }

    fn key_down(&mut self, w: &mut Win, vk: u16) {
        let ctrl = super::ctrl();
        let shift = super::shift();
        if self.editing.is_some() {
            if vk == 0x1B {
                self.commit_text();
            } else if vk == 0x56 && ctrl {
                if let (Some(i), Some(t)) = (self.editing, crate::clipboard::get_text()) {
                    if let Some(s) = self.items.get_mut(i).and_then(|it| it.text_mut()) {
                        s.push_str(&t.replace("\r\n", "\n"));
                        self.comp_dirty = true;
                    }
                }
            }
            return;
        }
        match vk {
            0x20 => self.space = true,
            0x1B => {
                if self.popup != Popup::None {
                    self.popup = Popup::None;
                } else {
                    self.selected = None;
                }
            }
            0x5A if ctrl => {
                if shift {
                    self.redo()
                } else {
                    self.undo()
                }
            }
            0x59 if ctrl => self.redo(),
            0x43 if ctrl => self.on_button(w, EBtn::Copy),
            0x53 if ctrl => self.on_button(w, if shift { EBtn::SaveAs } else { EBtn::Save }),
            0x56 if ctrl => {
                if let Some(img) = crate::clipboard::get_image() {
                    self.insert_image(img);
                }
            }
            0x30 if ctrl => {
                self.fit = true;
                self.fit_view();
            }
            0x31 if ctrl => {
                self.zoom = 1.0;
                self.fit = false;
                self.center();
            }
            0xBB | 0x6B if ctrl => {
                let c = self.canvas();
                self.zoom_at(1.25, c.x + c.w / 2, c.y + c.h / 2);
            }
            0xBD | 0x6D if ctrl => {
                let c = self.canvas();
                self.zoom_at(0.8, c.x + c.w / 2, c.y + c.h / 2);
            }
            0x2E => {
                if let Some(i) = self.selected.take().filter(|&i| i < self.items.len()) {
                    let before = self.items.clone();
                    self.items.remove(i);
                    self.push_items(before);
                    self.changed();
                }
            }
            _ if !ctrl => {
                let k = vk as u8;
                if k == b'P' {
                    self.on_button(w, EBtn::Pin);
                } else if k == b'I' {
                    self.tool = ETool::Ann(Tool::Sticker(self.sticker));
                    self.popup = Popup::Stickers;
                } else if let Some(t) = key_tool(k) {
                    self.tool = t;
                    self.selected = None;
                }
            }
            _ => {}
        }
        self.update_title(w.hwnd);
    }

    fn char_input(&mut self, c: u32) {
        let Some(i) = self.editing else { return };
        let Some(s) = self.items.get_mut(i).and_then(|it| it.text_mut()) else { return };
        match c {
            0x08 => {
                s.pop();
            }
            0x0D => {
                if super::shift() {
                    s.push('\n');
                } else {
                    return self.commit_text();
                }
            }
            c if c >= 0x20 => {
                if let Some(ch) = char::from_u32(c) {
                    s.push(ch);
                }
            }
            _ => return,
        }
        self.comp_dirty = true;
    }

    fn cursor_shape(&self) -> windows::core::PCWSTR {
        let (x, y) = self.cursor;
        if !self.canvas().contains(x, y) || self.popup_rect().is_some_and(|r| r.contains(x, y)) {
            return IDC_ARROW;
        }
        if self.space || matches!(self.drag, Drag::Pan { .. }) {
            return IDC_HAND;
        }
        match self.tool {
            ETool::Ann(Tool::Pointer) => {
                if let Some(h) = self.handle_at(x, y) {
                    return if h % 2 == 0 { IDC_SIZENWSE } else { IDC_SIZENESW };
                }
                if ann::hit_test(&self.items, self.to_img(x, y)).is_some() {
                    IDC_SIZEALL
                } else {
                    IDC_HAND
                }
            }
            ETool::Ann(Tool::Text) => IDC_IBEAM,
            _ => IDC_CROSS,
        }
    }

    // ---------- drawing ----------

    fn render(&mut self) {
        let (cw, ch) = (self.client.0.max(1) as u32, self.client.1.max(1) as u32);
        Backbuffer::ensure(&mut self.bb, cw, ch);
        let interacting = !matches!(self.drag, Drag::None);
        let zoom = self.zoom;
        let _ = self.composite();
        // High-quality downscaled view when zoomed out and idle.
        if zoom < 0.999 && !interacting && self.hq.as_ref().is_none_or(|(z, v, _)| *z != zoom || *v != self.version) {
            let c = self.comp.as_ref().expect("built");
            let (w, h) = ((c.width() as f32 * zoom).round().max(1.0) as u32, (c.height() as f32 * zoom).round().max(1.0) as u32);
            self.hq = effects::resize(c, w, h).map(|p| (zoom, self.version, p));
        }
        let Some(mut bb) = self.bb.take() else { return };
        {
            let mut pm = bb.pixmap();
            self.draw(&mut pm, interacting);
        }
        self.bb = Some(bb);
    }

    fn draw(&self, pm: &mut PixmapMut, interacting: bool) {
        let t = &self.theme;
        let s = self.s();
        let id = Transform::identity();
        pm.fill(t.bg.color());
        let c = self.canvas();
        let comp = self.comp.as_ref().expect("composite built");
        let (iw, ih) = (comp.width() as f32 * self.zoom, comp.height() as f32 * self.zoom);
        let (ix, iy) = (c.x as f32 + self.off.0, c.y as f32 + self.off.1);

        // Checkerboard under the image (shows transparency).
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(ix, iy, iw, ih) {
            if let Some(mut chk) = Pixmap::new(16, 16) {
                chk.fill(Rgba::rgb(255, 255, 255).color());
                for (x, y) in [(0, 0), (8, 8)] {
                    if let Some(sq) = snapit_gfx::tiny_skia::Rect::from_xywh(x as f32, y as f32, 8.0, 8.0) {
                        chk.fill_rect(sq, &Rgba::rgb(222, 222, 226).paint(), id, None);
                    }
                }
                let paint = Paint { shader: Pattern::new(chk.as_ref(), SpreadMode::Repeat, FilterQuality::Nearest, 1.0, Transform::from_translate(ix, iy)), ..Default::default() };
                pm.fill_rect(r, &paint, id, None);
            }
            // Image.
            if (self.zoom - 1.0).abs() < 1e-3 && ix.fract() == 0.0 && iy.fract() == 0.0 {
                // 1:1 fast path; clip to the canvas.
                if let Some(vis) = Rect::new(ix as i32, iy as i32, comp.width() as i32, comp.height() as i32).intersect(c) {
                    blit_rect(pm, comp, vis.offset(-(ix as i32), -(iy as i32)), vis.x, vis.y);
                }
            } else if let (false, Some((_, v, hq))) = (interacting, &self.hq) {
                if *v == self.version {
                    pm.draw_pixmap(ix.round() as i32, iy.round() as i32, hq.as_ref(), &PixmapPaint::default(), id, None);
                }
            } else {
                let q = if self.zoom >= 2.0 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
                let paint = PixmapPaint { quality: q, ..Default::default() };
                pm.draw_pixmap(0, 0, comp.as_ref(), &paint, Transform::from_row(self.zoom, 0.0, 0.0, self.zoom, ix, iy), None);
            }
        }

        // Selection outline and resize handles.
        if let Some(it) = self.selected.and_then(|i| self.items.get(i)) {
            let b = it.bounds();
            let (x0, y0) = self.to_screen(PointF::new(b.x0, b.y0));
            let (x1, y1) = self.to_screen(PointF::new(b.x1, b.y1));
            if let Some(p) = snapit_gfx::tiny_skia::Rect::from_ltrb(x0, y0, x1, y1).map(PathBuilder::from_rect) {
                let dash = Stroke { width: 1.0 * s, dash: StrokeDash::new(vec![4.0 * s, 3.0 * s], 0.0), ..Default::default() };
                pm.stroke_path(&p, &t.accent.paint(), &dash, id, None);
            }
            if let Some(hs) = self.item_handles() {
                for (hx, hy) in hs {
                    let r = 4.5 * s;
                    if let Some(p) = PathBuilder::from_circle(hx, hy, r) {
                        pm.fill_path(&p, &Rgba::WHITE.paint(), FillRule::Winding, id, None);
                        pm.stroke_path(&p, &t.accent.paint(), &Stroke { width: 1.5, ..Default::default() }, id, None);
                    }
                }
            }
        }
        if let Some(it) = self.editing.and_then(|i| self.items.get(i)) {
            let b = it.bounds();
            let (x0, y0) = self.to_screen(PointF::new(b.x0, b.y0));
            let (x1, y1) = self.to_screen(PointF::new(b.x1, b.y1));
            if let Some(p) = snapit_gfx::tiny_skia::Rect::from_ltrb(x0, y0, x1, y1).map(PathBuilder::from_rect) {
                let dash = Stroke { width: 1.0, dash: StrokeDash::new(vec![3.0, 3.0], 0.0), ..Default::default() };
                pm.stroke_path(&p, &t.accent.paint(), &dash, id, None);
            }
            // Caret at the end of the text.
            let l = text::layout(it.text_mut_clone().as_deref().unwrap_or(""), it.style.font_size, false);
            let (tx, ty) = match &it.shape {
                Shape::Text { at, .. } => (at.x, at.y),
                Shape::Balloon { r, .. } => {
                    let n = r.norm();
                    (n.x0 + it.style.font_size * 0.5, n.y0 + it.style.font_size * 0.5)
                }
                _ => (0.0, 0.0),
            };
            let line_y = ty + (l.lines.max(1) - 1) as f32 * l.line_height;
            let (sx, sy) = self.to_screen(PointF::new(tx + l.last_line_width + 1.0, line_y));
            let mut pb = PathBuilder::new();
            pb.move_to(sx, sy);
            pb.line_to(sx, sy + l.line_height * self.zoom);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &t.text.paint(), &Stroke { width: 1.5 * s, ..Default::default() }, id, None);
            }
        }
        // Crop / cut-out previews.
        match &self.drag {
            Drag::Crop { a, b } => {
                let (x0, y0) = self.to_screen(*a);
                let (x1, y1) = self.to_screen(*b);
                if let Some(r) = snapit_gfx::tiny_skia::Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)) {
                    let shade = Rgba::new(0, 0, 0, 120).paint();
                    for sh in [
                        snapit_gfx::tiny_skia::Rect::from_ltrb(ix, iy, ix + iw, r.top()),
                        snapit_gfx::tiny_skia::Rect::from_ltrb(ix, r.bottom(), ix + iw, iy + ih),
                        snapit_gfx::tiny_skia::Rect::from_ltrb(ix, r.top(), r.left(), r.bottom()),
                        snapit_gfx::tiny_skia::Rect::from_ltrb(r.right(), r.top(), ix + iw, r.bottom()),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        pm.fill_rect(sh, &shade, id, None);
                    }
                    pm.stroke_path(&PathBuilder::from_rect(r), &Rgba::WHITE.paint(), &Stroke { width: 1.5, ..Default::default() }, id, None);
                }
            }
            Drag::Cut { a, b } => {
                let vertical = (b.x - a.x).abs() >= (b.y - a.y).abs();
                let (x0, y0) = self.to_screen(*a);
                let (x1, y1) = self.to_screen(*b);
                let r = if vertical {
                    snapit_gfx::tiny_skia::Rect::from_ltrb(x0.min(x1), iy, x0.max(x1), iy + ih)
                } else {
                    snapit_gfx::tiny_skia::Rect::from_ltrb(ix, y0.min(y1), ix + iw, y0.max(y1))
                };
                if let Some(r) = r {
                    pm.fill_rect(r, &Rgba::new(239, 68, 68, 90).paint(), id, None);
                }
            }
            _ => {}
        }

        // Chrome: top bar, palette, status bar.
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, 0.0, self.client.0 as f32, self.top_h() as f32) {
            pm.fill_rect(r, &t.panel.paint(), id, None);
        }
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, (self.client.1 - self.status_h()) as f32, self.client.0 as f32, self.status_h() as f32) {
            pm.fill_rect(r, &t.panel.paint(), id, None);
        }
        let tool = self.tool;
        let popup = self.popup;
        let active = |b: EBtn| match b {
            EBtn::Tool(x) => x == tool,
            EBtn::Stickers => matches!(tool, ETool::Ann(Tool::Sticker(_))) || popup == Popup::Stickers,
            EBtn::Style => popup == Popup::Style,
            _ => false,
        };
        self.palette.draw(pm, t, active, None);
        self.top.draw(pm, t, active, Some((EBtn::Style, self.shown_color(), self.style.width)));

        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let status = format!(
            "{} × {}    {:.0}%    {}{}    ·  Ctrl+wheel zoom  ·  Space+drag pan  ·  Shift constrains",
            self.base.width(),
            self.base.height(),
            self.zoom * 100.0,
            name,
            if self.dirty { " •" } else { "" }
        );
        let ts = 12.0 * s;
        let l = text::layout(&status, ts, false);
        text::draw(pm, &status, 10.0 * s, (self.client.1 - self.status_h()) as f32 + (self.status_h() as f32 - l.height) / 2.0, ts, false, t.subtext, None);

        match self.popup {
            Popup::Style => self.style_popup().draw(pm, t, self.shown_color(), self.style.width, self.style.font_size, self.style.fill),
            Popup::Stickers => self.sticker_popup().draw(pm, t, Some(self.sticker)),
            Popup::None => {}
        }
    }
}

trait TextClone {
    fn text_mut_clone(&self) -> Option<String>;
}

impl TextClone for Item {
    fn text_mut_clone(&self) -> Option<String> {
        match &self.shape {
            Shape::Text { text, .. } | Shape::Balloon { text, .. } => Some(text.clone()),
            _ => None,
        }
    }
}

impl Handler for Editor {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let r = match msg {
            WM_SIZE => {
                let mut rc = Default::default();
                unsafe {
                    let _ = GetClientRect(w.hwnd, &mut rc);
                }
                self.client = (rc.right - rc.left, rc.bottom - rc.top);
                if self.scale == 1.0 || self.client.0 > 0 {
                    self.scale = w.dpi_scale();
                    theme::apply_titlebar(w.hwnd, &self.theme);
                }
                self.layout();
                self.update_title(w.hwnd);
                Some(LRESULT(0))
            }
            WM_GETMINMAXINFO => {
                let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                let s = w.dpi_scale();
                mmi.ptMinTrackSize = POINT { x: (560.0 * s) as i32, y: (420.0 * s) as i32 };
                return Some(LRESULT(0));
            }
            WM_PAINT => {
                self.render();
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                return Some(LRESULT(0));
            }
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    unsafe {
                        if let Ok(c) = LoadCursorW(None, self.cursor_shape()) {
                            SetCursor(Some(c));
                        }
                    }
                    return Some(LRESULT(1));
                }
                return None;
            }
            WM_MOUSEMOVE => {
                let (x, y) = super::lparam_xy(lp);
                self.mouse_move(x, y);
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = super::lparam_xy(lp);
                self.mouse_down(w, x, y);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.mouse_up(w);
                Some(LRESULT(0))
            }
            WM_MBUTTONDOWN => {
                let (x, y) = super::lparam_xy(lp);
                unsafe {
                    SetCapture(w.hwnd);
                }
                self.drag = Drag::Pan { start: (x, y), off0: self.off };
                Some(LRESULT(0))
            }
            WM_MBUTTONUP => {
                self.mouse_up(w);
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN => {
                self.commit_text();
                self.selected = None;
                self.popup = Popup::None;
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                let mut p = POINT { x: super::lparam_xy(lp).0, y: super::lparam_xy(lp).1 };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(w.hwnd, &mut p);
                }
                if super::ctrl() {
                    self.zoom_at(if delta > 0 { 1.25 } else { 0.8 }, p.x, p.y);
                } else {
                    let step = delta as f32 / 120.0 * 60.0 * self.s();
                    if super::shift() {
                        self.off.0 += step;
                    } else {
                        self.off.1 += step;
                    }
                    self.fit = false;
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                self.key_down(w, wp.0 as u16);
                Some(LRESULT(0))
            }
            WM_KEYUP => {
                if wp.0 == 0x20 {
                    self.space = false;
                }
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.char_input(wp.0 as u32);
                Some(LRESULT(0))
            }
            WM_EDITOR_TEST_SAVE => {
                let ok = self.save(w.hwnd);
                return Some(LRESULT(ok as isize));
            }
            WM_EDITOR_INSERT => {
                let img = unsafe { Box::from_raw(lp.0 as *mut Pixmap) };
                self.insert_image(*img);
                unsafe {
                    let _ = SetForegroundWindow(w.hwnd);
                }
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                self.commit_text();
                if self.dirty {
                    let what = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "this image".into());
                    match dialogs::ask_save(w.hwnd, &what) {
                        Answer::Cancel => return Some(LRESULT(0)),
                        Answer::Yes => {
                            if !self.save(w.hwnd) {
                                return Some(LRESULT(0));
                            }
                        }
                        Answer::No => {}
                    }
                }
                w.close();
                return Some(LRESULT(0));
            }
            WM_DESTROY => {
                let id = self.id;
                EDITORS.with(|m| m.borrow_mut().remove(&id));
                return None;
            }
            _ => return None,
        };
        w.invalidate();
        r
    }
}
