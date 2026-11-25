//! Custom integrated DWM window chrome (frameless extended titlebar).
//!
//! Keeps standard Win32 window styles (`WS_OVERLAPPEDWINDOW` / `WS_CAPTION | WS_SYSMENU`)
//! so Windows 11 rounded corners, drop shadow, Aero Snap, and taskbar animations work
//! natively while Snap It draws the entire header bar and window buttons.
//!
//! The Maximize button hit-tests as `HTMAXBUTTON` (so Windows 11 shows its Snap Layouts
//! flyout on hover); its press/release arrive as `WM_NCLBUTTON*` and are handled by
//! [`Chrome::nc_message`]. Minimize and Close are ordinary client-area buttons.
//!
//! Window owners must route messages as follows:
//! - `WM_NCCALCSIZE` -> [`Chrome::handle_nccalcsize`], `WM_NCHITTEST` -> [`Chrome::hit_test`]
//! - `WM_NCMOUSEMOVE` / `WM_NCMOUSELEAVE` / `WM_NCLBUTTON*` -> [`Chrome::nc_message`]
//! - `WM_MOUSEMOVE` -> [`Chrome::set_hover`], `WM_MOUSELEAVE` -> [`Chrome::mouse_leave`]
//! - `WM_LBUTTONDOWN` -> [`Chrome::press`], `WM_LBUTTONUP` -> [`Chrome::release`]

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, Pixmap, PixmapMut, PixmapPaint, Stroke, Transform};
use snapit_gfx::{text, Rect, Rgba};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, IsZoomed, PostMessageW, SetWindowPos, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
    HTCAPTION, HTCLIENT, HTLEFT, HTMAXBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    NCCALCSIZE_PARAMS, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXFRAME,
    SM_CXPADDEDBORDER, SM_CYFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, WM_NCLBUTTONDBLCLK, WM_SYSCOMMAND, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};

use super::icons::{self, Icon};
use super::theme::Theme;
use super::widgets;

/// Logical (unscaled) height of the custom integrated titlebar.
pub const TITLEBAR_H: f32 = 42.0;

/// Accessibility node keys of the window buttons (`CHROME_KEY + WinBtn as u32`); windows
/// keep their own keys below this.
pub const CHROME_KEY: u32 = 0xF000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinBtn {
    Min,
    Max,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tracking {
    Client,
    NonClient,
}

/// Per-window state for the custom integrated titlebar.
pub struct Chrome {
    pub resizable: bool,
    pub show_min: bool,
    pub show_max: bool,
    pub hover: Option<WinBtn>,
    /// Physical pixels kept free to the left of the window buttons (e.g. for a header action),
    /// so the title and badge never draw under it.
    pub reserve_right: i32,
    /// Button the current left-press started on; a release only acts on this same button.
    pressed: Option<WinBtn>,
    tracking: Option<Tracking>,
    icon_cache: Option<(u32, Pixmap)>,
}

impl Chrome {
    pub fn new(resizable: bool, show_min: bool, show_max: bool) -> Self {
        Chrome {
            resizable,
            show_min,
            show_max,
            hover: None,
            reserve_right: 0,
            pressed: None,
            tracking: None,
            icon_cache: None,
        }
    }

    /// Extends the DWM frame 1 px into the client area (keeps Windows 11 rounded corners & shadow)
    /// and triggers an immediate `WM_NCCALCSIZE`.
    pub fn init(hwnd: HWND) {
        let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        unsafe {
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Physical pixel height of the titlebar at `scale`.
    pub fn height(&self, scale: f32) -> i32 {
        (TITLEBAR_H * scale).round() as i32
    }

    /// Handles `WM_NCCALCSIZE` to remove the OS titlebar while keeping maximized window insets.
    pub fn handle_nccalcsize(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if wp.0 == 0 || lp.0 == 0 {
            return None;
        }
        unsafe {
            let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
            if IsZoomed(hwnd).as_bool() {
                let dpi = GetDpiForWindow(hwnd).max(96);
                let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad;
                let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad;
                params.rgrc[0].left += fx;
                params.rgrc[0].top += fy;
                params.rgrc[0].right -= fx;
                params.rgrc[0].bottom -= fy;
            }
        }
        Some(LRESULT(0))
    }

    /// Returns the window control buttons and their client-space rectangles.
    pub fn buttons(&self, client_w: i32, scale: f32) -> Vec<(WinBtn, Rect)> {
        let bw = (38.0 * scale).round() as i32;
        let bh = (28.0 * scale).round() as i32;
        let gap = (4.0 * scale).round() as i32;
        let right_pad = (8.0 * scale).round() as i32;
        let y = ((self.height(scale) - bh) / 2).max(2);
        let mut x = client_w - right_pad;
        let mut out = Vec::with_capacity(3);
        let mut push = |btn: WinBtn| {
            x -= bw;
            out.push((btn, Rect::new(x, y, bw, bh)));
            x -= gap;
        };
        push(WinBtn::Close);
        if self.show_max {
            push(WinBtn::Max);
        }
        if self.show_min {
            push(WinBtn::Min);
        }
        out
    }

    fn button_at(&self, client_w: i32, scale: f32, x: i32, y: i32) -> Option<WinBtn> {
        self.buttons(client_w, scale).into_iter().find(|(_, r)| r.contains(x, y)).map(|(b, _)| b)
    }

    /// Leftmost X coordinate occupied by the window buttons (for keeping header pills/actions clear).
    pub fn buttons_left(&self, client_w: i32, scale: f32) -> i32 {
        self.buttons(client_w, scale).iter().map(|(_, r)| r.x).min().unwrap_or(client_w)
    }

    /// Handles `WM_NCHITTEST` for resize borders, the Maximize button, and the draggable caption.
    pub fn hit_test(
        &self,
        hwnd: HWND,
        lp: LPARAM,
        scale: f32,
        interactive_rects: &[Rect],
    ) -> LRESULT {
        let sx = (lp.0 & 0xFFFF) as u16 as i16 as i32;
        let sy = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
        let mut pt = POINT { x: sx, y: sy };
        let mut rc = RECT::default();
        unsafe {
            let _ = ScreenToClient(hwnd, &mut pt);
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let (x, y) = (pt.x, pt.y);
        let (cw, ch) = (rc.right, rc.bottom);
        let zoomed = unsafe { IsZoomed(hwnd).as_bool() };

        if self.resizable && !zoomed {
            let b = (6.0 * scale).round().max(5.0) as i32;
            let left = x < b;
            let right = x >= cw - b;
            let top = y < b;
            let bottom = y >= ch - b;
            if top && left {
                return LRESULT(HTTOPLEFT as isize);
            }
            if top && right {
                return LRESULT(HTTOPRIGHT as isize);
            }
            if bottom && left {
                return LRESULT(HTBOTTOMLEFT as isize);
            }
            if bottom && right {
                return LRESULT(HTBOTTOMRIGHT as isize);
            }
            if left {
                return LRESULT(HTLEFT as isize);
            }
            if right {
                return LRESULT(HTRIGHT as isize);
            }
            if top {
                return LRESULT(HTTOP as isize);
            }
            if bottom {
                return LRESULT(HTBOTTOM as isize);
            }
        }

        if y >= 0 && y < self.height(scale) {
            match self.button_at(cw, scale, x, y) {
                // HTMAXBUTTON lets Windows 11 show the Snap Layouts flyout.
                Some(WinBtn::Max) => return LRESULT(HTMAXBUTTON as isize),
                Some(_) => return LRESULT(HTCLIENT as isize),
                None => {}
            }
            if interactive_rects.iter().any(|r| r.contains(x, y)) {
                return LRESULT(HTCLIENT as isize);
            }
            return LRESULT(HTCAPTION as isize);
        }

        LRESULT(HTCLIENT as isize)
    }

    fn track(&mut self, hwnd: HWND, kind: Tracking) {
        if self.tracking == Some(kind) {
            return;
        }
        let mut flags = TME_LEAVE;
        if kind == Tracking::NonClient {
            flags |= TME_NONCLIENT;
        }
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: flags,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
            self.tracking = Some(kind);
        }
    }

    /// Updates button hover state from a `WM_MOUSEMOVE` (client coordinates) and arms
    /// `WM_MOUSELEAVE`; returns `true` if the hover changed.
    pub fn set_hover(&mut self, hwnd: HWND, client_w: i32, scale: f32, x: i32, y: i32) -> bool {
        self.track(hwnd, Tracking::Client);
        let h = self.button_at(client_w, scale, x, y).filter(|b| *b != WinBtn::Max);
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    /// Handles `WM_MOUSELEAVE`; returns `true` if the hover changed. Window owners should
    /// also clear their own hover state.
    pub fn mouse_leave(&mut self) -> bool {
        if self.tracking == Some(Tracking::Client) {
            self.tracking = None;
        }
        // Max hover is owned by the non-client messages.
        if self.hover.is_some() && self.hover != Some(WinBtn::Max) {
            self.hover = None;
            return true;
        }
        false
    }

    /// Handles the non-client mouse messages produced by the `HTMAXBUTTON` / `HTCAPTION` areas.
    /// Returns `Some((result, repaint))` when the message was consumed, `None` to fall through
    /// to `DefWindowProcW`. `repaint` is set whenever hover state changed.
    pub fn nc_message(&mut self, hwnd: HWND, msg: u32, wp: WPARAM) -> Option<(LRESULT, bool)> {
        let on_max = wp.0 as u32 == HTMAXBUTTON;
        match msg {
            WM_NCMOUSEMOVE => {
                self.track(hwnd, Tracking::NonClient);
                let h = if on_max { Some(WinBtn::Max) } else { None };
                let changed = h != self.hover;
                self.hover = h;
                // Not consumed: DefWindowProc still needs it (e.g. for Snap Layouts).
                if changed {
                    unsafe {
                        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
                    }
                }
                None
            }
            WM_NCMOUSELEAVE => {
                if self.tracking == Some(Tracking::NonClient) {
                    self.tracking = None;
                }
                let changed = self.hover == Some(WinBtn::Max);
                if changed {
                    self.hover = None;
                }
                Some((LRESULT(0), changed))
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if on_max => {
                // Swallow so DefWindowProc doesn't draw the classic caption button.
                self.pressed = Some(WinBtn::Max);
                Some((LRESULT(0), false))
            }
            WM_NCLBUTTONUP if on_max => {
                if self.pressed.take() == Some(WinBtn::Max) {
                    Self::act(hwnd, WinBtn::Max);
                }
                Some((LRESULT(0), true))
            }
            WM_NCLBUTTONUP => {
                self.pressed = None;
                None
            }
            _ => None,
        }
    }

    /// Handles `WM_LBUTTONDOWN`; returns `true` if the press landed on a titlebar button
    /// (the caller should not start any other interaction).
    pub fn press(&mut self, client_w: i32, scale: f32, x: i32, y: i32) -> bool {
        self.pressed = self.button_at(client_w, scale, x, y);
        self.pressed.is_some()
    }

    /// Handles `WM_LBUTTONUP`. Acts only when the release is on the same button the press
    /// started on. Returns `true` if a titlebar press was pending (the release is consumed,
    /// whether or not it acted).
    pub fn release(&mut self, hwnd: HWND, client_w: i32, scale: f32, x: i32, y: i32) -> bool {
        let Some(pressed) = self.pressed.take() else {
            return false;
        };
        if self.button_at(client_w, scale, x, y) == Some(pressed) {
            Self::act(hwnd, pressed);
        }
        true
    }

    /// The window buttons for UI Automation (not Tab stops: Alt+F4 / Win+arrows cover the
    /// keyboard). Windows include these in their published node list.
    pub fn a11y_nodes(&self, hwnd: HWND, client_w: i32, scale: f32) -> Vec<super::a11y::Node> {
        use super::a11y::{Node, Role};
        let zoomed = unsafe { IsZoomed(hwnd).as_bool() };
        self.buttons(client_w, scale)
            .into_iter()
            .map(|(b, r)| {
                let name = match b {
                    WinBtn::Min => "Minimize",
                    WinBtn::Max if zoomed => "Restore",
                    WinBtn::Max => "Maximize",
                    WinBtn::Close => "Close",
                };
                Node::new(CHROME_KEY + b as u32, r, Role::Button, name).no_tab()
            })
            .collect()
    }

    /// Runs a window button a screen reader invoked; `false` if `key` isn't one.
    pub fn a11y_invoke(hwnd: HWND, key: u32) -> bool {
        let btn = match key.checked_sub(CHROME_KEY) {
            Some(0) => WinBtn::Min,
            Some(1) => WinBtn::Max,
            Some(2) => WinBtn::Close,
            _ => return false,
        };
        Self::act(hwnd, btn);
        true
    }

    /// Posts (never sends) the window command. Calling `ShowWindow` here would re-enter the
    /// window procedure while its handler is borrowed, so `WM_NCCALCSIZE` / `WM_SIZE` would
    /// fall through to `DefWindowProcW` and the window would get the native frame back.
    fn act(hwnd: HWND, btn: WinBtn) {
        let cmd = match btn {
            WinBtn::Min => SC_MINIMIZE,
            WinBtn::Max => {
                if unsafe { IsZoomed(hwnd).as_bool() } {
                    SC_RESTORE
                } else {
                    SC_MAXIMIZE
                }
            }
            WinBtn::Close => SC_CLOSE,
        };
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
        }
    }

    /// Draws the integrated titlebar: surface, bottom hairline, brand icon, title, optional badge,
    /// and Min/Max/Close buttons.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pm: &mut PixmapMut,
        t: &Theme,
        hwnd: HWND,
        client_w: i32,
        scale: f32,
        title: &str,
        badge: Option<&str>,
        bg_override: Option<Rgba>,
    ) {
        let h = self.height(scale) as f32;
        let cw = client_w as f32;
        let id = Transform::identity();
        let bg = bg_override.unwrap_or(t.sidebar);
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, 0.0, cw, h) {
            pm.fill_rect(r, &bg.paint(), id, None);
        }
        // Subtle 1px bottom hairline.
        if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, h - 1.0, cw, 1.0) {
            pm.fill_rect(r, &t.border_subtle.paint(), id, None);
        }

        // Mini Snap It viewfinder brand icon.
        let isz = (20.0 * scale).round().max(16.0) as u32;
        if self.icon_cache.as_ref().is_none_or(|(sz, _)| *sz != isz) {
            self.icon_cache = Some((isz, snapit_gfx::brand::icon(isz)));
        }
        let ix = (12.0 * scale).round() as i32;
        let iy = ((h - isz as f32) / 2.0).round() as i32;
        if let Some((_, ref ico)) = self.icon_cache {
            pm.draw_pixmap(ix, iy, ico.as_ref(), &PixmapPaint::default(), id, None);
        }

        // Title text, shortened so it never runs under the buttons or a reserved header action.
        let limit = (self.buttons_left(client_w, scale) - self.reserve_right) as f32 - 8.0 * scale;
        let tx = ix as f32 + isz as f32 + 10.0 * scale;
        let ts = 13.5 * scale;
        let title = widgets::ellipsize(title, ts, true, (limit - tx).max(0.0));
        let tl = text::layout(&title, ts, true);
        text::draw(pm, &title, tx, (h - tl.height) / 2.0, ts, true, t.text, None);

        // Optional status / version / count pill badge (dropped when it doesn't fit).
        if let Some(btxt) = badge {
            let bs = 11.0 * scale;
            let bl = text::layout(btxt, bs, false);
            let px = tx + tl.width + 10.0 * scale;
            let pw = bl.width + 14.0 * scale;
            let ph = 20.0 * scale;
            let py = (h - ph) / 2.0;
            if px + pw < limit {
                if let Some(p) = rounded_rect(px, py, pw, ph, ph / 2.0) {
                    pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
                    pm.stroke_path(&p, &t.border_subtle.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
                }
                text::draw(pm, btxt, px + 7.0 * scale, py + (ph - bl.height) / 2.0, bs, false, t.subtext, None);
            }
        }

        // Window buttons (Minimize, Maximize/Restore, Close).
        let zoomed = unsafe { IsZoomed(hwnd).as_bool() };
        for (btn, r) in self.buttons(client_w, scale) {
            let hovered = self.hover == Some(btn);
            if hovered {
                let fill = if btn == WinBtn::Close { t.danger } else { t.hover };
                if let Some(p) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 6.0 * scale) {
                    pm.fill_path(&p, &fill.paint(), FillRule::Winding, id, None);
                }
            }
            let fg = if hovered && btn == WinBtn::Close {
                Rgba::WHITE
            } else if hovered {
                t.text
            } else {
                t.subtext
            };
            let icon = match btn {
                WinBtn::Min => Icon::Minimize,
                WinBtn::Max => if zoomed { Icon::Restore } else { Icon::Maximize },
                WinBtn::Close => Icon::Close,
            };
            let gsz = 14.0 * scale;
            icons::draw(
                pm,
                icon,
                r.x as f32 + (r.w as f32 - gsz) / 2.0,
                r.y as f32 + (r.h as f32 - gsz) / 2.0,
                gsz,
                fg,
            );
        }
    }
}
