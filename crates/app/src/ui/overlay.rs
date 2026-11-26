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
use snapit_capture::uia::{Fetch, Fetcher};
use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, FilterQuality, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Stroke, StrokeDash, Transform};
use snapit_gfx::{blit_rect, crop, edges, pixel_at, text, PointF, Rect, Rgba};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    LoadCursorW, SetCursor, HTCLIENT, IDC_ARROW, IDC_CROSS,
    IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
    WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN,
    WM_SETCURSOR, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::text_caret::{TextCaret, TextUndo, Typed};
use super::theme::{self, Theme};
use super::tool_options::{options_for, ColorHit, ColorPopup, Opt, OptHit, OptionsBar, Values};
use super::toolbar::{self, StickerPopup, Toolbar};
use super::{backbuffer, icons::Icon, Backbuffer, Handler, Win};
use crate::actions::Action;
use crate::app::{self, Capture, Command, Finish};
use crate::settings::{Settings, ToolOptions};

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
            Mode::Region => "Drag to select · Click a control, or hold Ctrl for the whole window · Scroll to widen or narrow · Esc to cancel",
            Mode::Window => "Click a window to capture it · Esc to cancel",
            Mode::SetCustomRegion => "Drag to set your custom region · Esc to cancel",
            Mode::Scrolling => "Select the scrolling area · Esc cancels · once it scrolls, Esc or a right-click stops it",
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
    Color(Opt),
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
    /// Dragging across the text being edited to select it.
    SelectText,
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
    /// Each monitor's work area (without the taskbar), same order as `mons`.
    works: Vec<Rect>,
    scale: f32,
    cursor: (i32, i32),
    annotating: bool,
    sel: Option<Rect>,
    hover: Option<Rect>,
    /// Scroll-wheel steps from the default snap target under the cursor
    /// (positive: enclosing elements, negative: finer parts).
    hover_step: i32,
    /// Default snap target the steps count from; moving onto another resets them.
    hover_base: Option<Rect>,
    /// Scrolled out to the outermost target (the window): stay there while the
    /// cursor moves within that window, until scrolled back in or the cursor
    /// moves onto another window.
    hover_outer: bool,
    /// The window under the cursor when the hover was last picked.
    hover_win: Option<isize>,
    /// The cursor moved since the hover was last picked. Picking scans the
    /// screenshot, so it runs once per painted frame, not on every mouse move
    /// (a fast mouse sends hundreds per second and would hold off painting).
    hover_stale: bool,
    /// UI Automation element fetcher (started on the first message, once the window exists).
    uia: Option<Fetcher>,
    drag: Drag,
    items: Vec<Item>,
    history: History<Vec<Item>>,
    tool: Tool,
    style: Style,
    editing: Option<usize>,
    edit_before: Option<Vec<Item>>,
    /// Caret and selection in the text being edited.
    caret: TextCaret,
    /// Step-by-step undo inside the text box being edited.
    text_undo: TextUndo,
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
    /// The rest of the tool options (fill, shadow, dashes, blur radius…), shared with the editor.
    opts: ToolOptions,
    /// Mouse position for hover effects in the options strip.
    opt_hover: Option<(i32, i32)>,
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
                w.client = to_client(w.client);
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
    let monitors = cap::monitors();
    let mons: Vec<Rect> = monitors.iter().map(|m| to_client(m.rect)).collect();
    let works: Vec<Rect> = monitors.iter().map(|m| to_client(m.work)).collect();
    let dim = if mode.dims() { dimmed(&shot, s.region.dim_opacity) } else { shot.clone() };
    let a = &s.annotate;
    let style = Style { color: a.color, width: a.width, font_size: a.font_size, ..Style::default() };
    let cursor = cursor_client(vs);

    let ov = Overlay {
        mode,
        theme: theme::current(),
        origin: (vs.x, vs.y),
        bb: Backbuffer::new(shot.width(), shot.height()),
        comp: shot.clone(),
        shot,
        dim,
        wins,
        mons,
        works,
        scale: 1.0,
        cursor,
        annotating: false,
        sel: None,
        hover: None,
        hover_step: 0,
        hover_base: None,
        hover_outer: false,
        hover_win: None,
        hover_stale: false,
        uia: None,
        drag: Drag::None,
        items: Vec::new(),
        history: History::new(),
        tool: Tool::Arrow,
        style,
        editing: None,
        edit_before: None,
        caret: TextCaret::default(),
        text_undo: TextUndo::default(),
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
        opts: a.tools,
        opt_hover: None,
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
/// Posted by the UIA worker when a window's elements are ready (wparam = window handle).
const WM_OVERLAY_UIA: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 93;
const HANDLE: f32 = 9.0;
const MAG_CELLS: i32 = 17;
const MAG_CELL: f32 = 8.0;

/// Quick-edit button size: about 25% bigger than the editor's.
const QUICK_BTN: f32 = 42.0;
/// Padding around the options strip inside its panel (unscaled).
const OPT_PAD_X: f32 = 10.0;
const OPT_PAD_Y: f32 = 6.0;
/// Gap between the toolbar and the options panel (unscaled).
const OPT_GAP: f32 = 6.0;

/// Two rows: drawing tools on top, style and actions underneath.
fn tool_buttons() -> Vec<toolbar::Button<Btn>> {
    use toolbar::{b, row, sep};
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
        b(Btn::Tool(Tool::Spotlight), Icon::Spotlight, "Spotlight: dim everything else  (S)"),
        b(Btn::Stickers, Icon::Sticker, "Stickers  (I)"),
        row(Btn::Undo, Icon::Undo, "Undo  (Ctrl+Z)"),
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
        b'S' => Tool::Spotlight,
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

    fn snaps_controls(&self) -> bool {
        self.settings.region.detect_controls && self.mode != Mode::Window
    }

    fn wants_uia(&self) -> bool {
        self.mode.detects_windows() && self.snaps_controls() && self.settings.region.uia_controls && !self.wins.is_empty()
    }

    /// Snap targets under (x, y), innermost first, ending with the window (or
    /// monitor), and the index of the default one.
    ///
    /// Inside a window: UI Automation elements when the app exposes them; otherwise
    /// boxes found from the screenshot's edges plus any child window (legacy Win32
    /// controls). All local: the UIA data was fetched in the background.
    ///
    /// "Fine" targets (text labels, tiny parts, a box cut off by a divider) sit
    /// below the default and are reached by scrolling down, so hovering a button's
    /// caption picks the button and hovering half a card picks the card.
    ///
    /// The whole window is the default instead while Ctrl is held, or over its
    /// title bar or frame; scrolling down still reaches the parts inside.
    fn candidates(&self, x: i32, y: i32) -> (Vec<Rect>, usize) {
        if !self.mode.detects_windows() {
            return (Vec::new(), 0);
        }
        let full = self.full();
        let Some(w) = self.wins.iter().find(|w| w.rect.contains(x, y)) else {
            return if self.mode == Mode::Window { (Vec::new(), 0) } else { (self.monitor_at(x, y).intersect(full).into_iter().collect(), 0) };
        };
        // (rect, fine)
        let mut out: Vec<(Rect, bool)> = Vec::new();
        let mut whole = super::ctrl() || (!w.client.is_empty() && !w.client.contains(x, y));
        if self.snaps_controls() {
            // A "control" that is (nearly) the whole window adds nothing.
            let part = |r: &Rect| r.contains(x, y) && r.area() * 100 < w.rect.area() * 95;
            if let Some(Fetch::Ready(els)) = self.uia.as_ref().map(|f| f.get(w.hwnd)) {
                for e in els.iter() {
                    let r = e.rect.offset(-self.origin.0, -self.origin.1);
                    // A title bar the app draws itself (inside its client area).
                    whole |= e.title && r.contains(x, y);
                    if part(&r) {
                        out.extend(r.intersect(w.rect).map(|r| (r, e.fine)));
                    }
                }
            }
            if let Some(d) = out.iter().filter(|(_, f)| !f).map(|(r, _)| *r).min_by_key(|r| r.area()) {
                // Apps' element trees are often coarser than what they draw (a
                // user panel exposed as one element, holding visibly separate
                // strips). Boxes seen in the pixels slot in between the app's own
                // levels: below the app's default as finer steps, above it as
                // steps out. The app's element stays the default. Searched across
                // the window, not inside an element: drawn panels often overlap
                // the elements around them, so an element would cut them short.
                let seen: Vec<Rect> = edges::boxes_at(&self.shot, x, y, w.rect)
                    .into_iter()
                    .map(|(r, _)| r)
                    .filter(|r| !out.iter().any(|(o, _)| edges::near(*o, *r)))
                    .collect();
                out.extend(seen.into_iter().map(|r| (r, r.area() < d.area())));
            }
            if out.is_empty() {
                let child = w.children.iter().find(|c| part(c)).and_then(|c| c.intersect(w.rect));
                let bounds = child.unwrap_or(w.rect);
                out.extend(edges::boxes_at(&self.shot, x, y, bounds));
                out.extend(child.map(|r| (r, false)));
            }
        }
        out.push((w.rect, false));
        let mut v: Vec<(Rect, bool)> = out.into_iter().filter_map(|(r, f)| r.intersect(full).map(|r| (r, f))).filter(|(r, _)| r.w >= 2 && r.h >= 2).collect();
        // Same size: the one centred nearest the cursor first. Rows of a list are all
        // the same size, and one scrolled under a sticky header can still report
        // a rect overlapping the row the cursor is really on.
        let off_centre = |r: &Rect| {
            let (dx, dy) = ((2 * r.x + r.w - 2 * x) as i64, (2 * r.y + r.h - 2 * y) as i64);
            dx * dx + dy * dy
        };
        // Same rect from two sources: it is fine only if both say so.
        v.sort_by_key(|(r, f)| (r.area(), off_centre(r), r.x, r.y, r.w, *f));
        // A same-sized neighbour (the overlapping row) is a sibling, not a step out.
        v.dedup_by(|b, a| a.0.w == b.0.w && a.0.h == b.0.h);
        // Largest last: everything was clipped to the window, so that is the window.
        let default = if whole { v.len().saturating_sub(1) } else { v.iter().position(|(_, f)| !f).unwrap_or(0) };
        (v.into_iter().map(|(r, _)| r).collect(), default)
    }

    /// Choosing a target (nothing selected or being dragged yet).
    fn choosing(&self) -> bool {
        !self.annotating && self.sel.is_none() && matches!(self.drag, Drag::None)
    }

    fn detect(&self, x: i32, y: i32) -> Option<Rect> {
        let (c, d) = self.candidates(x, y);
        c.get(d).copied()
    }

    /// Re-picks the hovered target; returns whether it is the outermost one.
    /// A new window under the cursor starts at its default target, not zoomed out.
    fn sync_hover_win(&mut self, x: i32, y: i32) {
        let win = self.wins.iter().find(|w| w.rect.contains(x, y)).map(|w| w.hwnd);
        if win != self.hover_win {
            self.hover_win = win;
            self.hover_outer = false;
            self.hover_step = 0;
        }
    }

    /// Picks the hover for the current cursor if the mouse moved since the last pick.
    fn refresh_hover(&mut self) {
        if std::mem::take(&mut self.hover_stale) && self.choosing() {
            let (x, y) = self.cursor;
            self.update_hover(x, y);
        }
    }

    fn update_hover(&mut self, x: i32, y: i32) -> bool {
        self.sync_hover_win(x, y);
        let (c, d) = self.candidates(x, y);
        let base = c.get(d).copied();
        if base != self.hover_base {
            self.hover_base = base;
            self.hover_step = 0;
        }
        if c.is_empty() {
            // Nothing to snap to (snapping off, or no window under the cursor).
            self.hover = None;
            self.hover_step = 0;
            return false;
        }
        let last = c.len() as i32 - 1;
        if self.hover_outer {
            self.hover_step = last - d as i32;
        }
        let i = (d as i32 + self.hover_step).clamp(0, last);
        self.hover_step = i - d as i32;
        self.hover = c.get(i as usize).copied();
        i == last
    }

    // ---------- selection & toolbar ----------

    /// The monitor's work area (without the taskbar) at a point, so the toolbar and
    /// options strip are never hidden behind it.
    fn work_at(&self, x: i32, y: i32) -> Rect {
        let i = self.mons.iter().position(|m| m.contains(x, y)).unwrap_or(0);
        self.works.get(i).copied().filter(|w| w.w > 0 && w.h > 0).unwrap_or_else(|| self.monitor_at(x, y))
    }

    /// Height (with its gap) kept free beside the toolbar for the options strip.
    fn panel_room(&self) -> f32 {
        (28.0 + OPT_PAD_Y * 2.0 + OPT_GAP) * self.scale
    }

    fn place_toolbar(&mut self) {
        let Some(sel) = self.sel else { return };
        let (cx, cy) = sel.center();
        let work = self.work_at(cx, cy);
        let (w, h) = self.bar.size();
        let gap = 8.0 * self.scale;
        let room = self.panel_room();
        let (top, bottom) = (work.y as f32 + 4.0, work.bottom() as f32 - 4.0);
        let x = (cx as f32 - w / 2.0).clamp(work.x as f32 + 4.0, (work.right() as f32 - w - 4.0).max(work.x as f32));
        // Below the selection with the strip under it, else above with the strip over
        // it, else inside the selection's bottom edge with the strip above.
        let below = sel.bottom() as f32 + gap;
        let above = sel.y as f32 - gap - h;
        let y = if below + h + room <= bottom {
            below
        } else if above - room >= top {
            above
        } else {
            (sel.bottom() as f32 - h - gap).min(bottom - h).max(top + room)
        };
        self.bar.x = x;
        self.bar.y = y;
    }

    fn popup_origin(&self) -> (f32, f32) {
        let r = self.bar.rect();
        let below = r.bottom() as f32 + 6.0 * self.scale;
        let work = self.work_at(r.x, r.y);
        let ph = 90.0 * self.scale;
        if below + ph < work.bottom() as f32 { (r.x as f32, below) } else { (r.x as f32, r.y as f32 - ph - 6.0 * self.scale) }
    }

    /// The tool whose options the strip shows: the selected item's, else the current
    /// tool's. `None` when that tool has no options (pointer, eraser, stickers).
    fn opt_tool(&self) -> Option<Tool> {
        let t = self.editing.or(self.selected).and_then(|i| self.items.get(i)).and_then(|it| it.tool()).unwrap_or(self.tool);
        (!options_for(t).is_empty()).then_some(t)
    }

    /// The panel holding the options strip: under the toolbar when the toolbar is
    /// below the selection and there is room, otherwise over it. Always on screen.
    fn options_panel(&self) -> Option<Rect> {
        if !self.annotating {
            return None;
        }
        let tool = self.opt_tool()?;
        let s = self.scale;
        let strip = OptionsBar { x: 0.0, y: 0.0, scale: s, tool: Some(tool), hover: None }.width();
        let (w, h) = (strip + OPT_PAD_X * 2.0 * s, (28.0 + OPT_PAD_Y * 2.0) * s);
        let bar = self.bar.rect();
        let work = self.work_at(bar.x, bar.y);
        let gap = OPT_GAP * s;
        let (top, bottom) = (work.y as f32 + 4.0, work.bottom() as f32 - 4.0);
        let below = bar.bottom() as f32 + gap;
        let above = bar.y as f32 - gap - h;
        let bar_below_sel = self.sel.is_none_or(|sel| bar.y >= sel.bottom());
        let y = if bar_below_sel && below + h <= bottom {
            below
        } else if above >= top {
            above
        } else {
            below
        };
        let y = y.min(bottom - h).max(top);
        let x = (bar.x as f32).clamp(work.x as f32 + 4.0, (work.right() as f32 - w - 4.0).max(work.x as f32));
        Some(Rect::new(x as i32, y as i32, w.ceil() as i32, h.ceil() as i32))
    }

    fn options_bar(&self) -> OptionsBar {
        let s = self.scale;
        let (x, y, tool) = match self.options_panel() {
            Some(p) => (p.x as f32 + OPT_PAD_X * s, p.y as f32 + OPT_PAD_Y * s, self.opt_tool()),
            None => (0.0, 0.0, None),
        };
        OptionsBar { x, y, scale: s, tool, hover: self.opt_hover }
    }

    fn values(&self) -> Values {
        Values { style: self.style, opts: self.opts, marker: self.marker }
    }

    /// Stores new option values (shared with the editor) and restyles the selected item.
    fn set_values(&mut self, v: Values, changed: Opt) {
        if v == self.values() {
            return;
        }
        self.style = v.style;
        self.opts = v.opts;
        self.marker = v.marker;
        v.save();
        let Some(i) = self.editing.or(self.selected).filter(|&i| i < self.items.len()) else { return };
        let before = self.items.clone();
        v.apply_to_item(changed, &mut self.items[i]);
        if self.items != before {
            if self.editing.is_none() {
                self.history.push(before);
            }
            self.comp_dirty = true;
        }
    }

    /// Selecting an item shows its options, so editing them restyles it.
    fn select(&mut self, i: Option<usize>) {
        self.selected = i;
        if let Some(it) = i.and_then(|i| self.items.get(i)) {
            let mut v = self.values();
            v.load_from_item(it);
            self.style = v.style;
            self.opts = v.opts;
            self.marker = v.marker;
        }
    }

    fn on_option(&mut self, h: OptHit) {
        let Some(tool) = self.opt_tool() else { return };
        let mut v = self.values();
        let o = match h {
            OptHit::Swatch(o) => {
                self.popup = if self.popup == Popup::Color(o) { Popup::None } else { Popup::Color(o) };
                return;
            }
            OptHit::Step(o, d) => {
                v.step(o, d, tool);
                o
            }
            OptHit::Choice(o, i) => {
                v.choose(o, i);
                o
            }
            OptHit::Toggle(o) => {
                v.toggle(o);
                o
            }
        };
        self.set_values(v, o);
    }

    fn on_color(&mut self, w: &mut Win, o: Opt, h: ColorHit) {
        self.popup = Popup::None;
        let mut v = self.values();
        let c = match h {
            ColorHit::Color(c) => Some(c),
            ColorHit::None => None,
            ColorHit::Custom => match super::dialogs::choose_color(w.hwnd, v.swatch(o).unwrap_or(Rgba::WHITE)) {
                Some(c) => Some(c),
                None => return,
            },
        };
        v.set_swatch(o, c);
        self.set_values(v, o);
    }

    /// Colour choices under (or, with the strip above the toolbar, over) a swatch.
    fn color_popup(&self, o: Opt) -> ColorPopup {
        let s = self.scale;
        let r = self.options_bar().swatch_rect(o).unwrap_or_default();
        let mut p = ColorPopup {
            x: (r.x as f32 - 8.0 * s).max(0.0),
            y: r.bottom() as f32 + 10.0 * s,
            scale: s,
            palette: self.settings.annotate.palette.clone(),
            allow_none: matches!(o, Opt::FillColor | Opt::Outline),
            none_label: if o == Opt::Outline { "No outline" } else { "No fill" },
            current: self.values().swatch(o),
        };
        let above = self.options_panel().is_some_and(|pr| pr.bottom() <= self.bar.rect().y);
        let mon = self.work_at(r.x, r.y);
        if above || p.rect().bottom() > mon.bottom() {
            p.y = r.y as f32 - 10.0 * s - p.rect().h as f32;
        }
        p
    }

    fn sticker_popup(&self) -> StickerPopup {
        let (x, y) = self.popup_origin();
        let bx = self.bar.button_bounds(Btn::Stickers).map(|r| r.x as f32 - 40.0 * self.scale).unwrap_or(x);
        StickerPopup { x: bx.max(0.0), y, scale: self.scale }
    }

    fn popup_rect(&self) -> Option<Rect> {
        match self.popup {
            Popup::None => None,
            Popup::Color(o) => Some(self.color_popup(o).rect()),
            Popup::Stickers => Some(self.sticker_popup().rect()),
        }
    }

    fn enter_annotate(&mut self, w: &Win) {
        self.annotating = true;
        self.scale = w.dpi_scale();
        self.bar = Toolbar::new(tool_buttons(), self.scale);
        self.bar.btn = QUICK_BTN;
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

    /// One Ctrl+Z (or Ctrl+Y, `redo`) inside the text box being edited. False when the
    /// box has no step left, so the window's own undo / redo should run instead.
    fn text_step(&mut self, redo: bool) -> bool {
        let Some(it) = self.editing.and_then(|i| self.items.get_mut(i)) else { return false };
        let done = if redo { self.text_undo.redo(it, &mut self.caret) } else { self.text_undo.undo(it, &mut self.caret) };
        if done {
            self.comp_dirty = true;
        }
        done
    }

    fn commit_text(&mut self) {
        self.finish_text();
    }

    /// Finishes the text being typed. An empty text item is dropped; any change from
    /// before editing (including dropping a re-opened item) becomes one undo step.
    /// True when a new, still-empty item was dropped without adding a step, so that
    /// dropping it was the whole of an undo.
    fn finish_text(&mut self) -> bool {
        let Some(i) = self.editing.take() else { return false };
        self.text_undo = TextUndo::default();
        let before = self.edit_before.take();
        let mut dropped = false;
        if i < self.items.len() {
            let empty = self.items[i].text_mut().is_some_and(|t| t.trim().is_empty());
            if empty && matches!(self.items[i].shape, Shape::Text { .. }) {
                self.items.remove(i);
                dropped = true;
            }
        }
        self.comp_dirty = true;
        if let Some(b) = before.filter(|b| *b != self.items) {
            self.history.push(b);
            return false;
        }
        dropped
    }

    /// Reopens the text or balloon item under (x, y) for typing (double-click with the pointer).
    fn edit_text_at(&mut self, x: i32, y: i32) -> bool {
        // Committing may drop an empty text item, so hit-test afterwards.
        self.commit_text();
        let p = self.doc_point(x, y);
        let Some(i) = ann::hit_test(&self.items, p) else { return false };
        let Some(caret) = self.items[i].caret_index_at(p) else { return false };
        self.select(Some(i));
        self.selected = None;
        self.editing = Some(i);
        self.edit_before = Some(self.items.clone());
        self.caret = TextCaret::at(caret);
        self.comp_dirty = true;
        true
    }

    /// A press on the text being edited moves the caret there and starts a drag-select
    /// (Shift extends the selection, a double-click selects the word). False when it missed.
    fn click_in_edit(&mut self, p: PointF, dbl: bool) -> bool {
        let Some(it) = self.editing.and_then(|i| self.items.get(i)) else { return false };
        if !it.bounds().contains(p) {
            return false;
        }
        if let Some(c) = it.caret_index_at(p) {
            if dbl {
                let (a, b) = it.word_at(c);
                self.caret = TextCaret { caret: b, anchor: a };
            } else {
                self.caret.place(c, super::shift());
                self.drag = Drag::SelectText;
            }
        }
        true
    }

    /// Tool options are saved as they change; this keeps the last region.
    fn remember(&self) {
        let Some(sel) = self.sel.map(|r| self.screen_rect(r)) else { return };
        app::update_settings(|s| s.last_region = Some(sel));
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
        // While typing, step back inside the text box first.
        if self.text_step(false) {
            return;
        }
        // Closing a new, empty text box is this undo; don't also undo the step before it.
        if self.finish_text() {
            return;
        }
        if let Some(prev) = self.history.undo(self.items.clone()) {
            self.items = prev;
            self.selected = None;
            self.comp_dirty = true;
        }
    }

    fn redo(&mut self) {
        if self.text_step(true) {
            return;
        }
        self.commit_text();
        if let Some(next) = self.history.redo(self.items.clone()) {
            self.items = next;
            self.selected = None;
            self.comp_dirty = true;
        }
    }

    fn mouse_down(&mut self, w: &mut Win, x: i32, y: i32) {
        self.refresh_hover();
        // Popups, the options strip and the toolbar first.
        if let Some(pr) = self.popup_rect() {
            if pr.contains(x, y) {
                match self.popup {
                    Popup::Color(o) => {
                        if let Some(h) = self.color_popup(o).hit(x, y) {
                            self.on_color(w, o, h);
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
            let was = self.popup;
            self.popup = Popup::None;
            // A click on the swatch that opened the popup just closes it.
            if let (Popup::Color(o), Some(OptHit::Swatch(o2))) = (was, self.options_bar().hit(x, y)) {
                if o == o2 {
                    return;
                }
            }
        }
        if self.annotating {
            if let Some(p) = self.options_panel().filter(|p| p.contains(x, y)) {
                let _ = p;
                if let Some(h) = self.options_bar().hit(x, y) {
                    self.on_option(h);
                }
                return;
            }
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
                if let Some(r) = self.hover.filter(|h| h.contains(x, y)).or_else(|| self.detect(x, y)) {
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
        if self.click_in_edit(self.doc_point(x, y), false) {
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
                self.select(Some(i));
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
        let v = self.values();
        let style = v.item_style(self.tool);
        if let Some(mut item) = Item::begin(self.tool, p, style, &self.items, sample) {
            v.finish_new_item(&mut item);
            let before = self.items.clone();
            self.items.push(item);
            self.selected = None;
            self.comp_dirty = true;
            if self.tool == Tool::Text {
                self.editing = Some(self.items.len() - 1);
                self.edit_before = Some(before);
                self.caret = TextCaret::at(0);
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
        self.opt_hover = (self.annotating && matches!(self.drag, Drag::None)).then_some((x, y));
        let shift = super::shift();
        match &mut self.drag {
            Drag::None => {
                if !self.annotating {
                    self.hover_stale = true; // picked in WM_PAINT
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
            Drag::SelectText => {
                let p = self.doc_point(x, y);
                if let Some(c) = self.editing.and_then(|i| self.items.get(i)).and_then(|it| it.caret_index_at(p)) {
                    self.caret.place(c, true);
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
        self.refresh_hover();
        unsafe {
            let _ = ReleaseCapture();
        }
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::NewSel { moved, x0, y0 } => {
                if moved {
                    let r = Rect::from_points(x0, y0, x, y);
                    self.on_selected(w, Rect::new(r.x, r.y, r.w + 1, r.h + 1));
                } else if let Some(h) = self.hover.filter(|h| h.contains(x, y)).or_else(|| self.detect(x, y)) {
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
                    self.caret = TextCaret::at(0);
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

    /// Developer mode (F9): saves what region snapping sees under the cursor to
    /// `<save dir>\debug`: the raw screenshot, the same with every snap candidate
    /// outlined, and a text report, so a bad snap can be reproduced offline.
    fn debug_dump(&self) {
        use std::fmt::Write as _;
        let dir = self.settings.save_dir().join("debug");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("F9 dump: cannot create {}: {e}", dir.display());
            return;
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let (x, y) = self.cursor;
        let (cands, default) = self.candidates(x, y);
        let mut r = String::new();
        let _ = writeln!(r, "cursor {x},{y} (screen {},{})", x + self.origin.0, y + self.origin.1);
        let _ = writeln!(r, "mode {:?}  snaps_controls {}  hover_step {}  hover_outer {}", self.mode, self.snaps_controls(), self.hover_step, self.hover_outer);
        let _ = writeln!(r, "hover {:?}", self.hover);
        match self.wins.iter().find(|w| w.rect.contains(x, y)) {
            Some(w) => {
                let _ = writeln!(r, "window {:?} hwnd {:#x}\n  rect {:?}\n  client {:?}\n  {} child windows", w.title, w.hwnd, w.rect, w.client, w.children.len());
                let uia = match self.uia.as_ref().map(|f| f.get(w.hwnd)) {
                    None => "off".to_string(),
                    Some(Fetch::Pending) => "pending".to_string(),
                    Some(Fetch::Failed) => "failed".to_string(),
                    Some(Fetch::Ready(els)) => format!("{} elements", els.len()),
                };
                let _ = writeln!(r, "  UI Automation: {uia}");
                let _ = writeln!(r, "  edge boxes (fine = not default) {:?}", edges::boxes_at(&self.shot, x, y, w.rect));
            }
            None => {
                let _ = writeln!(r, "no window under the cursor");
            }
        }
        let _ = writeln!(r, "candidates (innermost first; * = default, > = current hover):");
        for (i, c) in cands.iter().enumerate() {
            let _ = writeln!(r, "  {}{} {i}: {:?}", if i == default { '*' } else { ' ' }, if Some(*c) == self.hover { '>' } else { ' ' }, c);
        }
        let mut marked = self.shot.clone();
        let id = Transform::identity();
        for (i, c) in cands.iter().enumerate() {
            let (colour, width) = if Some(*c) == self.hover {
                (Rgba::rgb(255, 40, 40), 3.0)
            } else if i == default {
                (Rgba::rgb(40, 220, 80), 2.0)
            } else {
                (Rgba::YELLOW, 1.0)
            };
            if let Some(p) = snapit_gfx::tiny_skia::Rect::from_xywh(c.x as f32 + 0.5, c.y as f32 + 0.5, c.w as f32 - 1.0, c.h as f32 - 1.0).map(PathBuilder::from_rect) {
                marked.stroke_path(&p, &colour.paint(), &Stroke { width, ..Default::default() }, id, None);
            }
        }
        // Cursor crosshair.
        let mut pb = PathBuilder::new();
        pb.move_to(x as f32 - 8.0, y as f32 + 0.5);
        pb.line_to(x as f32 + 9.0, y as f32 + 0.5);
        pb.move_to(x as f32 + 0.5, y as f32 - 8.0);
        pb.line_to(x as f32 + 0.5, y as f32 + 9.0);
        if let Some(p) = pb.finish() {
            marked.stroke_path(&p, &Rgba::rgb(255, 0, 255).paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
        let base = dir.join(format!("snap_{stamp}"));
        let raw = base.with_extension("raw.png");
        let mark = base.with_extension("marked.png");
        let txt = base.with_extension("txt");
        let res = snapit_gfx::io::save(&self.shot, &raw, snapit_gfx::io::Format::Png, 90)
            .and_then(|_| snapit_gfx::io::save(&marked, &mark, snapit_gfx::io::Format::Png, 90))
            .and_then(|_| std::fs::write(&txt, &r).map_err(|e| e.to_string()));
        eprint!("F9 dump{}\n{r}", match res {
            Ok(()) => format!(" saved to {}", txt.display()),
            Err(e) => format!(" FAILED: {e}"),
        });
    }

    fn key_down(&mut self, w: &mut Win, vk: u16) {
        self.refresh_hover();
        let ctrl = super::ctrl();
        if self.editing.is_some() {
            if ctrl && matches!(vk, 0x5A | 0x59) {
                // Falls through to undo / redo, which step through the box's own
                // history first.
            } else {
                if vk == 0x1B {
                    self.commit_text(); // Esc
                } else if let Some(it) = self.editing.and_then(|i| self.items.get_mut(i)) {
                    if self.caret.key_down(it, &mut self.text_undo, vk, ctrl, super::shift()) {
                        self.comp_dirty = true;
                    }
                }
                return;
            }
        }
        match vk {
            0x78 if self.settings.developer_mode => self.debug_dump(), // F9
            0x1B => {
                if self.popup != Popup::None {
                    self.popup = Popup::None;
                } else {
                    w.close();
                }
            }
            0x0D if self.annotating || self.sel.is_some() => self.finish(w, Finish::Tasks),
            // Ctrl held: snap to whole windows (see `candidates`).
            0x11 if self.choosing() => {
                let (x, y) = self.cursor;
                self.update_hover(x, y);
            }
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
        let Some(it) = self.items.get_mut(i) else { return };
        match self.caret.char_input(it, &mut self.text_undo, c) {
            Typed::Edited => self.comp_dirty = true,
            Typed::Commit => self.commit_text(),
            Typed::Ignored => {}
        }
    }

    fn wheel(&mut self, delta: i16) {
        self.refresh_hover();
        if !self.annotating {
            // While choosing: step out to the enclosing element, or back in.
            if matches!(self.drag, Drag::None) && self.sel.is_none() {
                let (x, y) = self.cursor;
                self.sync_hover_win(x, y);
                if delta > 0 {
                    self.hover_step += 1;
                } else {
                    self.hover_outer = false;
                    self.hover_step -= 1;
                }
                let outermost = self.update_hover(x, y);
                // Scrolled all the way out: keep picking whole windows as the cursor moves.
                if delta > 0 && outermost {
                    self.hover_outer = true;
                }
            }
            return;
        }
        let Some(tool) = self.opt_tool() else { return };
        // Over a number in the options strip the wheel changes it; elsewhere it
        // changes the tool's size (text size for text tools, else width).
        let (x, y) = self.cursor;
        let o = self.options_bar().stepper_at(x, y).or_else(|| {
            let opts = options_for(tool);
            [Opt::FontSize, Opt::Width].into_iter().find(|o| opts.contains(o))
        });
        let Some(o) = o else { return };
        let mut v = self.values();
        if v.step(o, if delta > 0 { 1 } else { -1 }, tool) {
            self.set_values(v, o);
        }
    }

    fn cursor_shape(&self) -> windows::core::PCWSTR {
        let (x, y) = self.cursor;
        if self.annotating {
            let over = |r: Option<Rect>| r.is_some_and(|r| r.contains(x, y));
            if self.bar.rect().contains(x, y) || over(self.popup_rect()) || over(self.options_panel()) {
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
                    v.push(it.outline().to_rect().inflate((4.0 * s) as i32 + 4));
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
            if let Some(p) = self.options_panel() {
                // Room for the strip's tooltips, which hang below it.
                let (tip_w, tip_h) = ((120.0 * s) as i32, (40.0 * s) as i32);
                v.push(Rect::new(p.x - tip_w, p.y - 4, p.w + tip_w * 2, p.h + tip_h + 8));
            }
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

        if self.hover_stale {
            // Make sure a frame comes, so WM_PAINT picks the new hover.
            let (x, y) = self.cursor;
            dirty.push(Rect::new(x, y, 1, 1));
        }
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
                let b = it.outline();
                if let Some(p) = b.skia().map(PathBuilder::from_rect) {
                    let dash = Stroke { width: 1.0 * s, dash: StrokeDash::new(vec![4.0 * s, 3.0 * s], 0.0), ..Default::default() };
                    pm.stroke_path(&p, &Rgba::WHITE.paint(), &dash, id, None);
                }
            }
            if let Some(it) = self.editing.and_then(|i| self.items.get(i)) {
                draw_caret(pm, it, self.caret, t.accent, s);
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
            // The options panel first, so the toolbar's tooltips draw over it.
            if let Some(p) = self.options_panel() {
                let (x, y, pw, ph) = (p.x as f32, p.y as f32, p.w as f32, p.h as f32);
                if let Some(sh) = rounded_rect(x + 1.0, y + 3.0 * s, pw, ph, 9.0 * s) {
                    pm.fill_path(&sh, &Rgba::new(0, 0, 0, 60).paint(), FillRule::Winding, id, None);
                }
                if let Some(bg) = rounded_rect(x, y, pw, ph, 9.0 * s) {
                    pm.fill_path(&bg, &t.panel.paint(), FillRule::Winding, id, None);
                    pm.stroke_path(&bg, &t.border.paint(), &thin, id, None);
                }
                self.options_bar().draw(pm, t, &self.values());
            }
            self.bar.draw(
                pm,
                t,
                |b| match b {
                    Btn::Tool(x) => x == tool,
                    Btn::Stickers => matches!(tool, Tool::Sticker(_)) || popup == Popup::Stickers,
                    _ => false,
                },
                None,
            );
            match self.popup {
                Popup::Color(o) => self.color_popup(o).draw(pm, t),
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
    let fc = st.fill_color;
    (fc.r, fc.g, fc.b, fc.a, f(st.corner_radius), st.dash, st.shadow, st.heads, st.step_type, f(st.strength)).hash(&mut h);
    std::mem::discriminant(&it.shape).hash(&mut h);
    match &it.shape {
        Shape::Line(a, b) | Shape::Arrow(a, b) => (f(a.x), f(a.y), f(b.x), f(b.y)).hash(&mut h),
        Shape::Pen(p) | Shape::FreeArrow(p) => {
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
        Shape::Spotlight { r, dim, ellipse } => (f(r.x0), f(r.y0), f(r.x1), f(r.y1), dim, ellipse).hash(&mut h),
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

fn draw_caret(pm: &mut PixmapMut, it: &Item, caret: TextCaret, accent: Rgba, s: f32) {
    let (lo, hi) = caret.range();
    for r in it.selection_rects(lo, hi) {
        if let Some(rc) = r.skia() {
            pm.fill_rect(rc, &accent.with_alpha(90).paint(), Transform::identity(), None);
        }
    }
    let Some((x, y, h)) = it.caret_point(caret.caret) else { return };
    let x = x + 1.0;
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
        if msg == super::WM_SNAPIT_THEME {
            self.theme = super::theme::current();
            self.pending = vec![self.full()];
        }
        if self.uia.is_none() && self.wants_uia() {
            let f = Fetcher::new(w.hwnd.0 as isize, WM_OVERLAY_UIA);
            // Start with the window under the cursor so it is likely ready by the first hover.
            let (x, y) = self.cursor;
            if let Some(win) = self.wins.iter().find(|wi| wi.rect.contains(x, y)) {
                f.get(win.hwnd);
            }
            self.uia = Some(f);
        }
        match msg {
            WM_OVERLAY_UIA => {
                let (x, y) = self.cursor;
                let under = self.wins.iter().find(|wi| wi.rect.contains(x, y)).map(|wi| wi.hwnd);
                if under == Some(wp.0 as isize) && !self.annotating && self.sel.is_none() && matches!(self.drag, Drag::None) {
                    self.update_hover(x, y);
                    self.redraw(w);
                }
                Some(LRESULT(0))
            }
            WM_PAINT => {
                if self.hover_stale {
                    // Pick the hover for where the cursor is now, then add whatever
                    // that changed to this paint (the update region is read below).
                    self.refresh_hover();
                    self.redraw(w);
                }
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
                let pointer = self.annotating && self.tool == Tool::Pointer && inside;
                if self.annotating && self.click_in_edit(self.doc_point(x, y), true) {
                    self.redraw(w); // a word in the text being typed is selected
                } else if pointer && self.edit_text_at(x, y) {
                    self.redraw(w);
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
                    self.update_hover(x, y);
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
            WM_KEYUP => {
                if wp.0 == 0x11 && self.choosing() {
                    let (x, y) = self.cursor;
                    self.update_hover(x, y);
                    self.redraw(w);
                }
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.char_input(wp.0 as u32);
                self.redraw(w);
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                app::post(Command::OverlayClosed);
                None
            }
            _ => None,
        }
    }
}
