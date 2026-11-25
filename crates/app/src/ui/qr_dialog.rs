//! "Make QR code" studio dialog: type text, see the live QR code, then copy, save or pin it.
//!
//! Keyboard: Tab / Shift+Tab move between the text box and the buttons, Space or Enter press
//! a button, Ctrl+Enter copies from anywhere, Esc closes. Screen readers see the text box and
//! the buttons through `a11y`.

use std::cell::Cell;

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, FilterQuality, Pixmap, PixmapPaint, Stroke, Transform};
use snapit_gfx::{text, Rect, Rgba};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, LoadCursorW, SetCursor, SetForegroundWindow, HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM, WM_CHAR,
    WM_DESTROY, WM_ERASEBKGND, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_SIZE,
    WM_SYSKEYDOWN, WINDOW_EX_STYLE, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU,
};

use super::a11y::{vk::*, Access, Action as AxAction, Node, Role};
use super::chrome::Chrome;
use super::icons::Icon;
use super::text_input::place_ime;
use super::theme::{self, Theme};
use super::widgets::{self, BtnStyle, TextInput};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QrBtn {
    Copy,
    Save,
    Pin,
}

/// Accessibility / focus keys.
const K_INPUT: u32 = 1;
const K_HINT: u32 = 2;

fn btn_key(b: QrBtn) -> u32 {
    10 + b as u32
}

fn btn_of(k: u32) -> Option<QrBtn> {
    [QrBtn::Copy, QrBtn::Save, QrBtn::Pin].into_iter().find(|b| btn_key(*b) == k)
}

struct QrDialog {
    theme: Theme,
    scale: f32,
    chrome: Chrome,
    input: TextInput,
    code: Option<Pixmap>,
    bb: Option<Backbuffer>,
    client: (i32, i32),
    hover_btn: Option<QrBtn>,
    hover_input: bool,
    ax: Access,
}

pub fn open() {
    let existing = OPEN.with(|o| o.get());
    if existing != 0 {
        unsafe {
            let _ = SetForegroundWindow(HWND(existing as *mut _));
        }
        return;
    }
    let mons = snapit_capture::monitors();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    let (work, sc) = m.map(|m| (m.work, m.scale())).unwrap_or((Rect::new(0, 0, 1280, 800), 1.0));
    let (w, h) = ((460.0 * sc) as i32, (620.0 * sc) as i32);
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));

    let clip = if super::headless() { None } else { crate::clipboard::get_text() };
    let initial = clip.filter(|t| !t.is_empty() && t.len() < 2000).unwrap_or_else(|| "https://".into());
    let mut input = TextInput::new(initial, "Enter a URL or text to encode…", false, true);
    // Typing replaces the suggestion; arrow keys keep it.
    input.select_all();
    let mut d = QrDialog {
        theme: theme::current(),
        scale: sc,
        chrome: Chrome::new(false, true, false),
        input,
        code: None,
        bb: None,
        client: (w, h),
        hover_btn: None,
        hover_input: false,
        ax: Access { focus: Some(K_INPUT), ..Access::default() },
    };
    d.regenerate();

    if let Ok(hwnd) = super::create(
        "SnapItQr",
        WINDOW_EX_STYLE(0),
        WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        "Make QR code",
        r,
        None,
        Box::new(d),
    ) {
        Chrome::init(hwnd);
        OPEN.with(|o| o.set(hwnd.0 as isize));
        super::show(hwnd, true);
    }
}

impl QrDialog {
    fn input_focused(&self) -> bool {
        self.ax.focus == Some(K_INPUT)
    }

    fn input_rect(&self) -> Rect {
        let s = self.scale;
        let pad = (20.0 * s) as i32;
        let top = self.chrome.height(s) + (38.0 * s) as i32;
        Rect::new(pad, top, (self.client.0 - pad * 2).max(40), (84.0 * s) as i32)
    }

    fn buttons(&self) -> [(QrBtn, Rect, &'static str, Icon, BtnStyle); 3] {
        let s = self.scale;
        let pad = (20.0 * s) as i32;
        let gap = (10.0 * s) as i32;
        let bh = (36.0 * s) as i32;
        let by = self.client.1 - pad - bh;
        let bw = ((self.client.0 - pad * 2 - gap * 2) / 3).max(60);
        [
            (QrBtn::Copy, Rect::new(pad, by, bw, bh), "Copy", Icon::Copy, BtnStyle::Primary),
            (QrBtn::Save, Rect::new(pad + bw + gap, by, bw, bh), "Save…", Icon::Save, BtnStyle::Secondary),
            (QrBtn::Pin, Rect::new(pad + (bw + gap) * 2, by, self.client.0 - pad - (pad + (bw + gap) * 2), bh), "Pin", Icon::Pin, BtnStyle::Secondary),
        ]
    }

    fn regenerate(&mut self) {
        let t = self.input.value.trim();
        self.code = if t.is_empty() { None } else { crate::tools::make_qr(t, 8) };
    }

    fn render(&mut self, hwnd: HWND) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        self.client = (rc.right.max(1), rc.bottom.max(1));
        self.scale = super::dpi_scale(hwnd);
        Backbuffer::ensure(&mut self.bb, self.client.0 as u32, self.client.1 as u32);
        let Some(mut bb) = self.bb.take() else { return };
        self.ax.begin();
        {
            let mut pm = bb.pixmap();
            let t = self.theme;
            let s = self.scale;
            let id = Transform::identity();
            pm.fill(t.bg.color());

            // Integrated custom titlebar.
            self.chrome.draw(&mut pm, &t, hwnd, self.client.0, s, "QR Code Studio", None, None);

            let pad = 20.0 * s;
            let lbl_y = self.chrome.height(s) as f32 + 14.0 * s;
            text::draw(&mut pm, "Content", pad, lbl_y, 12.5 * s, true, t.subtext, None);
            let count = format!("{} chars", self.input.value.chars().count());
            let cl = text::layout(&count, 11.5 * s, false);
            text::draw(&mut pm, &count, self.client.0 as f32 - pad - cl.width, lbl_y + 1.0 * s, 11.5 * s, false, t.subtext, None);

            // Text input.
            let ir = self.input_rect();
            let focused = self.input_focused();
            self.input.draw(&mut pm, &t, ir, focused, self.hover_input, s);
            self.ax.push(
                Node::new(K_INPUT, ir, Role::Edit, "Content to encode").value(self.input.value.clone()).help("The QR code updates as you type."),
            );

            // QR preview card.
            let btns = self.buttons();
            let card_top = ir.bottom() as f32 + 18.0 * s;
            let card_bot = btns[0].1.y as f32 - 20.0 * s;
            let card_h = (card_bot - card_top).max(120.0 * s);
            let card = Rect::new(pad as i32, card_top as i32, (self.client.0 as f32 - pad * 2.0) as i32, card_h as i32);
            widgets::draw_card(&mut pm, &t, card, 12.0, s);

            match &self.code {
                Some(code) => {
                    let avail = (card.w.min(card.h) as f32 - 44.0 * s).max(60.0 * s);
                    let k = (avail / code.width() as f32).floor().max(1.0).min(avail / code.width() as f32);
                    let size = code.width() as f32 * k;
                    let frame_pad = 12.0 * s;
                    let fx = card.x as f32 + (card.w as f32 - size) / 2.0;
                    let fy = card.y as f32 + (card.h as f32 - size) / 2.0;
                    if let Some(sh) = rounded_rect(fx - frame_pad, fy - frame_pad + 3.0 * s, size + frame_pad * 2.0, size + frame_pad * 2.0, 10.0 * s) {
                        pm.fill_path(&sh, &t.shadow.paint(), FillRule::Winding, id, None);
                    }
                    if let Some(wp) = rounded_rect(fx - frame_pad, fy - frame_pad, size + frame_pad * 2.0, size + frame_pad * 2.0, 10.0 * s) {
                        pm.fill_path(&wp, &Rgba::WHITE.paint(), FillRule::Winding, id, None);
                        pm.stroke_path(&wp, &t.border_subtle.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
                    }
                    pm.draw_pixmap(
                        0,
                        0,
                        code.as_ref(),
                        &PixmapPaint { quality: FilterQuality::Nearest, ..Default::default() },
                        Transform::from_row(k, 0.0, 0.0, k, fx.round(), fy.round()),
                        None,
                    );
                }
                None => {
                    let msg = "Type text or a link above to generate a QR code";
                    let l = text::layout(msg, 13.0 * s, false);
                    text::draw(
                        &mut pm,
                        msg,
                        card.x as f32 + (card.w as f32 - l.width) / 2.0,
                        card.y as f32 + (card.h as f32 - l.height) / 2.0,
                        13.0 * s,
                        false,
                        t.subtext,
                        None,
                    );
                }
            }
            let status = if self.code.is_some() { "QR code ready" } else { "No QR code: the content is empty" };
            self.ax.push(Node::new(K_HINT, card, Role::Text, status).live());

            // Footer buttons.
            let disabled = self.code.is_none();
            for (b, r, lbl, ic, st) in btns {
                widgets::draw_button(&mut pm, &t, r, lbl, Some(ic), st, self.hover_btn == Some(b), disabled, s);
                let name = match b {
                    QrBtn::Copy => "Copy QR code",
                    QrBtn::Save => "Save QR code",
                    QrBtn::Pin => "Pin QR code",
                };
                let help = if b == QrBtn::Copy { "Ctrl+Enter" } else { "" };
                self.ax.push(Node::new(btn_key(b), r, Role::Button, name).enabled(!disabled).help(help));
            }

            let chrome_nodes = self.chrome.a11y_nodes(hwnd, self.client.0, s);
            self.ax.nodes.extend(chrome_nodes);
            if let Some(r) = self.ax.ring_rect() {
                widgets::draw_focus_ring(&mut pm, &t, r, 8.0, s);
            }
        }
        self.bb = Some(bb);
        self.ax.publish(hwnd);
    }

    fn run_action(&self, hwnd: HWND, btn: QrBtn) {
        let Some(img) = self.code.clone() else { return };
        match btn {
            QrBtn::Copy => {
                let _ = crate::clipboard::set_image(&img);
                app::post(Command::Notify { title: "QR code copied".into(), body: format!("{} × {}", img.width(), img.height()) });
            }
            QrBtn::Save => {
                let s = app::settings();
                if let Some(p) = super::dialogs::save_image(Some(hwnd), &s.save_dir().join("QR code.png")) {
                    let fmt = snapit_gfx::io::Format::from_path(&p).unwrap_or(snapit_gfx::io::Format::Png);
                    if let Err(e) = snapit_gfx::io::save(&img, &p, fmt, s.jpeg_quality) {
                        super::dialogs::info(Some(hwnd), "Snap It", &e);
                    }
                }
            }
            QrBtn::Pin => app::post(Command::Pin { image: img, at: None }),
        }
    }
}

impl Handler for QrDialog {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn a11y_action(&mut self, w: &mut Win, k: u32, action: AxAction) {
        if Chrome::a11y_invoke(w.hwnd, k) {
            return;
        }
        match (k, action) {
            (K_INPUT, AxAction::SetValue(v)) => {
                self.ax.focus = Some(K_INPUT);
                self.input.set(v);
                self.regenerate();
            }
            (K_INPUT, AxAction::Focus) => self.ax.focus = Some(K_INPUT),
            (k, AxAction::Focus) if btn_of(k).is_some() => self.ax.focus = Some(k),
            (k, AxAction::Invoke) => {
                if let Some(b) = btn_of(k) {
                    self.ax.focus = Some(k);
                    self.run_action(w.hwnd, b);
                }
            }
            _ => {}
        }
        w.invalidate();
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
            WM_MOUSELEAVE => {
                self.chrome.mouse_leave();
                self.hover_btn = None;
                self.hover_input = false;
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_NCHITTEST => Some(self.chrome.hit_test(w.hwnd, lp, self.scale, &[])),
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
                let cur = if self.hover_input {
                    IDC_IBEAM
                } else if self.hover_btn.is_some() || self.chrome.hover.is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
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
                dirty |= self.input.mouse_move(x, y).handled();
                let hi = self.input_rect().contains(x, y);
                if hi != self.hover_input {
                    self.hover_input = hi;
                    dirty = true;
                }
                let hb = self.buttons().into_iter().find(|(_, r, _, _, _)| r.contains(x, y)).map(|(b, _, _, _, _)| b);
                if hb != self.hover_btn {
                    self.hover_btn = hb;
                    dirty = true;
                }
                if dirty {
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = super::lparam_xy(lp);
                if self.chrome.press(self.client.0, self.scale, x, y) {
                    return Some(LRESULT(0));
                }
                if self.input_rect().contains(x, y) {
                    self.ax.set_focus(Some(K_INPUT));
                    self.input.mouse_down(x, y, super::shift(), msg == WM_LBUTTONDBLCLK);
                    unsafe {
                        SetCapture(w.hwnd);
                    }
                }
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = super::lparam_xy(lp);
                unsafe {
                    let _ = ReleaseCapture();
                }
                if self.input.dragging() {
                    self.input.mouse_up();
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                if self.chrome.release(w.hwnd, self.client.0, self.scale, x, y) {
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                if let Some((b, _, _, _, _)) = self.buttons().into_iter().find(|(_, r, _, _, _)| r.contains(x, y)) {
                    self.ax.set_focus(Some(btn_key(b)));
                    self.run_action(w.hwnd, b);
                }
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                let vk = wp.0 as u32;
                let (ctrl, shift) = (super::ctrl(), super::shift());
                if vk == VK_ESCAPE {
                    w.close();
                    return Some(LRESULT(0));
                }
                if vk == VK_TAB && !ctrl {
                    self.ax.focus = self.ax.next(shift);
                    self.ax.ring = true;
                    if self.ax.focus == Some(K_INPUT) {
                        self.input.select_all();
                    }
                    w.invalidate();
                    return Some(LRESULT(0));
                }
                if vk == VK_RETURN && ctrl {
                    self.run_action(w.hwnd, QrBtn::Copy);
                    return Some(LRESULT(0));
                }
                if self.input_focused() {
                    let c = self.input.on_key(vk, ctrl, shift);
                    if c.edited() {
                        self.regenerate();
                    }
                    if c.handled() {
                        w.invalidate();
                    }
                } else if let (VK_SPACE | VK_RETURN, Some(b)) = (vk, self.ax.focus.and_then(btn_of)) {
                    self.ax.ring = true;
                    self.run_action(w.hwnd, b);
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_SYSKEYDOWN => None,
            WM_CHAR => {
                // (AltGr arrives as Ctrl+Alt, so don't filter on Ctrl: on_char ignores the
                // control characters Ctrl+letter produces.)
                if self.input_focused() {
                    if self.input.on_char(wp.0 as u16).edited() {
                        self.regenerate();
                    }
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
                if let Some(p) = self.input_focused().then(|| self.input.caret_point()).flatten() {
                    place_ime(w.hwnd, p);
                }
                None
            }
            WM_DESTROY => {
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}
