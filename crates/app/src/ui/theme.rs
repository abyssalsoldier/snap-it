//! Colours for Snap It's custom-drawn UI, following the Windows light/dark setting
//! unless the user picked one explicitly.

use snapit_gfx::Rgba;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::core::w;

use crate::settings::Theme as Pref;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub bg: Rgba,
    pub panel: Rgba,
    pub raised: Rgba,
    pub hover: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub subtext: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
}

pub const DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(24, 24, 27),
    panel: Rgba::rgb(36, 36, 41),
    raised: Rgba::rgb(50, 50, 57),
    hover: Rgba::rgb(63, 63, 72),
    border: Rgba::rgb(70, 70, 80),
    text: Rgba::rgb(240, 240, 245),
    subtext: Rgba::rgb(160, 160, 172),
    accent: Rgba::rgb(56, 152, 248),
    accent_text: Rgba::WHITE,
};

pub const LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(243, 244, 246),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(233, 234, 238),
    hover: Rgba::rgb(222, 224, 230),
    border: Rgba::rgb(209, 213, 219),
    text: Rgba::rgb(17, 24, 39),
    subtext: Rgba::rgb(100, 106, 120),
    accent: Rgba::rgb(37, 99, 235),
    accent_text: Rgba::WHITE,
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

pub fn current() -> Theme {
    match crate::app::settings().theme {
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

/// Matches the window's title bar to the theme.
pub fn apply_titlebar(hwnd: HWND, t: &Theme) {
    let v: u32 = t.dark as u32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as *const _, 4);
    }
}
