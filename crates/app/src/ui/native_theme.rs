//! Light/dark for windows built from standard Win32 controls (Settings, the QR dialog).
//!
//! Windows has no single switch for this, so each control type gets what works for it:
//! - window background and labels: our colours through `WM_CTLCOLOR*`;
//! - text boxes and drop-down lists: our field colours through `WM_CTLCOLOREDIT/LISTBOX`;
//! - push buttons, checkboxes, combo boxes and scrollbars: the dark visual styles Windows
//!   itself uses (`DarkMode_Explorer`, `DarkMode_DarkTheme`, `DarkMode_CFD`);
//! - the tab strip, which has no dark style: painted here in both themes, so its page area
//!   has the same background as the labels on it.
//!
//! In light mode the other controls keep their normal Windows look.

use std::cell::Cell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetSysColor,
    GetSysColorBrush, SelectObject, SetBkColor, SetBkMode, SetTextColor, COLOR_WINDOW, COLOR_WINDOWTEXT,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Controls::{SetWindowTheme, TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT, TCM_GETCURSEL, TCM_GETITEMCOUNT, TCM_GETITEMRECT, TCM_GETITEMW};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetClientRect, GetWindowLongW, SendMessageW, GWL_STYLE, WM_CTLCOLORBTN,
    WM_CTLCOLORDLG, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_GETFONT, WM_PAINT,
};

use snapit_gfx::Rgba;

fn cref(c: Rgba) -> COLORREF {
    COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16)
}

thread_local! {
    /// The tab strips' paint hook reads the current colours from here.
    static DARK_TABS: Cell<Option<Palette>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Palette {
    bg: COLORREF,
    field: COLORREF,
    text: COLORREF,
    subtext: COLORREF,
    border: COLORREF,
    accent: COLORREF,
}

/// Brushes and colours for one window; recreate with [`NativeTheme::refresh`] on a theme change.
pub struct NativeTheme {
    pub dark: bool,
    palette: Palette,
    bg: HBRUSH,
    field: HBRUSH,
}

impl NativeTheme {
    pub fn new() -> Self {
        let mut t = NativeTheme { dark: false, palette: Palette { bg: COLORREF(0), field: COLORREF(0), text: COLORREF(0), subtext: COLORREF(0), border: COLORREF(0), accent: COLORREF(0) }, bg: HBRUSH::default(), field: HBRUSH::default() };
        t.refresh();
        t
    }

    /// Re-reads the theme and rebuilds the brushes.
    pub fn refresh(&mut self) {
        self.free();
        let t = super::theme::current();
        self.dark = t.dark;
        self.palette = Palette { bg: cref(t.panel), field: cref(t.raised), text: cref(t.text), subtext: cref(t.subtext), border: cref(t.border), accent: cref(t.accent) };
        if !self.dark {
            self.palette.bg = Self::light_bg();
        }
        if self.dark {
            unsafe {
                self.bg = CreateSolidBrush(self.palette.bg);
                self.field = CreateSolidBrush(self.palette.field);
            }
        }
        // Tabs are painted by us in both themes, so the page background matches the labels'.
        DARK_TABS.with(|d| d.set(Some(self.palette)));
    }

    fn free(&mut self) {
        unsafe {
            for b in [self.bg, self.field] {
                if !b.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(b.0));
                }
            }
        }
        self.bg = HBRUSH::default();
        self.field = HBRUSH::default();
    }

    fn bg_brush(&self) -> HBRUSH {
        if self.dark { self.bg } else { unsafe { GetSysColorBrush(COLOR_WINDOW) } }
    }

    /// The light palette's page colour is the system window colour, so painted tabs and
    /// `COLOR_WINDOW` label backgrounds agree.
    fn light_bg() -> COLORREF {
        unsafe { COLORREF(GetSysColor(COLOR_WINDOW)) }
    }

    /// Applies the matching visual style to every child control of `parent`.
    pub fn style_children(&self, parent: HWND) {
        unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> windows::core::BOOL {
            let dark = lp.0 != 0;
            let mut buf = [0u16; 64];
            let n = unsafe { GetClassNameW(h, &mut buf) };
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_ascii_lowercase();
            let style = unsafe { GetWindowLongW(h, GWL_STYLE) } as u32 & 0xF;
            let dark_style: Option<PCWSTR> = match class.as_str() {
                // BS_CHECKBOX, BS_AUTOCHECKBOX, BS_RADIOBUTTON, BS_3STATE…: the Windows 11 dark
                // theme draws their box and their label in dark-mode colours.
                "button" if (2..=6).contains(&style) || style == 9 => Some(w!("DarkMode_DarkTheme")),
                "button" => Some(w!("DarkMode_Explorer")),
                "edit" | "combobox" => Some(w!("DarkMode_CFD")),
                "scrollbar" => Some(w!("DarkMode_Explorer")),
                _ => None,
            };
            if let Some(s) = dark_style {
                unsafe {
                    let _ = if dark { SetWindowTheme(h, s, PCWSTR::null()) } else { SetWindowTheme(h, PCWSTR::null(), PCWSTR::null()) };
                }
            }
            windows::core::BOOL(1)
        }
        unsafe {
            let _ = EnumChildWindows(Some(parent), Some(each), LPARAM(self.dark as isize));
        }
    }

    /// Dark checkboxes: Windows' dark style draws their labels in a dim grey that is hard to
    /// read, so the box is drawn by the theme and the label by us in the normal text colour.
    /// Call from `WM_NOTIFY`; returns `Some` when it drew the control.
    pub fn custom_draw(&self, lp: LPARAM) -> Option<LRESULT> {
        use windows::Win32::UI::Controls::{
            CloseThemeData, DrawThemeBackground, GetThemePartSize, OpenThemeData, BP_CHECKBOX, CDDS_PREPAINT, CDIS_FOCUS, CDIS_HOT,
            CDIS_SELECTED, CDRF_SKIPDEFAULT, NMCUSTOMDRAW, NM_CUSTOMDRAW, TS_DRAW,
        };
        use windows::Win32::Graphics::Gdi::{DrawFocusRect, DT_CALCRECT, DT_LEFT};
        use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
        if !self.dark || lp.0 == 0 {
            return None;
        }
        let cd = unsafe { &*(lp.0 as *const NMCUSTOMDRAW) };
        if cd.hdr.code != NM_CUSTOMDRAW || cd.dwDrawStage != CDDS_PREPAINT || !is_checkbox(cd.hdr.hwndFrom) {
            return None;
        }
        let h = cd.hdr.hwndFrom;
        let hdc = cd.hdc;
        let st = cd.uItemState.0;
        unsafe {
            let checked = SendMessageW(h, windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK, None, None).0 == 1;
            let enabled = IsWindowEnabled(h).as_bool();
            // CBS_UNCHECKEDNORMAL.. = 1..4, CBS_CHECKEDNORMAL.. = 5..8 (normal, hot, pressed, disabled).
            let base = if checked { 5 } else { 1 };
            let state = base
                + if !enabled {
                    3
                } else if st & CDIS_SELECTED.0 != 0 {
                    2
                } else if st & CDIS_HOT.0 != 0 {
                    1
                } else {
                    0
                };
            FillRect(hdc, &cd.rc, self.bg);
            let theme = OpenThemeData(Some(h), w!("Button"));
            let size = GetThemePartSize(theme, Some(hdc), BP_CHECKBOX.0, state, None, TS_DRAW).unwrap_or(windows::Win32::Foundation::SIZE { cx: 13, cy: 13 });
            let mid = (cd.rc.top + cd.rc.bottom) / 2;
            let boxr = RECT { left: cd.rc.left, top: mid - size.cy / 2, right: cd.rc.left + size.cx, bottom: mid - size.cy / 2 + size.cy };
            let _ = DrawThemeBackground(theme, hdc, BP_CHECKBOX.0, state, &boxr, None);
            let _ = CloseThemeData(theme);

            let mut text = [0u16; 512];
            let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(h, &mut text).max(0) as usize;
            let font = HFONT(SendMessageW(h, WM_GETFONT, None, None).0 as *mut _);
            let old = (!font.is_invalid()).then(|| SelectObject(hdc, HGDIOBJ(font.0)));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, if enabled { self.palette.text } else { self.palette.subtext });
            let gap = size.cx / 3 + 1;
            let mut tr = RECT { left: boxr.right + gap, ..cd.rc };
            DrawTextW(hdc, &mut text[..n], &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            if st & CDIS_FOCUS.0 != 0 {
                // Focus rectangle around the label, as the default drawing does.
                let mut fr = tr;
                DrawTextW(hdc, &mut text[..n], &mut fr, DT_LEFT | DT_SINGLELINE | DT_CALCRECT);
                let fh = fr.bottom - fr.top;
                let focus = RECT { left: fr.left - 1, top: mid - fh / 2 - 1, right: fr.right + 1, bottom: mid - fh / 2 + fh + 1 };
                let _ = DrawFocusRect(hdc, &focus);
            }
            if let Some(o) = old {
                SelectObject(hdc, o);
            }
        }
        Some(LRESULT(CDRF_SKIPDEFAULT as isize))
    }

    /// Paints the window background (for `WM_ERASEBKGND`).
    pub fn erase(&self, hwnd: HWND, hdc: HDC) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
            FillRect(hdc, &rc, self.bg_brush());
        }
    }

    /// Answers the `WM_CTLCOLOR*` messages; `None` means "use the default".
    pub fn ctlcolor(&self, msg: u32, wp: WPARAM) -> Option<LRESULT> {
        let hdc = HDC(wp.0 as *mut _);
        unsafe {
            match msg {
                WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORDLG => {
                    let (text, bg) = if self.dark { (self.palette.text, self.palette.bg) } else { (COLORREF(GetSysColor(COLOR_WINDOWTEXT)), COLORREF(GetSysColor(COLOR_WINDOW))) };
                    SetTextColor(hdc, text);
                    SetBkColor(hdc, bg);
                    Some(LRESULT(self.bg_brush().0 as isize))
                }
                WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX if self.dark => {
                    SetTextColor(hdc, self.palette.text);
                    SetBkColor(hdc, self.palette.field);
                    Some(LRESULT(self.field.0 as isize))
                }
                _ => None,
            }
        }
    }
}

impl Drop for NativeTheme {
    fn drop(&mut self) {
        self.free();
        DARK_TABS.with(|d| d.set(None));
    }
}

fn is_checkbox(h: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    let style = unsafe { GetWindowLongW(h, GWL_STYLE) } as u32 & 0xF;
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).eq_ignore_ascii_case("button") && matches!(style, 2 | 3 | 5 | 6)
}

/// Hooks a tab control so it is painted in dark colours while the theme is dark.
pub fn hook_tabs(tab: HWND) {
    unsafe {
        let _ = SetWindowSubclass(tab, Some(tab_proc), 1, 0);
    }
}

unsafe extern "system" fn tab_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    let Some(p) = DARK_TABS.with(|d| d.get()) else { return unsafe { DefSubclassProc(hwnd, msg, wp, lp) } };
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint_tabs(hwnd, p) };
            LRESULT(0)
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wp, lp) },
    }
}

unsafe fn paint_tabs(hwnd: HWND, p: Palette) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let brush = |c| CreateSolidBrush(c);
        let (bg, field, border, accent) = (brush(p.bg), brush(p.field), brush(p.border), brush(p.accent));
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        FillRect(hdc, &rc, bg);
        // The page area under the tabs, with a hairline frame.
        let mut page = rc;
        SendMessageW(hwnd, TCM_ADJUSTRECT, Some(WPARAM(0)), Some(LPARAM(&mut page as *mut _ as isize)));
        let frame = RECT { left: rc.left, top: page.top - 4, right: rc.right, bottom: rc.bottom };
        FrameRect(hdc, &frame, border);

        let font = HFONT(SendMessageW(hwnd, WM_GETFONT, None, None).0 as *mut _);
        let old = (!font.is_invalid()).then(|| SelectObject(hdc, HGDIOBJ(font.0)));
        SetBkMode(hdc, TRANSPARENT);
        let count = SendMessageW(hwnd, TCM_GETITEMCOUNT, None, None).0.max(0) as usize;
        let sel = SendMessageW(hwnd, TCM_GETCURSEL, None, None).0;
        for i in 0..count {
            let mut r = RECT::default();
            SendMessageW(hwnd, TCM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
            let on = i as isize == sel;
            if on {
                FillRect(hdc, &r, field);
                let bar = RECT { left: r.left, top: r.bottom - 2, right: r.right, bottom: r.bottom };
                FillRect(hdc, &bar, accent);
            }
            let mut buf = [0u16; 128];
            let mut item = TCITEMW { mask: TCIF_TEXT, pszText: windows::core::PWSTR(buf.as_mut_ptr()), cchTextMax: buf.len() as i32, ..Default::default() };
            SendMessageW(hwnd, TCM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize)));
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            SetTextColor(hdc, if on { p.text } else { p.subtext });
            let mut tr = r;
            DrawTextW(hdc, &mut buf[..n], &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        if let Some(o) = old {
            SelectObject(hdc, o);
        }
        for b in [bg, field, border, accent] {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
        let _ = EndPaint(hwnd, &ps);
    }
}
