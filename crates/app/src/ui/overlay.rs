//! The full-screen capture overlay.
//!
//! On open it freezes the screen with one capture, then shows that image
//! dimmed in a topmost window covering every monitor. The user selects a
//! region (or clicks a detected window or control), and in Region mode a
//! quick-annotate toolbar appears so simple markup can be done without
//! opening the editor. The same overlay drives OCR, QR scan, colour picking,
//! the ruler, and choosing an area to record or scroll-capture.
//!
//! Rendering: the frame is composed into a DIB backbuffer, but only inside a
//! dirty rectangle (old + new chrome + selection), so mouse moves stay cheap even
//! on large multi-monitor desktops.

use std::rc::Rc;

use snapit_annotate::{self as ann, History, Item, Shape, Sticker, Style, Tool};
use snapit_capture as cap;
use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, FilterQuality, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Stroke, StrokeDash, Transform};
use snapit_gfx::{blit_rect, crop, pixel_at, text, PointF, Rect, Rgba};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    LoadCursorW, SetCursor, HTCLIENT, IDC_ARROW, IDC_CROSS,
    IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
    WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN,
    WM_SETCURSOR, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::theme::{self, Theme};
use super::toolbar::{self, StickerPopup, StyleHit, StylePopup, Toolbar};
use super::{backbuffer, icons::Icon, Backbuffer, Handler, Win};
use crate::actions::Action;
use crate::app::{self, Capture, Command, Finish};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    SetCustomRegion,
    Scrolling,
    Record(crate::record_ui::Kind),
    Ocr,
    ScanQr,
    ColorPicker,
    Ruler,
    /// Pick an area to insert as an image into editor `id`.
    PickForEditor(u64),
}

impl Mode {
    fn hint(self) -> &'static str {
        match self {
            Mode::Region => "Drag to select · Click a window or control · Esc to cancel",
            Mode::Window => "Click a window to capture it · Esc to cancel",
            Mode::SetCustomRegion => "Drag to set your custom region · Esc to cancel",
            Mode::Scrolling => "Select the scrolling area · Esc to cancel",
            Mode::Record(_) => "Select the area to record · Esc to cancel",
            Mode::Ocr => "Select text to read · Esc to cancel",
            Mode::ScanQr => "Select the QR code · Esc to cancel",
            Mode::ColorPicker => "Click to copy the colour · Esc to cancel",
            Mode::Ruler => "Drag to measure · Esc to close",
            Mode::PickForEditor(_) => "Select an area to insert · Esc to cancel",
        }
    }
    fn detects_windows(self) -> bool {
        !matches!(self, Mode::ColorPicker | Mode::Ruler)
    }
    fn dims(self) -> bool {
        !matches!(self, Mode::ColorPicker)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Btn {
    Tool(Tool),
    Style,
    Stickers,
    Undo,
    Redo,
    Copy,
    Save,
    Pin,
    Editor,
    Done,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum Popup {
    None,
    Style,
    Stickers,
}

enum Drag {
    None,
    NewSel { x0: i32, y0: i32, moved: bool },
    MoveSel { start: (i32, i32), orig: Rect },
    Resize { handle: usize, start: (i32, i32), orig: Rect },
    Draw { start: PointF, before: Vec<Item> },
    MoveItem { idx: usize, last: PointF, before: Option<Vec<Item>> },
    Ruler,
}

struct Overlay {
    mode: Mode,
    settings: Rc<Settings>,
    theme: Theme,
    origin: (i32, i32),
    shot: Pixmap,
    dim: Pixmap,
    bb: Option<Backbuffer>,
    wins: Vec<cap::WindowInfo>,
    mons: Vec<Rect>,
    scale: f32,
    cursor: (i32, i32),
    annotating: bool,
    sel: Option<Rect>,
    hover: Option<Rect>,
    drag: Drag,
    items: Vec<Item>,
    history: History<Vec<Item>>,
    tool: Tool,
    style: Style,
    editing: Option<usize>,
    edit_before: Option<Vec<Item>>,
    selected: Option<usize>,
    /// Full-screen screenshot + annotations. Patched locally when items change, so
    /// moving or resizing the selection never re-renders anything.
    comp: Pixmap,
    /// Items changed since the last sync (cheap flag; the diff happens in `sync_comp`).
    comp_dirty: bool,
    /// Per-item (signature, bounds) from the last sync, to find what changed.
    item_sigs: Vec<(u64, Rect)>,
    bar: Toolbar<Btn>,
    popup: Popup,
    sticker: Sticker,
    ruler: Option<(i32, i32, i32, i32)>,
    /// Chrome element rects drawn in the previous frame (they must be erased).
    prev_elems: Vec<Rect>,
    /// Bright/bordered rectangle (hover or selection) in the previous frame.
    prev_frame: Option<Rect>,
    /// Areas to recompose at the next WM_PAINT.
    pending: Vec<Rect>,
    first_paint: bool,
    style_changed: bool,
    /// Highlighter colour (separate from the pen colour).
    marker: Rgba,
}

thread_local! {
    static TEST_SHOT: std::cell::RefCell<Option<Pixmap>> = const { std::cell::RefCell::new(None) };
}

/// Self-test: the next overlay uses this image instead of capturing the screen.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub fn set_test_shot(p: Pixmap) {
    TEST_SHOT.with(|t| *t.borrow_mut() = Some(p));
}

pub fn open(mode: Mode) {
    if app::with(|s| s.overlay_open) {
        return;
    }
    let s = app::settings();
    let test_shot = TEST_SHOT.with(|t| t.borrow_mut().take());
    let vs = test_shot.as_ref().map(|p| Rect::new(0, 0, p.width() as i32, p.height() as i32)).unwrap_or_else(cap::virtual_screen);
    let cursor_in_shot = s.capture_cursor && !matches!(mode, Mode::ColorPicker | Mode::Ruler | Mode::Record(_) | Mode::Scrolling);
    let captured = match test_shot {
        Some(p) => Ok(p),
        None => cap::capture_rect(vs, cursor_in_shot),
    };
    let shot = match captured {
        Ok(p) => p,
        Err(e) => {
            app::post(Command::Notify { title: "Capture failed".into(), body: e });
            return;
        }
    };
    let to_client = |r: Rect| r.offset(-vs.x, -vs.y);
    let wins = if mode.detects_windows() && (s.region.detect_windows || mode == Mode::Window) {
        cap::visible_windows()
            .into_iter()
            .map(|mut w| {
                w.rect = to_client(w.rect);
                w.children = w.children.into_iter().map(to_client).collect();
                if !s.region.detect_controls || mode == Mode::Window {
                    w.children.clear();
                }
                w
            })
            .collect()
    } else {
        Vec::new()
    };
    let mons: Vec<Rect> = cap::monitors().into_iter().map(|m| to_client(m.rect)).collect();
    let dim = if mode.dims() { dimmed(&shot, s.region.dim_opacity) } else { shot.clone() };
    let a = &s.annotate;
    let style = Style { color: a.color, width: a.width, font_size: a.font_size, fill: false };
    let cursor = cursor_client(vs);

    let ov = Overlay {
        mode,
        theme: theme::DARK,
        origin: (vs.x, vs.y),
        bb: Backbuffer::new(shot.width(), shot.height()),
        comp: shot.clone(),
        shot,
        dim,
        wins,
        mons,
        scale: 1.0,
        cursor,
        annotating: false,
        sel: None,
        hover: None,
        drag: Drag::None,
        items: Vec::new(),
        history: History::new(),
        tool: Tool::Arrow,
        style,
        editing: None,
        edit_before: None,
        selected: None,
        comp_dirty: false,
        item_sigs: Vec::new(),
        bar: Toolbar::new(Vec::new(), 1.0),
        popup: Popup::None,
        sticker: Sticker::Check,
        ruler: None,
        prev_elems: Vec::new(),
        prev_frame: None,
        pending: Vec::new(),
        first_paint: true,
        style_changed: false,
        marker: s.annotate.highlight,
        settings: s,
    };
    let created = super::create("SnapItOverlay", WS_EX_TOPMOST | WS_EX_TOOLWINDOW, WS_POPUP, "Snap It", vs, None, Box::new(ov));
    match created {
        Ok(hwnd) => {
            app::with(|st| st.overlay_open = true);
            super::show(hwnd, true);
            if !super::headless() {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
            }
        }
        Err(e) => crate::log(&format!("overlay: {e}")),
    }
}

fn cursor_client(vs: Rect) -> (i32, i32) {
    let mut p = Default::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    (p.x - vs.x, p.y - vs.y)
}

fn dimmed(src: &Pixmap, amount: u8) -> Pixmap {
    let mut d = src.clone();
    let k = 255 - amount as u32;
    for px in d.data_mut().as_chunks_mut::<4>().0 {
        px[0] = (px[0] as u32 * k / 255) as u8;
        px[1] = (px[1] as u32 * k / 255) as u8;
        px[2] = (px[2] as u32 * k / 255) as u8;
    }
    d
}

/// Self-test: recompose the whole frame from scratch.
pub const WM_OVERLAY_TEST_FULL: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 92;
const HANDLE: f32 = 9.0;
const MAG_CELLS: i32 = 17;
const MAG_CELL: f32 = 8.0;

fn tool_buttons() -> Vec<toolbar::Button<Btn>> {
    use toolbar::{b, sep};
    vec![
        b(Btn::Tool(Tool::Pointer), Icon::Pointer, "Select / move  (V)"),
        sep(Btn::Tool(Tool::Rect), Icon::Rect, "Rectangle  (R)"),
        b(Btn::Tool(Tool::Ellipse), Icon::Ellipse, "Ellipse  (O)"),
        b(Btn::Tool(Tool::Line), Icon::Line, "Line  (L)"),
        b(Btn::Tool(Tool::Arrow), Icon::Arrow, "Arrow  (A)"),
        b(Btn::Tool(Tool::Pen), Icon::Pen, "Pen  (D)"),
        b(Btn::Tool(Tool::Highlight), Icon::Highlight, "Highlight  (H)"),
        b(Btn::Tool(Tool::Text), Icon::Text, "Text  (T)"),
        b(Btn::Tool(Tool::Balloon), Icon::Balloon, "Speech balloon  (K)"),
        b(Btn::Tool(Tool::Step), Icon::Step, "Step number  (N)"),
        b(Btn::Tool(Tool::Blur), Icon::Blur, "Blur  (B)"),
        b(Btn::Tool(Tool::Pixelate), Icon::Pixelate, "Pixelate  (X)"),
        b(Btn::Tool(Tool::Eraser), Icon::Eraser, "Smart eraser  (G)"),
        b(Btn::Tool(Tool::Magnify), Icon::Magnify, "Magnify  (M)"),
        b(Btn::Stickers, Icon::Sticker, "Stickers  (I)"),
        sep(Btn::Style, Icon::Fill, "Colour, width & text size"),
        sep(Btn::Undo, Icon::Undo, "Undo  (Ctrl+Z)"),
        b(Btn::Redo, Icon::Redo, "Redo  (Ctrl+Y)"),
        sep(Btn::Copy, Icon::Copy, "Copy  (Ctrl+C)"),
        b(Btn::Save, Icon::Save, "Save  (Ctrl+S)"),
        b(Btn::Pin, Icon::Pin, "Pin to screen  (P)"),
        b(Btn::Editor, Icon::Editor, "Open in editor  (E)"),
        sep(Btn::Done, Icon::Done, "Done: run after-capture tasks  (Enter)"),
        b(Btn::Close, Icon::Close, "Cancel  (Esc)"),
    ]
}

fn tool_for_key(k: u8) -> Option<Tool> {
    Some(match k {
        b'V' => Tool::Pointer,
        b'R' => Tool::Rect,
        b'O' => Tool::Ellipse,
        b'L' => Tool::Line,
        b'A' => Tool::Arrow,
        b'D' => Tool::Pen,
        b'H' => Tool::Highlight,
        b'T' => Tool::Text,
        b'K' => Tool::Balloon,
        b'N' => Tool::Step,
        b'B' => Tool::Blur,
        b'X' => Tool::Pixelate,
        b'G' => Tool::Eraser,
        b'M' => Tool::Magnify,
        _ => return None,
    })
}

fn handles(r: Rect) -> [(f32, f32); 8] {
    let (l, t, rr, b) = (r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32);
    let (cx, cy) = ((l + rr) / 2.0, (t + b) / 2.0);
    [(l, t), (cx, t), (rr, t), (rr, cy), (rr, b), (cx, b), (l, b), (l, cy)]
}

impl Overlay {
    fn full(&self) -> Rect {
        Rect::new(0, 0, self.shot.width() as i32, self.shot.height() as i32)
    }

    fn monitor_at(&self, x: i32, y: i32) -> Rect {
        self.mons.iter().copied().find(|m| m.contains(x, y)).or_else(|| self.mons.first().copied()).unwrap_or(self.full())
    }

    fn detect(&self, x: i32, y: i32) -> Option<Rect> {
        if !self.mode.detects_windows() {
            return None;
        }
        let r = cap::pick_at(&self.wins, x, y, self.settings.region.detect_controls && self.mode != Mode::Window);
        let r = r.or_else(|| (self.mode != Mode::Window).then(|| self.monitor_at(x, y)))?;
        r.intersect(self.full())
    }

    // ---------- selection & toolbar ----------

    fn place_toolbar(&mut self) {
        let Some(sel) = self.sel else { return };
        let (cx, cy) = sel.center();
        let mon = self.monitor_at(cx, cy);
        let (w, h) = self.bar.size();
        let gap = 8.0 * self.scale;
        let x = (cx as f32 - w / 2.0).clamp(mon.x as f32 + 4.0, (mon.right() as f32 - w - 4.0).max(mon.x as f32));
        let below = sel.bottom() as f32 + gap;
        let above = sel.y as f32 - gap - h;
        let y = if below + h <= mon.bottom() as f32 - 30.0 * self.scale {
            below
        } else if above >= mon.y as f32 + 4.0 {
            above
        } else {
            sel.bottom() as f32 - h - gap
        };
        self.bar.x = x;
        self.bar.y = y;
    }

    fn popup_origin(&self) -> (f32, f32) {
        let r = self.bar.rect();
        let below = r.bottom() as f32 + 6.0 * self.scale;
        let mon = self.monitor_at(r.x, r.y);
        let ph = 90.0 * self.scale;
        if below + ph < mon.bottom() as f32 { (r.x as f32, below) } else { (r.x as f32, r.y as f32 - ph - 6.0 * self.scale) }
    }

    fn style_popup(&self) -> StylePopup {
        let (x, y) = self.popup_origin();
        StylePopup { x, y, scale: self.scale, palette: self.settings.annotate.palette.clone() }
    }

    fn sticker_popup(&self) -> StickerPopup {
        let (x, y) = self.popup_origin();
        let bx = self.bar.button_bounds(Btn::Stickers).map(|r| r.x as f32 - 40.0 * self.scale).unwrap_or(x);
        StickerPopup { x: bx.max(0.0), y, scale: self.scale }
    }

    fn popup_rect(&self) -> Option<Rect> {
        match self.popup {
            Popup::None => None,
            Popup::Style => Some(self.style_popup().rect()),
            Popup::Stickers => Some(self.sticker_popup().rect()),
        }
    }

    fn enter_annotate(&mut self, w: &Win) {
        self.annotating = true;
        self.scale = w.dpi_scale();
        self.bar = Toolbar::new(tool_buttons(), self.scale);
        self.comp_dirty = true;
        self.place_toolbar();
    }

    fn handle_at(&self, x: i32, y: i32) -> Option<usize> {
        let sel = self.sel?;
        let tol = HANDLE * self.scale;
        handles(sel).iter().position(|(hx, hy)| (x as f32 - hx).abs() <= tol && (y as f32 - hy).abs() <= tol)
    }

    fn doc_point(&self, x: i32, y: i32) -> PointF {
        PointF::new(x as f32, y as f32)
    }

    /// Brings `comp` up to date with the items, re-rendering only the areas of items
    /// that changed. Returns those areas (they also need repainting).
    fn sync_comp(&mut self) -> Vec<Rect> {
        if !self.comp_dirty {
            return Vec::new();
        }
        self.comp_dirty = false;
        let full = self.full();
        let sigs: Vec<(u64, Rect)> = self.items.iter().map(|it| (item_sig(it), it.bounds().to_rect().inflate(2))).collect();
        let mut changed = Vec::new();
        for i in 0..sigs.len().max(self.item_sigs.len()) {
            let (old, new) = (self.item_sigs.get(i), sigs.get(i));
            if old.map(|o| o.0) != new.map(|n| n.0) {
                changed.extend(old.map(|o| o.1));
                changed.extend(new.map(|n| n.1));
            }
        }
        self.item_sigs = sigs;
        // Pixel effects (blur, pixelate, magnify) read their whole area, so any region
        // touching one must include all of it or the effect would be cut at the edge.
        let mut rects = merge_rects(changed);
        for _ in 0..4 {
            let mut grown = false;
            for r in rects.iter_mut() {
                for it in &self.items {
                    if matches!(it.shape, Shape::Blur(_) | Shape::Pixelate(_) | Shape::Magnify { .. }) {
                        let b = it.bounds().to_rect().inflate(2);
                        if b.intersect(*r).is_some() && r.union(b) != *r {
                            *r = r.union(b);
                            grown = true;
                        }
                    }
                }
            }
            if !grown {
                break;
            }
            rects = merge_rects(rects);
        }
        let mut out = Vec::new();
        for r in rects {
            let Some(r) = r.intersect(full) else { continue };
            if let Some(mut sub) = crop(&self.shot, r) {
                ann::render(&mut sub.as_mut(), &self.items, -r.x as f32, -r.y as f32);
                blit_rect(&mut self.comp.as_mut(), &sub, Rect::new(0, 0, r.w, r.h), r.x, r.y);
                out.push(r);
            }
        }
        out
    }

    /// The final image: the selected part of the annotated screenshot.
    fn composite(&mut self) -> Option<Pixmap> {
        let sel = self.sel?;
        self.sync_comp();
        crop(&self.comp, sel)
    }

    fn screen_rect(&self, r: Rect) -> Rect {
        r.offset(self.origin.0, self.origin.1)
    }

    // ---------- finishing ----------

    fn commit_text(&mut self) {
        let Some(i) = self.editing.take() else { return };
        let before = self.edit_before.take();
        if i < self.items.len() {
            let empty = self.items[i].text_mut().is_some_and(|t| t.trim().is_empty());
            if empty && matches!(self.items[i].shape, Shape::Text { .. }) {
                self.items.remove(i);
            } else if let Some(b) = before {
                self.history.push(b);
            }
        }
        self.comp_dirty = true;
    }

    fn remember(&self) {
        let sel = self.sel.map(|r| self.screen_rect(r));
        let style = self.style;
        let marker = self.marker;
        let changed = self.style_changed;
        app::update_settings(|s| {
            if sel.is_some() {
                s.last_region = sel;
            }
            if changed {
                s.annotate.color = style.color;
                s.annotate.width = style.width;
                s.annotate.font_size = style.font_size;
                s.annotate.highlight = marker;
            }
        });
    }

    fn finish(&mut self, w: &mut Win, how: Finish) {
        self.commit_text();
        let Some(sel) = self.sel else { return };
        self.remember();
        let screen = self.screen_rect(sel);
        if how == Finish::Editor {
            if let Some(base) = crop(&self.shot, sel) {
                let mut items = self.items.clone();
                items.iter_mut().for_each(|i| i.translate(-sel.x as f32, -sel.y as f32));
                app::post(Command::OpenEditorWith { image: base, items });
            }
        } else if let Some(img) = self.composite() {
            app::post(Command::Finish(Capture { image: img, kind: Action::CaptureRegion, title: "Region".into(), screen_rect: Some(screen) }, how));
        }
        w.close();
    }

    /// A region was chosen (drag finished or a detected window was clicked).
    fn on_selected(&mut self, w: &mut Win, r: Rect) {
        let Some(r) = r.intersect(self.full()) else { return };
        if r.w < 2 || r.h < 2 {
            return;
        }
        self.sel = Some(r);
        self.comp_dirty = true;
        let screen = self.screen_rect(r);
        let image = || crop(&self.shot, r);
        match self.mode {
            Mode::Region => {
                if self.settings.region.quick_annotate {
                    self.enter_annotate(w);
                } else {
                    self.finish(w, Finish::Tasks);
                }
            }
            Mode::Window => self.finish(w, Finish::Tasks),
            Mode::SetCustomRegion => {
                app::update_settings(|s| s.custom_region = Some(screen));
                app::post(Command::Notify { title: "Custom region saved".into(), body: format!("{} × {} at {}, {}", r.w, r.h, screen.x, screen.y) });
                self.finish(w, Finish::Tasks);
            }
            Mode::Scrolling => {
                app::post(Command::StartScrolling(screen));
                w.close();
            }
            Mode::Record(kind) => {
                app::post(Command::StartRecording(kind, screen));
                w.close();
            }
            Mode::Ocr => {
                if let Some(img) = image() {
                    app::post(Command::Ocr(img));
                }
                w.close();
            }
            Mode::ScanQr => {
                if let Some(img) = image() {
                    app::post(Command::ScanQr(img));
                }
                w.close();
            }
            Mode::PickForEditor(id) => {
                if let Some(img) = image() {
                    app::post(Command::EditorInsert { id, image: img });
                }
                w.close();
            }
            Mode::ColorPicker | Mode::Ruler => {}
        }
    }

    // ---------- input ----------

    fn on_button(&mut self, w: &mut Win, b: Btn) {
        match b {
            Btn::Tool(t) => {
                self.commit_text();
                self.tool = t;
                self.selected = None;
                self.popup = Popup::None;
            }
            Btn::Stickers => {
                self.popup = if self.popup == Popup::Stickers { Popup::None } else { Popup::Stickers };
                self.tool = Tool::Sticker(self.sticker);
            }
            Btn::Style => self.popup = if self.popup == Popup::Style { Popup::None } else { Popup::Style },
            Btn::Undo => self.undo(),
            Btn::Redo => self.redo(),
            Btn::Copy => self.finish(w, Finish::Copy),
            Btn::Save => self.finish(w, Finish::Save),
            Btn::Pin => self.finish(w, Finish::Pin),
            Btn::Editor => self.finish(w, Finish::Editor),
            Btn::Done => self.finish(w, Finish::Tasks),
            Btn::Close => w.close(),
        }
    }

    fn undo(&mut self) {
        self.commit_text();
        if let Some(prev) = self.history.undo(self.items.clone()) {
            self.items = prev;
            self.selected = None;
            self.comp_dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.history.redo(self.items.clone()) {
            self.items = next;
            self.selected = None;
            self.comp_dirty = true;
        }
    }

    /// True when colour changes should go to the highlighter's own colour.
    fn marker_context(&self) -> bool {
        self.tool == Tool::Highlight || self.selected.and_then(|i| self.items.get(i)).is_some_and(|it| matches!(it.shape, Shape::Highlight(_)))
    }

    /// The colour shown on the style button and preselected in the popup.
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
        self.style_changed = true;
        // Restyle the selected or edited item too.
        if let Some(i) = self.editing.or(self.selected) {
            if i < self.items.len() {
                if self.editing.is_none() {
                    self.history.push(self.items.clone());
                }
                let marker_color = self.marker;
                let it = &mut self.items[i];
                let color = if matches!(it.shape, Shape::Highlight(_)) { marker_color } else { self.style.color };
                it.style = Style { fill: it.style.fill, color, ..self.style };
                if let (StyleHit::ToggleFill, Shape::Text { background, .. }) = (h, &mut it.shape) {
                    *background = !*background;
                }
                self.comp_dirty = true;
            }
        }
    }

    fn mouse_down(&mut self, w: &mut Win, x: i32, y: i32) {
        // Popups and toolbar first.
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
                            self.tool = Tool::Sticker(k);
                            self.popup = Popup::None;
                        }
                    }
                    Popup::None => {}
                }
                return;
            }
            self.popup = Popup::None;
        }
        if self.annotating {
            if let Some(b) = self.bar.hit(x, y) {
                return self.on_button(w, b);
            }
        }
        unsafe {
            SetCapture(w.hwnd);
        }
        match self.mode {
            Mode::ColorPicker => return self.pick_color(w, x, y),
            Mode::Ruler => {
                self.ruler = Some((x, y, x, y));
                self.drag = Drag::Ruler;
                return;
            }
            Mode::Window => {
                if let Some(r) = self.detect(x, y) {
                    self.on_selected(w, r);
                }
                return;
            }
            _ => {}
        }
        if !self.annotating {
            self.drag = Drag::NewSel { x0: x, y0: y, moved: false };
            return;
        }
        self.commit_text();
        let p = self.doc_point(x, y);
        let sel = self.sel.unwrap_or_default();
        if let Some(hd) = self.handle_at(x, y) {
            self.drag = Drag::Resize { handle: hd, start: (x, y), orig: sel };
            return;
        }
        if self.tool == Tool::Pointer {
            if let Some(i) = ann::hit_test(&self.items, p) {
                self.selected = Some(i);
                self.drag = Drag::MoveItem { idx: i, last: p, before: Some(self.items.clone()) };
            } else if sel.contains(x, y) {
                self.selected = None;
                self.drag = Drag::MoveSel { start: (x, y), orig: sel };
            } else {
                self.restart_selection(x, y);
            }
            return;
        }
        if !sel.contains(x, y) {
            return self.restart_selection(x, y);
        }
        let sample = pixel_at(&self.shot, x, y).unwrap_or(Rgba::WHITE);
        let mut style = if self.tool == Tool::Text { Style { fill: self.style.fill, ..self.style } } else { Style { fill: self.style.fill && matches!(self.tool, Tool::Rect | Tool::Ellipse), ..self.style } };
        if self.tool == Tool::Highlight {
            style.color = self.marker;
        }
        if let Some(item) = Item::begin(self.tool, p, style, &self.items, sample) {
            let before = self.items.clone();
            self.items.push(item);
            self.selected = None;
            self.comp_dirty = true;
            if self.tool == Tool::Text {
                self.editing = Some(self.items.len() - 1);
                self.edit_before = Some(before);
            } else if self.tool == Tool::Step {
                self.history.push(before);
            } else {
                self.drag = Drag::Draw { start: p, before };
            }
        }
    }

    fn restart_selection(&mut self, x: i32, y: i32) {
        self.annotating = false;
        self.sel = None;
        self.selected = None;
        self.drag = Drag::NewSel { x0: x, y0: y, moved: false };
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
        if self.annotating {
            self.bar.set_hover(x, y);
        }
        let shift = super::shift();
        match &mut self.drag {
            Drag::None => {
                if !self.annotating {
                    self.hover = self.detect(x, y);
                }
            }
            Drag::NewSel { x0, y0, moved } => {
                if (x - *x0).abs() > 3 || (y - *y0).abs() > 3 {
                    *moved = true;
                }
                if *moved {
                    self.sel = Some(Rect::from_points(*x0, *y0, x, y));
                    self.hover = None;
                }
            }
            Drag::MoveSel { start, orig } => {
                let full = Rect::new(0, 0, self.shot.width() as i32, self.shot.height() as i32);
                let nx = (orig.x + x - start.0).clamp(0, full.w - orig.w);
                let ny = (orig.y + y - start.1).clamp(0, full.h - orig.h);
                self.sel = Some(Rect::new(nx, ny, orig.w, orig.h));
                self.comp_dirty = true;
                self.place_toolbar();
            }
            Drag::Resize { handle, start, orig } => {
                let (dx, dy) = (x - start.0, y - start.1);
                let (mut l, mut t, mut r, mut b) = (orig.x, orig.y, orig.right(), orig.bottom());
                match *handle {
                    0 => (l, t) = (l + dx, t + dy),
                    1 => t += dy,
                    2 => (r, t) = (r + dx, t + dy),
                    3 => r += dx,
                    4 => (r, b) = (r + dx, b + dy),
                    5 => b += dy,
                    6 => (l, b) = (l + dx, b + dy),
                    _ => l += dx,
                }
                let n = Rect::from_points(l, t, r, b);
                if n.w >= 2 && n.h >= 2 {
                    self.sel = n.intersect(Rect::new(0, 0, self.shot.width() as i32, self.shot.height() as i32));
                    self.comp_dirty = true;
                    self.place_toolbar();
                }
            }
            Drag::Draw { start, .. } => {
                let s = *start;
                if let Some(it) = self.items.last_mut() {
                    it.drag(s, PointF::new(x as f32, y as f32), shift);
                    self.comp_dirty = true;
                }
            }
            Drag::MoveItem { idx, last, .. } => {
                let p = PointF::new(x as f32, y as f32);
                if let Some(it) = self.items.get_mut(*idx) {
                    it.translate(p.x - last.x, p.y - last.y);
                    *last = p;
                    self.comp_dirty = true;
                }
            }
            Drag::Ruler => {
                if let Some(r) = &mut self.ruler {
                    r.2 = x;
                    r.3 = y;
                }
            }
        }
    }

    fn mouse_up(&mut self, w: &mut Win, x: i32, y: i32) {
        unsafe {
            let _ = ReleaseCapture();
        }
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::NewSel { moved, x0, y0 } => {
                if moved {
                    let r = Rect::from_points(x0, y0, x, y);
                    self.on_selected(w, Rect::new(r.x, r.y, r.w + 1, r.h + 1));
                } else if let Some(h) = self.detect(x, y) {
                    self.on_selected(w, h);
                }
            }
            Drag::Draw { before, .. } => {
                let degenerate = self.items.last().is_none_or(|i| i.is_degenerate());
                let is_balloon = self.items.last().is_some_and(|i| matches!(i.shape, Shape::Balloon { .. }));
                if is_balloon {
                    if degenerate || self.items.last().is_some_and(|i| i.bounds().w() < 8.0) {
                        // A click with the balloon tool: give it a sensible default size.
                        let p = PointF::new(x as f32, y as f32);
                        if let Some(it) = self.items.last_mut() {
                            it.drag(p, p.offset(160.0, 56.0), false);
                        }
                    }
                    self.editing = Some(self.items.len() - 1);
                    self.edit_before = Some(before);
                } else if degenerate {
                    self.items.pop();
                } else {
                    self.history.push(before);
                }
                self.comp_dirty = true;
            }
            Drag::MoveItem { before, last, .. } => {
                if let Some(b) = before {
                    if b != self.items {
                        self.history.push(b);
                    }
                }
                let _ = last;
            }
            _ => {}
        }
    }

    fn pick_color(&mut self, w: &mut Win, x: i32, y: i32) {
        if let Some(c) = pixel_at(&self.shot, x, y) {
            let hex = c.hex();
            let _ = crate::clipboard::set_text(&hex);
            app::post(Command::Notify { title: format!("Copied {hex}"), body: format!("RGB {}, {}, {}", c.r, c.g, c.b) });
        }
        w.close();
    }

    fn key_down(&mut self, w: &mut Win, vk: u16) {
        let ctrl = super::ctrl();
        if self.editing.is_some() {
            match vk {
                0x1B => self.commit_text(), // Esc
                0x56 if ctrl => {
                    if let (Some(i), Some(t)) = (self.editing, crate::clipboard::get_text()) {
                        if let Some(s) = self.items.get_mut(i).and_then(|it| it.text_mut()) {
                            s.push_str(&t.replace("\r\n", "\n"));
                            self.comp_dirty = true;
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        match vk {
            0x1B => {
                if self.popup != Popup::None {
                    self.popup = Popup::None;
                } else {
                    w.close();
                }
            }
            0x0D if self.annotating || self.sel.is_some() => self.finish(w, Finish::Tasks),
            0x43 if ctrl && self.sel.is_some() => self.finish(w, Finish::Copy),
            0x53 if ctrl && self.sel.is_some() => self.finish(w, Finish::Save),
            0x5A if ctrl => {
                if super::shift() {
                    self.redo()
                } else {
                    self.undo()
                }
            }
            0x59 if ctrl => self.redo(),
            0x2E => {
                if let Some(i) = self.selected.take() {
                    if i < self.items.len() {
                        self.history.push(self.items.clone());
                        self.items.remove(i);
                        self.comp_dirty = true;
                    }
                }
            }
            0x25..=0x28 if self.annotating && self.tool == Tool::Pointer => {
                let step = if super::shift() { 10 } else { 1 };
                let (dx, dy) = match vk {
                    0x25 => (-step, 0),
                    0x26 => (0, -step),
                    0x27 => (step, 0),
                    _ => (0, step),
                };
                if let Some(s) = self.sel {
                    self.sel = s.offset(dx, dy).intersect(self.full());
                    self.comp_dirty = true;
                    self.place_toolbar();
                }
            }
            _ if self.annotating && !ctrl => {
                let k = vk as u8;
                if k == b'P' {
                    self.finish(w, Finish::Pin);
                } else if k == b'E' {
                    self.finish(w, Finish::Editor);
                } else if k == b'I' {
                    self.tool = Tool::Sticker(self.sticker);
                    self.popup = Popup::Stickers;
                } else if let Some(t) = tool_for_key(k) {
                    self.tool = t;
                    self.selected = None;
                }
            }
            _ => {}
        }
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

    fn wheel(&mut self, delta: i16) {
        if !self.annotating {
            return;
        }
        let up = delta > 0;
        if matches!(self.tool, Tool::Text | Tool::Balloon | Tool::Step) {
            self.style.font_size = (self.style.font_size + if up { 2.0 } else { -2.0 }).clamp(10.0, 120.0);
        } else {
            self.style.width = (self.style.width + if up { 1.0 } else { -1.0 }).clamp(1.0, 40.0);
        }
        self.style_changed = true;
    }

    fn cursor_shape(&self) -> windows::core::PCWSTR {
        let (x, y) = self.cursor;
        if self.annotating {
            if self.bar.rect().contains(x, y) || self.popup_rect().is_some_and(|r| r.contains(x, y)) {
                return IDC_ARROW;
            }
            if let Some(h) = self.handle_at(x, y) {
                return match h {
                    0 | 4 => IDC_SIZENWSE,
                    2 | 6 => IDC_SIZENESW,
                    1 | 5 => IDC_SIZENS,
                    _ => IDC_SIZEWE,
                };
            }
            if self.tool == Tool::Pointer {
                let p = self.doc_point(x, y);
                if ann::hit_test(&self.items, p).is_some() || self.sel.is_some_and(|s| s.contains(x, y)) {
                    return IDC_SIZEALL;
                }
            }
            if self.tool == Tool::Text {
                return IDC_IBEAM;
            }
        }
        IDC_CROSS
    }

    // ---------- drawing ----------

    fn magnifier_rect(&self) -> Rect {
        let s = self.scale.max(1.0);
        let size = (MAG_CELLS as f32 * MAG_CELL * s) as i32;
        let label = (44.0 * s) as i32;
        let (x, y) = self.cursor;
        let mon = self.monitor_at(x, y);
        let off = (22.0 * s) as i32;
        let mut rx = x + off;
        let mut ry = y + off;
        if rx + size > mon.right() {
            rx = x - off - size;
        }
        if ry + size + label > mon.bottom() {
            ry = y - off - size - label;
        }
        Rect::new(rx, ry, size, size + label)
    }

    fn show_magnifier(&self) -> bool {
        (self.mode == Mode::ColorPicker || (self.settings.region.magnifier && !self.annotating && !matches!(self.mode, Mode::Window)))
            && !matches!(self.drag, Drag::MoveSel { .. })
    }

    fn hint_rect(&self) -> Option<Rect> {
        if self.annotating || !matches!(self.drag, Drag::None) {
            return None;
        }
        let (x, y) = self.cursor;
        let mon = self.monitor_at(x, y);
        let s = self.scale.max(1.0);
        let size = 14.0 * s;
        let l = text::layout(self.mode.hint(), size, false);
        let (w, h) = ((l.width + 28.0 * s) as i32, (l.height + 14.0 * s) as i32);
        Some(Rect::new(mon.x + (mon.w - w) / 2, mon.y + (24.0 * s) as i32, w, h))
    }

    fn label_rect(&self, sel: Rect) -> Rect {
        let s = self.scale.max(1.0);
        let h = (24.0 * s) as i32;
        let w = (130.0 * s) as i32;
        let y = if sel.y - h - 4 >= 0 { sel.y - h - 4 } else { sel.y + 4 };
        Rect::new(sel.x, y, w, h)
    }

    fn ruler_bounds(&self) -> Option<Rect> {
        let (x0, y0, x1, y1) = self.ruler?;
        let r = Rect::from_points(x0, y0, x1, y1).inflate(4);
        Some(r.union(Rect::new(x1 + 10, y1 + 10, (260.0 * self.scale.max(1.0)) as i32, (60.0 * self.scale.max(1.0)) as i32)))
    }

    /// The bright, outlined rectangle: the selection, or the hovered window while choosing.
    fn frame_rect(&self) -> Option<Rect> {
        self.sel.or(if self.annotating { None } else { self.hover })
    }

    /// Bounds of every translucent / anti-aliased chrome element drawn this frame.
    /// Each must be fully restored before it is drawn again, so they are always repainted.
    /// (The frame outline is drawn without anti-aliasing, so redrawing it is idempotent.)
    fn elements(&self) -> Vec<Rect> {
        let s = self.scale.max(1.0);
        let mut v = Vec::new();
        if let Some(sel) = self.sel {
            v.push(self.label_rect(sel).inflate(2));
            if self.handles_visible() {
                let hs = (HANDLE * s) as i32 + 3;
                for (hx, hy) in handles(sel) {
                    v.push(Rect::new(hx as i32 - hs, hy as i32 - hs, hs * 2, hs * 2));
                }
            }
            for i in self.selected.iter().chain(self.editing.iter()) {
                if let Some(it) = self.items.get(*i) {
                    v.push(it.bounds().to_rect().inflate((4.0 * s) as i32 + 4));
                }
            }
        }
        if self.show_magnifier() {
            v.push(self.magnifier_rect().inflate(5));
        }
        if self.settings.region.crosshair && !self.annotating {
            let (x, y) = self.cursor;
            v.push(Rect::new(0, y - 2, self.shot.width() as i32, 5));
            v.push(Rect::new(x - 2, 0, 5, self.shot.height() as i32));
        }
        if let Some(h) = self.hint_rect() {
            v.push(h.inflate(2));
        }
        if let Some(rb) = self.ruler_bounds() {
            v.push(rb.inflate(4));
        }
        if self.annotating {
            v.push(self.bar.paint_bounds());
            if let Some(p) = self.popup_rect() {
                v.push(p.inflate(4));
            }
        }
        v
    }

    fn handles_visible(&self) -> bool {
        (self.annotating && self.tool == Tool::Pointer) || matches!(self.drag, Drag::Resize { .. })
    }

    /// Records what changed since the last frame and schedules a repaint of just those
    /// areas. The actual drawing happens in WM_PAINT, so a burst of mouse moves costs
    /// one frame, not one frame each.
    fn redraw(&mut self, w: &Win) {
        if self.scale == 1.0 {
            self.scale = w.dpi_scale();
        }
        let mut dirty = std::mem::take(&mut self.prev_elems);
        let cur = self.elements();
        dirty.extend(cur.iter().copied());
        self.prev_elems = cur;

        // Frame (hover/selection) moved: only the strips that changed brightness, plus
        // both outlines, need repainting.
        let frame = self.frame_rect();
        if frame != self.prev_frame {
            let t = (2.5 * self.scale.max(1.0)).ceil() as i32 + 2;
            match (self.prev_frame, frame) {
                (Some(a), Some(b)) => {
                    dirty.extend(subtract(a, b));
                    dirty.extend(subtract(b, a));
                }
                (Some(a), None) | (None, Some(a)) => dirty.push(a),
                (None, None) => {}
            }
            dirty.extend(self.prev_frame.iter().flat_map(|r| edges(*r, t)));
            dirty.extend(frame.iter().flat_map(|r| edges(*r, t)));
            self.prev_frame = frame;
        }
        dirty.extend(self.sync_comp());

        let full = self.full();
        for r in dirty.into_iter().filter_map(|r| r.intersect(full)) {
            super::invalidate(w.hwnd, Some(r));
            self.pending.push(r);
        }
    }

    /// Restores every pending area from the dimmed / bright images, then draws the chrome once.
    fn compose_pending(&mut self) {
        let rects = merge_rects(std::mem::take(&mut self.pending));
        if rects.is_empty() {
            return;
        }
        let Some(mut bb) = self.bb.take() else { return };
        {
            let mut pm = bb.pixmap();
            let frame = self.frame_rect();
            for r in &rects {
                blit_rect(&mut pm, &self.dim, *r, r.x, r.y);
                if let Some(b) = frame.and_then(|f| f.intersect(*r)) {
                    blit_rect(&mut pm, &self.comp, b, b.x, b.y);
                }
            }
            self.draw_chrome(&mut pm);
        }
        self.bb = Some(bb);
    }

    fn draw_chrome(&self, pm: &mut PixmapMut) {
        let t = &self.theme;
        let s = self.scale.max(1.0);
        let id = Transform::identity();
        let thin = Stroke { width: 1.0, ..Default::default() };
        // Frame outline: opaque and not anti-aliased, so drawing it again over itself is a no-op.
        let mut frame_paint = t.accent.paint();
        frame_paint.anti_alias = false;
        if let (Some(h), None) = (self.hover, self.sel) {
            if let Some(p) = h.to_skia().map(PathBuilder::from_rect) {
                pm.stroke_path(&p, &frame_paint, &Stroke { width: (2.0 * s).round(), ..Default::default() }, id, None);
            }
        }
        if let Some(sel) = self.sel {
            if let Some(p) = sel.to_skia().map(PathBuilder::from_rect) {
                pm.stroke_path(&p, &frame_paint, &Stroke { width: (1.5 * s).round().max(1.0), ..Default::default() }, id, None);
            }
            if self.handles_visible() {
                for (hx, hy) in handles(sel) {
                    let hs = HANDLE * s * 0.8;
                    if let Some(p) = rounded_rect(hx - hs / 2.0, hy - hs / 2.0, hs, hs, 2.0) {
                        pm.fill_path(&p, &Rgba::WHITE.paint(), FillRule::Winding, id, None);
                        pm.stroke_path(&p, &t.accent.paint(), &thin, id, None);
                    }
                }
            }
            let lr = self.label_rect(sel);
            let label = format!("{} × {}", sel.w, sel.h);
            let size = 13.0 * s;
            let l = text::layout(&label, size, false);
            if let Some(p) = rounded_rect(lr.x as f32, lr.y as f32, l.width + 16.0 * s, lr.h as f32, 5.0 * s) {
                pm.fill_path(&p, &Rgba::new(0, 0, 0, 190).paint(), FillRule::Winding, id, None);
            }
            text::draw(pm, &label, lr.x as f32 + 8.0 * s, lr.y as f32 + (lr.h as f32 - l.height) / 2.0, size, false, Rgba::WHITE, None);

            if let Some(it) = self.selected.and_then(|i| self.items.get(i)) {
                let b = it.bounds();
                if let Some(p) = b.skia().map(PathBuilder::from_rect) {
                    let dash = Stroke { width: 1.0 * s, dash: StrokeDash::new(vec![4.0 * s, 3.0 * s], 0.0), ..Default::default() };
                    pm.stroke_path(&p, &Rgba::WHITE.paint(), &dash, id, None);
                }
            }
            if let Some(it) = self.editing.and_then(|i| self.items.get(i)) {
                draw_caret(pm, it, s);
            }
        }
        if self.settings.region.crosshair && !self.annotating {
            let (x, y) = (self.cursor.0 as f32 + 0.5, self.cursor.1 as f32 + 0.5);
            let mut pb = PathBuilder::new();
            pb.move_to(0.0, y);
            pb.line_to(self.shot.width() as f32, y);
            pb.move_to(x, 0.0);
            pb.line_to(x, self.shot.height() as f32);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &t.accent.with_alpha(170).paint(), &thin, id, None);
            }
        }
        if let Some((x0, y0, x1, y1)) = self.ruler {
            self.draw_ruler(pm, x0, y0, x1, y1);
        }
        if self.show_magnifier() {
            self.draw_magnifier(pm);
        }
        if let Some(h) = self.hint_rect() {
            if let Some(p) = rounded_rect(h.x as f32, h.y as f32, h.w as f32, h.h as f32, h.h as f32 / 2.0) {
                pm.fill_path(&p, &Rgba::new(20, 20, 24, 215).paint(), FillRule::Winding, id, None);
            }
            let size = 14.0 * s;
            let l = text::layout(self.mode.hint(), size, false);
            text::draw(pm, self.mode.hint(), h.x as f32 + (h.w as f32 - l.width) / 2.0, h.y as f32 + (h.h as f32 - l.height) / 2.0, size, false, Rgba::WHITE, None);
        }
        if self.annotating {
            let tool = self.tool;
            let popup = self.popup;
            self.bar.draw(
                pm,
                t,
                |b| match b {
                    Btn::Tool(x) => x == tool,
                    Btn::Stickers => matches!(tool, Tool::Sticker(_)) || popup == Popup::Stickers,
                    Btn::Style => popup == Popup::Style,
                    _ => false,
                },
                Some((Btn::Style, self.shown_color(), self.style.width)),
            );
            match self.popup {
                Popup::Style => self.style_popup().draw(pm, t, self.shown_color(), self.style.width, self.style.font_size, self.style.fill),
                Popup::Stickers => self.sticker_popup().draw(pm, t, Some(self.sticker)),
                Popup::None => {}
            }
        }
    }

    fn draw_magnifier(&self, pm: &mut PixmapMut) {
        let r = self.magnifier_rect();
        let s = self.scale.max(1.0);
        let cells = MAG_CELLS;
        let cell = MAG_CELL * s;
        let (cx, cy) = self.cursor;
        let src = Rect::new(cx - cells / 2, cy - cells / 2, cells, cells);
        let id = Transform::identity();
        let size = cells as f32 * cell;
        let (x0, y0) = (r.x as f32, r.y as f32);
        if let Some(bg) = rounded_rect(x0 - 3.0, y0 - 3.0, size + 6.0, r.h as f32 + 6.0, 8.0) {
            pm.fill_path(&bg, &Rgba::new(20, 20, 24, 240).paint(), FillRule::Winding, id, None);
        }
        // Black padding where the sample hangs off the screen edge (blit_rect clips).
        if let Some(mut sample) = Pixmap::new(cells as u32, cells as u32) {
            sample.fill(Rgba::BLACK.color());
            blit_rect(&mut sample.as_mut(), &self.shot, src, 0, 0);
            let paint = PixmapPaint { quality: FilterQuality::Nearest, ..Default::default() };
            pm.draw_pixmap(0, 0, sample.as_ref(), &paint, Transform::from_row(cell, 0.0, 0.0, cell, x0, y0), None);
        }
        // Pixel grid.
        let mut grid = PathBuilder::new();
        for i in 1..cells {
            let o = i as f32 * cell;
            grid.move_to(x0 + o, y0);
            grid.line_to(x0 + o, y0 + size);
            grid.move_to(x0, y0 + o);
            grid.line_to(x0 + size, y0 + o);
        }
        if let Some(g) = grid.finish() {
            pm.stroke_path(&g, &Rgba::new(128, 128, 128, 45).paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
        // Centre pixel frame (dark + light so it shows on any colour).
        let c0 = (cells / 2) as f32 * cell;
        if let Some(p) = snapit_gfx::tiny_skia::Rect::from_xywh(x0 + c0, y0 + c0, cell, cell).map(PathBuilder::from_rect) {
            pm.stroke_path(&p, &Rgba::BLACK.paint(), &Stroke { width: 3.0, ..Default::default() }, id, None);
            pm.stroke_path(&p, &Rgba::WHITE.paint(), &Stroke { width: 1.5, ..Default::default() }, id, None);
        }
        // Readout: coordinates, then a swatch and the hex value.
        let colour = pixel_at(&self.shot, cx, cy).unwrap_or(Rgba::BLACK);
        let ts = 12.0 * s;
        let pad = 7.0 * s;
        let line1 = format!("X {}   Y {}", cx + self.origin.0, cy + self.origin.1);
        text::draw(pm, &line1, x0 + pad, y0 + size + 4.0 * s, ts, false, Rgba::rgb(200, 200, 210), None);
        let sw = 13.0 * s;
        let ly = y0 + size + 23.0 * s;
        if let Some(d) = rounded_rect(x0 + pad, ly + 1.0, sw, sw, 3.0) {
            pm.fill_path(&d, &colour.paint(), FillRule::Winding, id, None);
            pm.stroke_path(&d, &Rgba::new(255, 255, 255, 120).paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
        text::draw(pm, &colour.hex(), x0 + pad + sw + 6.0 * s, ly, ts, true, Rgba::WHITE, None);
    }

    fn draw_ruler(&self, pm: &mut PixmapMut, x0: i32, y0: i32, x1: i32, y1: i32) {
        let s = self.scale.max(1.0);
        let id = Transform::identity();
        let r = Rect::from_points(x0, y0, x1, y1);
        if let Some(p) = r.to_skia().map(PathBuilder::from_rect) {
            let dash = Stroke { width: 1.0, dash: StrokeDash::new(vec![5.0, 4.0], 0.0), ..Default::default() };
            pm.stroke_path(&p, &self.theme.accent.paint(), &dash, id, None);
        }
        let mut pb = PathBuilder::new();
        pb.move_to(x0 as f32, y0 as f32);
        pb.line_to(x1 as f32, y1 as f32);
        if let Some(p) = pb.finish() {
            pm.stroke_path(&p, &Rgba::YELLOW.paint(), &Stroke { width: 2.0 * s, ..Default::default() }, id, None);
        }
        let (dx, dy) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let len = (dx * dx + dy * dy).sqrt();
        let ang = (-dy).atan2(dx).to_degrees();
        let label = format!("W {}  H {}\nLength {:.1} px   Angle {:.1}°", r.w, r.h, len, ang);
        let ts = 13.0 * s;
        let l = text::layout(&label, ts, false);
        let (lx, ly) = (x1 as f32 + 12.0, y1 as f32 + 12.0);
        if let Some(p) = rounded_rect(lx, ly, l.width + 16.0 * s, l.height + 10.0 * s, 6.0 * s) {
            pm.fill_path(&p, &Rgba::new(20, 20, 24, 225).paint(), FillRule::Winding, id, None);
        }
        text::draw(pm, &label, lx + 8.0 * s, ly + 5.0 * s, ts, false, Rgba::WHITE, None);
    }
}

/// A cheap fingerprint of everything that affects how an item renders.
fn item_sig(it: &Item) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let f = |v: f32| v.to_bits();
    let b = it.bounds();
    (f(b.x0), f(b.y0), f(b.x1), f(b.y1)).hash(&mut h);
    let st = &it.style;
    (st.color.r, st.color.g, st.color.b, st.color.a, f(st.width), f(st.font_size), st.fill).hash(&mut h);
    std::mem::discriminant(&it.shape).hash(&mut h);
    match &it.shape {
        Shape::Line(a, b) | Shape::Arrow(a, b) => (f(a.x), f(a.y), f(b.x), f(b.y)).hash(&mut h),
        Shape::Pen(p) => {
            p.len().hash(&mut h);
            if let Some(l) = p.last() {
                (f(l.x), f(l.y)).hash(&mut h);
            }
        }
        Shape::Text { text, background, .. } => (text, background).hash(&mut h),
        Shape::Balloon { text, tail, .. } => (text, f(tail.x), f(tail.y)).hash(&mut h),
        Shape::Step { n, .. } => n.hash(&mut h),
        Shape::Eraser(_, c) => (c.r, c.g, c.b, c.a).hash(&mut h),
        Shape::Sticker { kind, .. } => kind.hash(&mut h),
        Shape::Image { img, .. } => (std::sync::Arc::as_ptr(img) as usize).hash(&mut h),
        Shape::Magnify { zoom, .. } => f(*zoom).hash(&mut h),
        _ => {}
    }
    h.finish()
}

/// Parts of `a` not covered by `b` (up to four rectangles).
fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    let Some(i) = a.intersect(b) else { return vec![a] };
    [
        Rect::from_ltrb(a.x, a.y, a.right(), i.y),
        Rect::from_ltrb(a.x, i.bottom(), a.right(), a.bottom()),
        Rect::from_ltrb(a.x, i.y, i.x, i.bottom()),
        Rect::from_ltrb(i.right(), i.y, a.right(), i.bottom()),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
    .collect()
}

/// Thin strips covering a rectangle's outline (`t` pixels either side of each edge).
fn edges(r: Rect, t: i32) -> [Rect; 4] {
    [
        Rect::new(r.x - t, r.y - t, r.w + 2 * t, 2 * t),
        Rect::new(r.x - t, r.bottom() - t, r.w + 2 * t, 2 * t),
        Rect::new(r.x - t, r.y - t, 2 * t, r.h + 2 * t),
        Rect::new(r.right() - t, r.y - t, 2 * t, r.h + 2 * t),
    ]
}

/// Merges rectangles whose union is not much bigger than the pair, to avoid
/// restoring overlapping areas twice without inflating small, distant ones.
fn merge_rects(mut v: Vec<Rect>) -> Vec<Rect> {
    v.retain(|r| !r.is_empty());
    let mut merged = true;
    while merged && v.len() > 1 {
        merged = false;
        'outer: for i in 0..v.len() {
            for j in i + 1..v.len() {
                let u = v[i].union(v[j]);
                if u.area() * 10 <= (v[i].area() + v[j].area()) * 12 {
                    v[i] = u;
                    v.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    v
}

fn draw_caret(pm: &mut PixmapMut, it: &Item, s: f32) {
    let size = it.style.font_size;
    let (x, y, h) = match &it.shape {
        Shape::Text { at, text: t, .. } => {
            let l = text::layout(if t.is_empty() { " " } else { t }, size, false);
            let lw = if t.is_empty() { 0.0 } else { text::layout(t.rsplit('\n').next().unwrap_or(""), size, false).width };
            (at.x + lw + 1.0, at.y + (l.lines.max(1) - 1) as f32 * l.line_height + if t.ends_with('\n') { l.line_height } else { 0.0 }, l.line_height)
        }
        Shape::Balloon { r, text: t, .. } => {
            let n = r.norm();
            let pad = size * 0.5;
            let l = text::layout(if t.is_empty() { " " } else { t }, size, false);
            let lw = text::layout(t.rsplit('\n').next().unwrap_or(""), size, false).width;
            (n.x0 + pad + lw + 1.0, n.y0 + pad + (l.lines.max(1) - 1) as f32 * l.line_height, l.line_height)
        }
        _ => return,
    };
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x, y + h);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &Rgba::WHITE.paint(), &Stroke { width: 3.0 * s, ..Default::default() }, Transform::identity(), None);
        pm.stroke_path(&p, &Rgba::BLACK.paint(), &Stroke { width: 1.2 * s, ..Default::default() }, Transform::identity(), None);
    }
}

impl Handler for Overlay {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => {
                if self.first_paint {
                    // First paint: compose everything once.
                    self.first_paint = false;
                    self.scale = w.dpi_scale();
                    self.prev_elems = self.elements();
                    self.prev_frame = self.frame_rect();
                    self.pending = vec![self.full()];
                }
                self.compose_pending();
                backbuffer::paint_region(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_OVERLAY_TEST_FULL => {
                // Self-test: recompose everything from scratch (to compare with the incremental frame).
                self.pending = vec![self.full()];
                self.compose_pending();
                Some(LRESULT(1))
            }
            WM_DPICHANGED => Some(LRESULT(0)), // keep covering the whole virtual screen
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    unsafe {
                        if let Ok(c) = LoadCursorW(None, self.cursor_shape()) {
                            SetCursor(Some(c));
                        }
                    }
                    return Some(LRESULT(1));
                }
                None
            }
            WM_MOUSEMOVE => {
                let (x, y) = super::lparam_xy(lp);
                self.mouse_move(x, y);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (x, y) = super::lparam_xy(lp);
                self.mouse_down(w, x, y);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = super::lparam_xy(lp);
                let inside = self.sel.is_some_and(|s| s.contains(x, y));
                if self.annotating && self.tool == Tool::Pointer && inside && ann::hit_test(&self.items, self.doc_point(x, y)).is_none() {
                    self.finish(w, Finish::Tasks);
                } else {
                    self.mouse_down(w, x, y);
                    self.redraw(w);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = super::lparam_xy(lp);
                self.mouse_up(w, x, y);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN => {
                if !matches!(self.drag, Drag::None) {
                    if let Drag::Draw { before, .. } = std::mem::replace(&mut self.drag, Drag::None) {
                        self.items = before;
                        self.comp_dirty = true;
                    }
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                } else if self.editing.is_some() {
                    self.commit_text();
                } else if self.annotating || self.sel.is_some() {
                    self.annotating = false;
                    self.sel = None;
                    self.items.clear();
                    self.comp_dirty = true;
                    self.selected = None;
                    self.popup = Popup::None;
                    let (x, y) = self.cursor;
                    self.hover = self.detect(x, y);
                } else {
                    w.close();
                }
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                self.wheel(((wp.0 >> 16) & 0xFFFF) as u16 as i16);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                self.key_down(w, wp.0 as u16);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.char_input(wp.0 as u32);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                if self.style_changed && self.sel.is_none() {
                    self.remember();
                }
                app::post(Command::OverlayClosed);
                None
            }
            _ => None,
        }
    }
}
