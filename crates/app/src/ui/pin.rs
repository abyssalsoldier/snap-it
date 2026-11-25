//! Pin to screen: an always-on-top floating copy of an image.
//! Drag to move · wheel to zoom · Ctrl+wheel for opacity · double-click or Esc to close.

use snapit_gfx::tiny_skia::{FilterQuality, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};
use snapit_gfx::{Rect, Rgba};
use windows::core::HSTRING;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetWindowRect, LoadCursorW, SetCursor,
    SetLayeredWindowAttributes, SetWindowPos, TrackPopupMenu, HTCLIENT, IDC_SIZEALL,
    LWA_ALPHA, MF_POPUP, MF_SEPARATOR, MF_STRING, SWP_NOACTIVATE, SWP_NOZORDER,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_CHAR, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};

struct Pin {
    img: Pixmap,
    zoom: f32,
    opacity: u8,
    bb: Option<Backbuffer>,
    size: (i32, i32),
    drag: Option<(POINT, RECT)>,
}

pub fn open(image: Pixmap, at: Option<Rect>) {
    let (w, h) = (image.width() as i32, image.height() as i32);
    let r = match at {
        Some(r) => Rect::new(r.x, r.y, w, h),
        None => {
            let mut p = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
            Rect::new(p.x - w / 2, p.y - h / 2, w, h)
        }
    };
    // Keep at least part of it on a monitor.
    let vs = snapit_capture::virtual_screen();
    let r = Rect::new(r.x.clamp(vs.x - w + 40, vs.right() - 40), r.y.clamp(vs.y, vs.bottom() - 40), w, h);
    let pin = Pin { img: image, zoom: 1.0, opacity: 255, bb: None, size: (w, h), drag: None };
    match super::create("SnapItPin", WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED, WS_POPUP, "Snap It pin", r, None, Box::new(pin)) {
        Ok(hwnd) => {
            unsafe {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
            super::show(hwnd, false);
        }
        Err(e) => crate::log(&format!("pin: {e}")),
    }
}

const M_COPY: usize = 1;
const M_SAVE: usize = 2;
const M_EDIT: usize = 3;
const M_ACTUAL: usize = 4;
const M_CLOSE: usize = 5;
const M_OPACITY: usize = 10; // + percent/10

impl Pin {
    fn resize(&mut self, hwnd: HWND, zoom: f32, anchor: Option<POINT>) {
        let zoom = zoom.clamp(0.1, 8.0);
        let (w, h) = (((self.img.width() as f32) * zoom).round().max(16.0) as i32, ((self.img.height() as f32) * zoom).round().max(16.0) as i32);
        let mut wr = RECT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut wr);
        }
        // Zoom around the cursor (or the centre).
        let (ax, ay) = anchor.map(|p| (p.x, p.y)).unwrap_or(((wr.left + wr.right) / 2, (wr.top + wr.bottom) / 2));
        let fx = (ax - wr.left) as f32 / (wr.right - wr.left).max(1) as f32;
        let fy = (ay - wr.top) as f32 / (wr.bottom - wr.top).max(1) as f32;
        let x = ax - (w as f32 * fx) as i32;
        let y = ay - (h as f32 * fy) as i32;
        self.zoom = zoom;
        unsafe {
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn set_opacity(&mut self, hwnd: HWND, a: u8) {
        self.opacity = a.max(30);
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), self.opacity, LWA_ALPHA);
        }
    }

    fn render(&mut self) {
        let (w, h) = (self.size.0.max(1) as u32, self.size.1.max(1) as u32);
        Backbuffer::ensure(&mut self.bb, w, h);
        let Some(bb) = self.bb.as_mut() else { return };
        let mut pm = bb.pixmap();
        pm.fill(Rgba::rgb(40, 40, 40).color());
        let (sx, sy) = (w as f32 / self.img.width() as f32, h as f32 / self.img.height() as f32);
        let q = if self.zoom >= 1.5 { FilterQuality::Nearest } else { FilterQuality::Bicubic };
        pm.draw_pixmap(0, 0, self.img.as_ref(), &PixmapPaint { quality: q, ..Default::default() }, Transform::from_scale(sx, sy), None);
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.5, 0.5, w as f32 - 1.0, h as f32 - 1.0) {
            pm.stroke_path(&PathBuilder::from_rect(r), &Rgba::rgb(56, 152, 248).with_alpha(200).paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
    }

    fn menu(&mut self, w: &mut Win) {
        let id = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let add = |m, id: usize, s: &str| {
                let _ = AppendMenuW(m, MF_STRING, id, &HSTRING::from(s));
            };
            add(m, M_COPY, "Copy\tCtrl+C");
            add(m, M_SAVE, "Save as…");
            add(m, M_EDIT, "Open in editor");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            add(m, M_ACTUAL, "Actual size");
            if let Ok(o) = CreatePopupMenu() {
                for pct in [100, 80, 60, 40] {
                    add(o, M_OPACITY + pct / 10, &format!("{pct}%"));
                }
                let _ = AppendMenuW(m, MF_POPUP, o.0 as usize, &HSTRING::from("Opacity"));
            }
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            add(m, M_CLOSE, "Close\tEsc");
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, w.hwnd, None);
            let _ = DestroyMenu(m);
            r.0 as usize
        };
        match id {
            M_COPY => {
                let _ = crate::clipboard::set_image(&self.img);
            }
            M_SAVE => {
                let s = app::settings();
                if let Some(p) = super::dialogs::save_image(Some(w.hwnd), &s.save_dir().join(format!("Pinned.{}", s.format.ext()))) {
                    let fmt = snapit_gfx::io::Format::from_path(&p).unwrap_or(s.format);
                    if let Err(e) = snapit_gfx::io::save(&self.img, &p, fmt, s.jpeg_quality) {
                        super::dialogs::info(Some(w.hwnd), "Snap It", &e);
                    }
                }
            }
            M_EDIT => app::post(Command::OpenEditor { image: self.img.clone(), path: None }),
            M_ACTUAL => self.resize(w.hwnd, 1.0, None),
            M_CLOSE => w.close(),
            i if (M_OPACITY + 4..=M_OPACITY + 10).contains(&i) => self.set_opacity(w.hwnd, (((i - M_OPACITY) * 10) as f32 * 2.55) as u8),
            _ => {}
        }
    }
}

impl Handler for Pin {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_SIZE => {
                let (cw, ch) = ((lp.0 & 0xFFFF) as i32, ((lp.0 >> 16) & 0xFFFF) as i32);
                self.size = (cw, ch);
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_PAINT => {
                self.render();
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                unsafe {
                    if let Ok(c) = LoadCursorW(None, IDC_SIZEALL) {
                        SetCursor(Some(c));
                    }
                }
                Some(LRESULT(1))
            }
            WM_LBUTTONDOWN => {
                let mut p = POINT::default();
                let mut r = RECT::default();
                unsafe {
                    let _ = GetCursorPos(&mut p);
                    let _ = GetWindowRect(w.hwnd, &mut r);
                    SetCapture(w.hwnd);
                }
                self.drag = Some((p, r));
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                if let Some((p0, r0)) = self.drag {
                    let mut p = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                        let _ = SetWindowPos(w.hwnd, None, r0.left + p.x - p0.x, r0.top + p.y - p0.y, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
                    }
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.drag = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK | WM_MBUTTONUP => {
                w.close();
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                self.menu(w);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                if super::ctrl() {
                    let a = self.opacity as i32 + if delta > 0 { 20 } else { -20 };
                    self.set_opacity(w.hwnd, a.clamp(30, 255) as u8);
                } else {
                    let mut p = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                    }
                    let z = self.zoom * if delta > 0 { 1.15 } else { 1.0 / 1.15 };
                    self.resize(w.hwnd, z, Some(p));
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                match wp.0 as u16 {
                    0x1B => w.close(),
                    0x43 if super::ctrl() => {
                        let _ = crate::clipboard::set_image(&self.img);
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_CHAR => Some(LRESULT(0)),
            WM_DESTROY => None,
            _ => None,
        }
    }
}
