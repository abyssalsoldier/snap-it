//! Window plumbing shared by every Snap It window.
//!
//! [`create`] registers a class on first use and stores a boxed [`Handler`] in
//! the window's user data. Messages are routed to it through a `RefCell`: a
//! re-entrant message (one sent while the handler is already running) falls
//! back to `DefWindowProc` instead of aliasing `&mut`. Handlers ask to close
//! via [`Win::close`]; the window is destroyed only after the handler returns.

pub mod backbuffer;
pub mod chrome;
pub mod dialogs;
pub mod editor;
pub mod history;
pub mod icons;
pub mod overlay;
pub mod pin;
pub mod qr_dialog;
pub mod settings_window;
pub mod theme;
pub mod tool_options;
pub mod toolbar;
pub mod welcome;
pub mod widgets;

use std::cell::RefCell;
use std::collections::HashSet;

use snapit_gfx::Rect;
use windows::core::{Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, GetWindowTextW, LoadCursorW,
    RegisterClassExW, SetWindowLongPtrW, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW,
    GWLP_USERDATA, IDC_ARROW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW,
};

pub use backbuffer::Backbuffer;

pub struct Win {
    pub hwnd: HWND,
    close: bool,
}

impl Win {
    pub fn close(&mut self) {
        self.close = true;
    }
    pub fn invalidate(&self) {
        invalidate(self.hwnd, None);
    }
    pub fn dpi_scale(&self) -> f32 {
        dpi_scale(self.hwnd)
    }
}

pub trait Handler {
    /// Return `Some` to consume the message, `None` for default processing.
    fn message(&mut self, w: &mut Win, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT>;
    /// The last rendered frame, for the self-test's PNG dumps.
    fn snapshot(&self) -> Option<snapit_gfx::tiny_skia::Pixmap> {
        None
    }
}

/// Self-test: `lparam` is a `Box<PathBuf>`; the window saves its last frame there.
pub const WM_SNAPIT_DUMP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 90;

/// Light/dark changed (Windows setting or Snap It's own). Every window gets it: the
/// title bar is updated here, then the handler re-reads [`theme::current`].
pub const WM_SNAPIT_THEME: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 94;

/// Tells every Snap It window (all live on this thread) that the theme changed.
pub fn broadcast_theme() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, PostMessageW};
    unsafe extern "system" fn post(h: HWND, _: LPARAM) -> windows::core::BOOL {
        unsafe {
            let _ = PostMessageW(Some(h), WM_SNAPIT_THEME, WPARAM(0), LPARAM(0));
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(post), LPARAM(0));
    }
}

thread_local! {
    static HEADLESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub fn set_headless(on: bool) {
    HEADLESS.with(|h| h.set(on));
}

pub fn headless() -> bool {
    HEADLESS.with(|h| h.get())
}

/// Shows a window (and optionally brings it to the front), unless running headless.
pub fn show(hwnd: HWND, activate: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW, SW_SHOWNOACTIVATE};
    if headless() {
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if activate {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

type Slot = RefCell<Box<dyn Handler>>;

thread_local! {
    static CLASSES: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
}

fn ensure_class(name: &'static str) -> Result<()> {
    if CLASSES.with(|c| c.borrow().contains(name)) {
        return Ok(());
    }
    let wname = HSTRING::from(name);
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: PCWSTR(wname.as_ptr()),
            hIcon: crate::app_icon(),
            hIconSm: crate::app_icon_small(),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_win32());
        }
    }
    CLASSES.with(|c| c.borrow_mut().insert(name));
    Ok(())
}

pub fn create(
    class: &'static str,
    ex: WINDOW_EX_STYLE,
    style: WINDOW_STYLE,
    title: &str,
    r: Rect,
    owner: Option<HWND>,
    handler: Box<dyn Handler>,
) -> Result<HWND> {
    ensure_class(class)?;
    let slot: *mut Slot = Box::into_raw(Box::new(RefCell::new(handler)));
    let res = unsafe {
        CreateWindowExW(
            ex,
            &HSTRING::from(class),
            &HSTRING::from(title),
            style,
            r.x,
            r.y,
            r.w,
            r.h,
            owner,
            None,
            Some(GetModuleHandleW(None)?.into()),
            Some(slot as *const _),
        )
    };
    if let Ok(h) = res {
        theme::apply_titlebar(h, &theme::current());
    }
    if res.is_err() {
        // WM_NCCREATE may not have run; if it did, WM_NCDESTROY already freed the slot.
        // CreateWindowEx failing before NCCREATE is the only path that reaches here
        // with the slot unowned, so leaking a little is the safe choice.
        crate::log("CreateWindowEx failed");
    }
    res
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Slot;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_SNAPIT_DUMP {
        let path = Box::from_raw(lp.0 as *mut std::path::PathBuf);
        let ok = (*ptr).try_borrow().ok().and_then(|h| h.snapshot()).map(|pm| snapit_gfx::io::save(&pm, &path, snapit_gfx::io::Format::Png, 100).is_ok()).unwrap_or(false);
        return LRESULT(ok as isize);
    }
    if msg == WM_SNAPIT_THEME {
        theme::apply_titlebar(hwnd, &theme::current());
    }
    let mut want_close = false;
    let result = match (*ptr).try_borrow_mut() {
        Ok(mut h) => {
            let mut w = Win { hwnd, close: false };
            let r = h.message(&mut w, msg, wp, lp);
            want_close = w.close;
            r
        }
        Err(_) => None,
    };
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if (*ptr).try_borrow_mut().is_ok() {
            drop(Box::from_raw(ptr));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if want_close {
        let _ = DestroyWindow(hwnd);
        return result.unwrap_or(LRESULT(0));
    }
    if msg == WM_SNAPIT_THEME {
        // Repaint everything (frame, client and child controls) in the new colours.
        use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE};
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
        return result.unwrap_or(LRESULT(0));
    }
    result.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

pub fn invalidate(hwnd: HWND, r: Option<Rect>) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    unsafe {
        match r {
            Some(r) => {
                let rc = RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
                let _ = InvalidateRect(Some(hwnd), Some(&rc), false);
            }
            None => {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

pub fn dpi_scale(hwnd: HWND) -> f32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 1.0 } else { d as f32 / 96.0 }
}

pub fn window_title(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(HWND(hwnd as *mut _), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Signed client coordinates packed in an LPARAM.
pub fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

pub fn key_down(vk: u16) -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(vk as i32) < 0 }
}

pub fn ctrl() -> bool {
    key_down(0x11)
}
pub fn shift() -> bool {
    key_down(0x10)
}
