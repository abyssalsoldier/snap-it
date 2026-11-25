//! "Make QR code": type text, see the code, then copy, save or pin it.

use std::cell::Cell;

use snapit_gfx::tiny_skia::{Pixmap, PixmapPaint, Transform, FilterQuality};
use snapit_gfx::{text, Rect};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, HFONT};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, SendMessageW,
    SetForegroundWindow, BS_PUSHBUTTON, EN_CHANGE, ES_AUTOVSCROLL,
    ES_MULTILINE, ES_WANTRETURN, HMENU, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DESTROY,
    WM_ERASEBKGND, WM_PAINT, WM_SETFONT, WM_SHOWWINDOW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::theme::{self, Theme};
use super::{backbuffer, Backbuffer, Handler, Win};
use crate::app::{self, Command};

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

const ID_TEXT: u16 = 1;
const ID_COPY: u16 = 2;
const ID_SAVE: u16 = 3;
const ID_PIN: u16 = 4;

struct QrDialog {
    theme: Theme,
    scale: f32,
    edit: HWND,
    code: Option<Pixmap>,
    bb: Option<Backbuffer>,
    built: bool,
    font: HFONT,
    nt: super::native_theme::NativeTheme,
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
    let (w, h) = ((460.0 * sc) as i32, (640.0 * sc) as i32);
    let r = Rect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h.min(work.h));
    let d = QrDialog { theme: theme::current(), scale: sc, edit: HWND::default(), code: None, bb: None, built: false, font: HFONT::default(), nt: super::native_theme::NativeTheme::new() };
    if let Ok(hwnd) = super::create("SnapItQr", WINDOW_EX_STYLE(0), WS_CAPTION | WS_SYSMENU, "Make QR code", r, None, Box::new(d)) {
        OPEN.with(|o| o.set(hwnd.0 as isize));
        super::show(hwnd, true);
    }
}

impl QrDialog {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale) as i32
    }

    fn build(&mut self, hwnd: HWND) {
        self.scale = super::dpi_scale(hwnd);
        unsafe {
            let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
            if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0, GetDpiForWindow(hwnd)).is_ok() {
                self.font = windows::Win32::Graphics::Gdi::CreateFontIndirectW(&ncm.lfMessageFont);
            }
        }
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let cw = rc.right;
        let mk = |me: &Self, class, text: &str, style: WINDOW_STYLE, ex, x, y, w, h, id: u16| unsafe {
            let c = CreateWindowExW(ex, class, &HSTRING::from(text), WS_CHILD | WS_VISIBLE | style, x, y, w, h, Some(hwnd), Some(HMENU(id as usize as *mut _)), None, None).unwrap_or_default();
            SendMessageW(c, WM_SETFONT, Some(WPARAM(me.font.0 as usize)), Some(LPARAM(1)));
            c
        };
        let pad = self.px(14);
        let clip = if super::headless() { None } else { crate::clipboard::get_text() };
        let initial = clip.filter(|t| !t.is_empty() && t.len() < 2000).unwrap_or_else(|| "https://".into());
        let ml = WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32);
        self.edit = mk(self, w!("EDIT"), &initial, ml | WS_VSCROLL | WS_TABSTOP | WS_BORDER, WS_EX_CLIENTEDGE, pad, pad, cw - pad * 2, self.px(70), ID_TEXT);
        let by = rc.bottom - self.px(44);
        let bw = (cw - pad * 4) / 3;
        for (i, (id, label)) in [(ID_COPY, "Copy"), (ID_SAVE, "Save…"), (ID_PIN, "Pin")].iter().enumerate() {
            mk(self, w!("BUTTON"), label, WINDOW_STYLE(BS_PUSHBUTTON as u32) | WS_TABSTOP, WINDOW_EX_STYLE(0), pad + i as i32 * (bw + pad), by, bw, self.px(30), *id);
        }
        self.regenerate();
        self.built = true;
    }

    fn text(&self) -> String {
        unsafe {
            let n = GetWindowTextLengthW(self.edit);
            let mut buf = vec![0u16; n as usize + 1];
            let got = GetWindowTextW(self.edit, &mut buf);
            String::from_utf16_lossy(&buf[..got.max(0) as usize]).replace("\r\n", "\n")
        }
    }

    fn regenerate(&mut self) {
        let t = self.text();
        self.code = if t.is_empty() { None } else { crate::tools::make_qr(&t, 8) };
    }
}

impl Handler for QrDialog {
    fn snapshot(&self) -> Option<Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
        if msg == super::WM_SNAPIT_THEME {
            self.theme = super::theme::current();
            self.nt.refresh();
            self.nt.style_children(w.hwnd);
        }
        match msg {
            WM_SHOWWINDOW if !self.built => {
                self.build(w.hwnd);
                self.nt.style_children(w.hwnd);
                None
            }
            // The text box and buttons are standard controls.
            m @ (windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT
            | windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC
            | windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORBTN) => self.nt.ctlcolor(m, wp),
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_PAINT => {
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(w.hwnd, &mut rc);
                }
                Backbuffer::ensure(&mut self.bb, rc.right.max(1) as u32, rc.bottom.max(1) as u32);
                if let Some(bb) = self.bb.as_mut() {
                    let mut pm = bb.pixmap();
                    pm.fill(self.theme.panel.color());
                    let s = self.scale;
                    let top = 100.0 * s;
                    let bottom = rc.bottom as f32 - 56.0 * s;
                    let avail = (bottom - top).min(rc.right as f32 - 28.0 * s);
                    match &self.code {
                        Some(code) => {
                            let k = (avail / code.width() as f32).floor().max(1.0).min(avail / code.width() as f32);
                            let size = code.width() as f32 * k;
                            let x = (rc.right as f32 - size) / 2.0;
                            pm.draw_pixmap(0, 0, code.as_ref(), &PixmapPaint { quality: FilterQuality::Nearest, ..Default::default() }, Transform::from_row(k, 0.0, 0.0, k, x.round(), top.round()), None);
                        }
                        None => {
                            let msg = "Type something above (too long for a QR code?)";
                            let l = text::layout(msg, 13.0 * s, false);
                            text::draw(&mut pm, msg, (rc.right as f32 - l.width) / 2.0, top + avail / 2.0, 13.0 * s, false, self.theme.subtext, None);
                        }
                    }
                }
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as u16;
                let code = ((wp.0 >> 16) & 0xFFFF) as u32;
                if id == ID_TEXT && code == EN_CHANGE {
                    self.regenerate();
                    unsafe {
                        let _ = InvalidateRect(Some(w.hwnd), None, false);
                    }
                    return Some(LRESULT(0));
                }
                let Some(img) = self.code.clone() else { return Some(LRESULT(0)) };
                match id {
                    ID_COPY => {
                        let _ = crate::clipboard::set_image(&img);
                        app::post(Command::Notify { title: "QR code copied".into(), body: format!("{} × {}", img.width(), img.height()) });
                    }
                    ID_SAVE => {
                        let s = app::settings();
                        if let Some(p) = super::dialogs::save_image(Some(w.hwnd), &s.save_dir().join("QR code.png")) {
                            let fmt = snapit_gfx::io::Format::from_path(&p).unwrap_or(snapit_gfx::io::Format::Png);
                            if let Err(e) = snapit_gfx::io::save(&img, &p, fmt, s.jpeg_quality) {
                                super::dialogs::info(Some(w.hwnd), "Snap It", &e);
                            }
                        }
                    }
                    ID_PIN => app::post(Command::Pin { image: img, at: None }),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                OPEN.with(|o| o.set(0));
                None
            }
            _ => None,
        }
    }
}
