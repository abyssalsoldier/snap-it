//! Colours for Snap It's custom-drawn Modern Studio UI, following the Windows
//! light/dark setting unless the user picked one explicitly.

use snapit_gfx::tiny_skia::{GradientStop, LinearGradient, Paint, Point, SpreadMode, Transform};
use snapit_gfx::Rgba;
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_STYLE, WS_CAPTION};

use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::Theme as Pref;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Deepest window / canvas backdrop.
    pub bg: Rgba,
    /// Navigation sidebar / secondary chrome surface.
    pub sidebar: Rgba,
    /// Primary elevated panel / card surface.
    pub panel: Rgba,
    /// Raised interactive surface (buttons, chips, active pills).
    pub raised: Rgba,
    /// Hover surface fill.
    pub hover: Rgba,
    /// Text field / input well background.
    pub input_bg: Rgba,
    /// Crisp control / frame border.
    pub border: Rgba,
    /// Subtle hairline divider / card border.
    pub border_subtle: Rgba,
    /// Primary text colour.
    pub text: Rgba,
    /// Secondary / muted label colour.
    pub subtext: Rgba,
    /// Primary brand accent (Sky Cyan).
    pub accent: Rgba,
    /// Brand gradient end colour (Indigo), matching the Snap It viewfinder icon.
    pub accent_end: Rgba,
    /// Subtle translucent accent tint for active nav rows / selection fills.
    pub accent_soft: Rgba,
    /// Foreground text on accent fills.
    pub accent_text: Rgba,
    /// Destructive / Close hover / REC colour.
    pub danger: Rgba,
    /// Positive / valid status colour.
    pub success: Rgba,
    /// Soft drop shadow tint.
    pub shadow: Rgba,
}

impl Theme {
    /// Brand cyan-to-indigo linear gradient fill for primary buttons, active indicators, etc.
    pub fn accent_paint(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Paint<'static> {
        LinearGradient::new(
            Point::from_xy(x0, y0),
            Point::from_xy(x1, y1),
            vec![
                GradientStop::new(0.0, self.accent.color()),
                GradientStop::new(1.0, self.accent_end.color()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .map(|shader| Paint { shader, anti_alias: true, ..Default::default() })
        .unwrap_or_else(|| self.accent.paint())
    }

    /// Slightly brighter brand gradient for hovered primary buttons.
    pub fn accent_hover_paint(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Paint<'static> {
        let lift = |c: Rgba| {
            if self.dark {
                Rgba::rgb(c.r.saturating_add(18), c.g.saturating_add(16), c.b.saturating_add(14))
            } else {
                Rgba::rgb(c.r.saturating_sub(14), c.g.saturating_sub(12), c.b.saturating_sub(10))
            }
        };
        LinearGradient::new(
            Point::from_xy(x0, y0),
            Point::from_xy(x1, y1),
            vec![
                GradientStop::new(0.0, lift(self.accent).color()),
                GradientStop::new(1.0, lift(self.accent_end).color()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .map(|shader| Paint { shader, anti_alias: true, ..Default::default() })
        .unwrap_or_else(|| self.accent.paint())
    }
}

pub const DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(15, 17, 23),
    sidebar: Rgba::rgb(20, 23, 32),
    panel: Rgba::rgb(26, 30, 41),
    raised: Rgba::rgb(35, 41, 55),
    hover: Rgba::rgb(46, 53, 71),
    input_bg: Rgba::rgb(18, 21, 29),
    border: Rgba::rgb(52, 60, 80),
    border_subtle: Rgba::rgb(37, 43, 58),
    text: Rgba::rgb(241, 245, 249),
    subtext: Rgba::rgb(148, 163, 184),
    accent: Rgba::rgb(56, 189, 248),
    accent_end: Rgba::rgb(99, 102, 241),
    accent_soft: Rgba::new(56, 189, 248, 34),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(239, 68, 68),
    success: Rgba::rgb(34, 197, 94),
    shadow: Rgba::new(0, 0, 0, 110),
};

pub const LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(244, 246, 250),
    sidebar: Rgba::rgb(236, 240, 247),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(241, 245, 250),
    hover: Rgba::rgb(228, 234, 244),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(207, 215, 228),
    border_subtle: Rgba::rgb(228, 233, 242),
    text: Rgba::rgb(15, 23, 42),
    subtext: Rgba::rgb(100, 116, 139),
    accent: Rgba::rgb(14, 165, 233),
    accent_end: Rgba::rgb(79, 70, 229),
    accent_soft: Rgba::new(14, 165, 233, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(15, 23, 42, 34),
};

fn system_is_dark() -> bool {
    let mut v: u32 = 1;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    ok.is_ok() && v == 0
}

/// Unsaved theme choice being previewed from Settings (0 = none, else `Pref` index + 1).
static PREVIEW: AtomicU8 = AtomicU8::new(0);

const PREFS: [Pref; 3] = [Pref::System, Pref::Light, Pref::Dark];

/// Shows `pref` in every window until [`end_preview`]; pass `None` to stop previewing.
/// Re-themes all windows when the effective choice changes.
pub fn set_preview(pref: Option<Pref>) {
    let v = pref.map_or(0, |p| PREFS.iter().position(|x| *x == p).map_or(0, |i| i as u8 + 1));
    if PREVIEW.swap(v, Ordering::Relaxed) != v {
        super::broadcast_theme();
    }
}

/// Drops any preview without re-theming (the caller broadcasts once the real setting is in place).
pub fn end_preview() -> bool {
    PREVIEW.swap(0, Ordering::Relaxed) != 0
}

pub fn current() -> Theme {
    let pref = match PREVIEW.load(Ordering::Relaxed) {
        0 => crate::app::settings().theme,
        i => PREFS[(i as usize - 1).min(2)],
    };
    resolve(pref)
}

/// The colours a theme preference stands for right now (e.g. to preview an unsaved choice).
pub fn resolve(pref: Pref) -> Theme {
    match pref {
        Pref::Dark => DARK,
        Pref::Light => LIGHT,
        Pref::System => {
            if system_is_dark() {
                DARK
            } else {
                LIGHT
            }
        }
    }
}

/// Applies Windows 10/11 DWM frame styling (dark mode, rounded corners, border colour).
///
/// Corners and border are only set on framed windows (`WS_CAPTION`): forcing them on
/// popups would round and outline the capture overlay, pins and the recording frame.
pub fn apply_titlebar(hwnd: HWND, t: &Theme) {
    let dark: u32 = t.dark as u32;
    let framed = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CAPTION.0 == WS_CAPTION.0;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
    }
    if !framed {
        return;
    }
    let corner: u32 = DWMWCP_ROUND.0 as u32;
    let border_cref: u32 = (t.border.r as u32) | ((t.border.g as u32) << 8) | ((t.border.b as u32) << 16);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border_cref as *const _ as *const _, 4);
    }
}
