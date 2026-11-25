//! Screen capture.
//!
//! Still screenshots use one GDI `BitBlt` from the screen DC per capture, the
//! same approach Snipping Tool uses, and only ever on an explicit user action.
//! Continuous capture for recording lives in the `record` crate and uses
//! Windows.Graphics.Capture instead.
//!
//! All coordinates are physical pixels in virtual-screen space (the process is
//! per-monitor DPI aware v2 via its manifest).

use snapit_gfx::{io::from_bgra_opaque, tiny_skia::Pixmap, Rect};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, EnumChildWindows, EnumWindows, GetClassNameW, GetCursorInfo, GetForegroundWindow,
    GetIconInfo, GetSystemMetrics, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, ICONINFO, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

pub type Error = String;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub rect: Rect,
    pub work: Rect,
    pub primary: bool,
    pub dpi: u32,
    pub handle: isize,
}

impl Monitor {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

fn rect_of(r: RECT) -> Rect {
    Rect::from_ltrb(r.left, r.top, r.right, r.bottom)
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let out = &mut *(lp.0 as *mut Vec<Monitor>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hm, &mut mi).as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            out.push(Monitor {
                rect: rect_of(mi.rcMonitor),
                work: rect_of(mi.rcWork),
                primary: mi.dwFlags & 1 != 0,
                dpi: dx,
                handle: hm.0 as isize,
            });
        }
        TRUE
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

pub fn virtual_screen() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn monitor_at(mons: &[Monitor], x: i32, y: i32) -> Option<&Monitor> {
    mons.iter().find(|m| m.rect.contains(x, y)).or_else(|| mons.iter().find(|m| m.primary))
}

/// Captures a screen rectangle once. `cursor` draws the mouse pointer into the image.
pub fn capture_rect(r: Rect, cursor: bool) -> Result<Pixmap, Error> {
    if r.is_empty() {
        return Err("empty capture area".into());
    }
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = (|| {
            let dib = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|e| format!("CreateDIBSection: {e}"))?;
            let old = SelectObject(mem, dib.into());
            let blt = BitBlt(mem, 0, 0, r.w, r.h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT);
            if blt.is_ok() && cursor {
                draw_cursor(mem, r.x, r.y);
            }
            let len = (r.w as usize) * (r.h as usize) * 4;
            let data = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
            blt.map_err(|e| format!("BitBlt: {e}"))?;
            from_bgra_opaque(r.w as u32, r.h as u32, data).ok_or_else(|| "bad size".to_string())
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

unsafe fn draw_cursor(dc: HDC, ox: i32, oy: i32) {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 {
        return;
    }
    let mut ii = ICONINFO::default();
    if GetIconInfo(ci.hCursor.into(), &mut ii).is_err() {
        return;
    }
    let x = ci.ptScreenPos.x - ii.xHotspot as i32 - ox;
    let y = ci.ptScreenPos.y - ii.yHotspot as i32 - oy;
    let _ = DrawIconEx(dc, x, y, ci.hCursor.into(), 0, 0, 0, None, DI_NORMAL);
    if !ii.hbmMask.is_invalid() {
        let _ = DeleteObject(ii.hbmMask.into());
    }
    if !ii.hbmColor.is_invalid() {
        let _ = DeleteObject(ii.hbmColor.into());
    }
}

pub fn capture_fullscreen(cursor: bool) -> Result<(Pixmap, Rect), Error> {
    let r = virtual_screen();
    capture_rect(r, cursor).map(|p| (p, r))
}

/// The visible frame of a window, without the invisible resize borders Windows 10+ adds.
pub fn window_rect(hwnd: isize) -> Option<Rect> {
    let h = HWND(hwnd as *mut _);
    let mut r = RECT::default();
    unsafe {
        let ok = DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32);
        if ok.is_err() && GetWindowRect(h, &mut r).is_err() {
            return None;
        }
    }
    let r = rect_of(r);
    (!r.is_empty()).then_some(r)
}

pub fn foreground_window() -> Option<(isize, Rect)> {
    let h = unsafe { GetForegroundWindow() };
    if h.is_invalid() {
        return None;
    }
    window_rect(h.0 as isize).map(|r| (h.0 as isize, r))
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub rect: Rect,
    pub title: String,
    /// Visible child-control rectangles, used for control-level snapping.
    pub children: Vec<Rect>,
}

fn is_cloaked(h: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0
    }
}

fn text_of(h: HWND, class: bool) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { if class { GetClassNameW(h, &mut buf) } else { GetWindowTextW(h, &mut buf) } };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Visible top-level windows in z-order (topmost first), excluding this process's own windows.
/// Snapshotted once when the region overlay opens; no per-mouse-move cross-process calls.
pub fn visible_windows() -> Vec<WindowInfo> {
    struct Ctx {
        out: Vec<WindowInfo>,
        own_pid: u32,
    }
    unsafe extern "system" fn child_cb(h: HWND, lp: LPARAM) -> BOOL {
        let out = &mut *(lp.0 as *mut Vec<Rect>);
        if IsWindowVisible(h).as_bool() {
            let mut r = RECT::default();
            if GetWindowRect(h, &mut r).is_ok() {
                let r = rect_of(r);
                if r.w > 4 && r.h > 4 {
                    out.push(r);
                }
            }
        }
        TRUE
    }
    unsafe extern "system" fn top_cb(h: HWND, lp: LPARAM) -> BOOL {
        let ctx = &mut *(lp.0 as *mut Ctx);
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || is_cloaked(h) {
            return TRUE;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == ctx.own_pid {
            return TRUE;
        }
        let Some(rect) = window_rect(h.0 as isize) else { return TRUE };
        if rect.w < 8 || rect.h < 8 {
            return TRUE;
        }
        let class = text_of(h, true);
        if class == "Progman" || class == "WorkerW" {
            return TRUE; // the desktop; monitors cover that case
        }
        let mut children = Vec::new();
        let _ = EnumChildWindows(Some(h), Some(child_cb), LPARAM(&mut children as *mut _ as isize));
        // Smallest first so hit-testing finds the most specific control.
        children.sort_by_key(|r: &Rect| r.area());
        ctx.out.push(WindowInfo { hwnd: h.0 as isize, rect, title: text_of(h, false), children });
        TRUE
    }
    let mut ctx = Ctx { out: Vec::new(), own_pid: std::process::id() };
    unsafe {
        let _ = EnumWindows(Some(top_cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.out
}

/// The rectangle under (x, y): the topmost window containing it, or with
/// `controls` its smallest child control containing it.
pub fn pick_at(wins: &[WindowInfo], x: i32, y: i32, controls: bool) -> Option<Rect> {
    let w = wins.iter().find(|w| w.rect.contains(x, y))?;
    if controls {
        if let Some(c) = w.children.iter().find(|c| c.contains(x, y)) {
            return c.intersect(w.rect);
        }
    }
    Some(w.rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_monitors_and_virtual_screen() {
        let mons = monitors();
        assert!(!mons.is_empty());
        let vs = virtual_screen();
        for m in &mons {
            assert_eq!(m.rect.union(vs), vs);
        }
    }

    #[test]
    fn captures_a_small_rect() {
        let pm = capture_rect(Rect::new(0, 0, 32, 16), true).unwrap();
        assert_eq!((pm.width(), pm.height()), (32, 16));
        assert!(pm.data().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn pick_prefers_children_when_asked() {
        let w = WindowInfo {
            hwnd: 1,
            rect: Rect::new(0, 0, 100, 100),
            title: String::new(),
            children: vec![Rect::new(10, 10, 20, 20)],
        };
        let wins = [w];
        assert_eq!(pick_at(&wins, 15, 15, true), Some(Rect::new(10, 10, 20, 20)));
        assert_eq!(pick_at(&wins, 15, 15, false), Some(Rect::new(0, 0, 100, 100)));
        assert_eq!(pick_at(&wins, 200, 15, false), None);
    }
}
