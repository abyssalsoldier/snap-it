//! Colours for Snap It's custom-drawn UI, with 11 dual-mode (Dark + Light)
//! colour and genre themes that follow the Windows light/dark setting unless
//! the user picked a mode explicitly.

use std::cell::Cell;

use snapit_gfx::tiny_skia::{GradientStop, LinearGradient, Paint, Point, SpreadMode, Transform};
use snapit_gfx::Rgba;
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

use crate::settings::{Theme as Pref, ThemePreset};

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
    /// Primary brand / theme accent.
    pub accent: Rgba,
    /// Accent gradient end colour.
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
    /// Theme linear gradient fill for primary buttons, active indicators, and swatches.
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

    /// Slightly shifted gradient for hovered primary buttons.
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

pub struct PresetMeta {
    pub id: ThemePreset,
    pub name: &'static str,
    pub tagline: &'static str,
    pub dark: Theme,
    pub light: Theme,
}

// 1. Modern Studio (Default)
pub const STUDIO_DARK: Theme = Theme {
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

pub const STUDIO_LIGHT: Theme = Theme {
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

// 2. Crimson Noir (Dark & Red)
const CRIMSON_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(16, 11, 13),
    sidebar: Rgba::rgb(22, 15, 18),
    panel: Rgba::rgb(31, 20, 24),
    raised: Rgba::rgb(44, 28, 33),
    hover: Rgba::rgb(58, 35, 42),
    input_bg: Rgba::rgb(19, 13, 15),
    border: Rgba::rgb(78, 42, 50),
    border_subtle: Rgba::rgb(48, 29, 34),
    text: Rgba::rgb(250, 242, 243),
    subtext: Rgba::rgb(184, 146, 154),
    accent: Rgba::rgb(239, 68, 68),
    accent_end: Rgba::rgb(185, 28, 28),
    accent_soft: Rgba::new(239, 68, 68, 42),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(248, 113, 113),
    success: Rgba::rgb(34, 197, 94),
    shadow: Rgba::new(0, 0, 0, 125),
};

const CRIMSON_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(251, 245, 246),
    sidebar: Rgba::rgb(246, 236, 238),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(250, 241, 243),
    hover: Rgba::rgb(242, 227, 230),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(230, 204, 210),
    border_subtle: Rgba::rgb(242, 226, 230),
    text: Rgba::rgb(38, 17, 22),
    subtext: Rgba::rgb(132, 92, 102),
    accent: Rgba::rgb(220, 38, 38),
    accent_end: Rgba::rgb(153, 27, 27),
    accent_soft: Rgba::new(220, 38, 38, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(40, 12, 18, 34),
};

// 3. Midnight Amethyst (Dark & Deep Purple)
const AMETHYST_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(14, 11, 22),
    sidebar: Rgba::rgb(20, 15, 31),
    panel: Rgba::rgb(28, 21, 43),
    raised: Rgba::rgb(40, 30, 61),
    hover: Rgba::rgb(52, 39, 78),
    input_bg: Rgba::rgb(17, 13, 27),
    border: Rgba::rgb(70, 52, 104),
    border_subtle: Rgba::rgb(44, 33, 66),
    text: Rgba::rgb(245, 241, 255),
    subtext: Rgba::rgb(168, 154, 198),
    accent: Rgba::rgb(147, 51, 234),
    accent_end: Rgba::rgb(109, 40, 217),
    accent_soft: Rgba::new(147, 51, 234, 44),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(244, 63, 94),
    success: Rgba::rgb(52, 211, 153),
    shadow: Rgba::new(4, 2, 12, 125),
};

const AMETHYST_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(247, 245, 253),
    sidebar: Rgba::rgb(239, 234, 250),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(244, 240, 252),
    hover: Rgba::rgb(233, 226, 248),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(213, 202, 236),
    border_subtle: Rgba::rgb(231, 224, 246),
    text: Rgba::rgb(28, 19, 48),
    subtext: Rgba::rgb(111, 96, 146),
    accent: Rgba::rgb(126, 34, 206),
    accent_end: Rgba::rgb(91, 33, 182),
    accent_soft: Rgba::new(126, 34, 206, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(225, 29, 72),
    success: Rgba::rgb(16, 185, 129),
    shadow: Rgba::new(30, 15, 58, 34),
};

// 4. Cyber Neon (Cyberpunk / Synthwave)
const CYBER_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(11, 10, 24),
    sidebar: Rgba::rgb(17, 14, 34),
    panel: Rgba::rgb(24, 20, 46),
    raised: Rgba::rgb(35, 29, 66),
    hover: Rgba::rgb(47, 39, 86),
    input_bg: Rgba::rgb(14, 12, 30),
    border: Rgba::rgb(78, 56, 126),
    border_subtle: Rgba::rgb(42, 34, 76),
    text: Rgba::rgb(244, 246, 255),
    subtext: Rgba::rgb(160, 156, 204),
    accent: Rgba::rgb(168, 85, 247),
    accent_end: Rgba::rgb(236, 72, 153),
    accent_soft: Rgba::new(236, 72, 153, 38),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(251, 113, 133),
    success: Rgba::rgb(45, 212, 191),
    shadow: Rgba::new(5, 2, 18, 130),
};

const CYBER_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(246, 245, 252),
    sidebar: Rgba::rgb(238, 235, 249),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(245, 240, 252),
    hover: Rgba::rgb(234, 226, 248),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(214, 202, 236),
    border_subtle: Rgba::rgb(232, 224, 246),
    text: Rgba::rgb(24, 18, 46),
    subtext: Rgba::rgb(107, 98, 142),
    accent: Rgba::rgb(147, 51, 234),
    accent_end: Rgba::rgb(219, 39, 119),
    accent_soft: Rgba::new(219, 39, 119, 26),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(225, 29, 72),
    success: Rgba::rgb(13, 148, 136),
    shadow: Rgba::new(36, 16, 68, 34),
};

// 5. Nord Frost (Arctic / Polar Slate)
const NORD_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(27, 32, 42),
    sidebar: Rgba::rgb(33, 39, 51),
    panel: Rgba::rgb(40, 47, 61),
    raised: Rgba::rgb(50, 59, 76),
    hover: Rgba::rgb(62, 72, 92),
    input_bg: Rgba::rgb(30, 35, 46),
    border: Rgba::rgb(71, 83, 105),
    border_subtle: Rgba::rgb(52, 61, 78),
    text: Rgba::rgb(236, 239, 244),
    subtext: Rgba::rgb(159, 173, 194),
    accent: Rgba::rgb(136, 192, 208),
    accent_end: Rgba::rgb(94, 129, 172),
    accent_soft: Rgba::new(136, 192, 208, 34),
    accent_text: Rgba::rgb(20, 26, 36),
    danger: Rgba::rgb(191, 97, 106),
    success: Rgba::rgb(163, 190, 140),
    shadow: Rgba::new(12, 16, 24, 110),
};

const NORD_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(236, 239, 244),
    sidebar: Rgba::rgb(229, 233, 240),
    panel: Rgba::rgb(248, 250, 252),
    raised: Rgba::rgb(233, 238, 245),
    hover: Rgba::rgb(220, 227, 237),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(198, 207, 220),
    border_subtle: Rgba::rgb(218, 224, 234),
    text: Rgba::rgb(46, 52, 64),
    subtext: Rgba::rgb(96, 109, 130),
    accent: Rgba::rgb(76, 139, 168),
    accent_end: Rgba::rgb(76, 106, 152),
    accent_soft: Rgba::new(76, 139, 168, 30),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(191, 97, 106),
    success: Rgba::rgb(112, 148, 92),
    shadow: Rgba::new(46, 52, 64, 30),
};

// 6. Emerald Forest (Nature / Botanical)
const FOREST_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(11, 20, 17),
    sidebar: Rgba::rgb(15, 27, 23),
    panel: Rgba::rgb(21, 36, 31),
    raised: Rgba::rgb(29, 49, 42),
    hover: Rgba::rgb(38, 63, 54),
    input_bg: Rgba::rgb(13, 23, 20),
    border: Rgba::rgb(44, 74, 63),
    border_subtle: Rgba::rgb(30, 50, 43),
    text: Rgba::rgb(236, 250, 244),
    subtext: Rgba::rgb(142, 179, 164),
    accent: Rgba::rgb(16, 185, 129),
    accent_end: Rgba::rgb(13, 148, 136),
    accent_soft: Rgba::new(16, 185, 129, 36),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(239, 68, 68),
    success: Rgba::rgb(52, 211, 153),
    shadow: Rgba::new(0, 8, 5, 115),
};

const FOREST_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(242, 249, 246),
    sidebar: Rgba::rgb(232, 244, 238),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(239, 247, 243),
    hover: Rgba::rgb(224, 239, 232),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(197, 224, 212),
    border_subtle: Rgba::rgb(220, 238, 230),
    text: Rgba::rgb(16, 38, 30),
    subtext: Rgba::rgb(82, 118, 104),
    accent: Rgba::rgb(5, 150, 105),
    accent_end: Rgba::rgb(15, 118, 110),
    accent_soft: Rgba::new(5, 150, 105, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(10, 38, 28, 30),
};

// 7. Sunset Ember (Warm / Golden Hour)
const SUNSET_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(22, 16, 15),
    sidebar: Rgba::rgb(29, 21, 19),
    panel: Rgba::rgb(39, 28, 25),
    raised: Rgba::rgb(53, 38, 34),
    hover: Rgba::rgb(68, 49, 43),
    input_bg: Rgba::rgb(25, 18, 17),
    border: Rgba::rgb(86, 61, 53),
    border_subtle: Rgba::rgb(56, 40, 35),
    text: Rgba::rgb(253, 246, 240),
    subtext: Rgba::rgb(191, 162, 148),
    accent: Rgba::rgb(249, 115, 22),
    accent_end: Rgba::rgb(234, 88, 12),
    accent_soft: Rgba::new(249, 115, 22, 38),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(239, 68, 68),
    success: Rgba::rgb(74, 222, 128),
    shadow: Rgba::new(10, 4, 2, 120),
};

const SUNSET_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(252, 247, 242),
    sidebar: Rgba::rgb(247, 239, 230),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(250, 243, 236),
    hover: Rgba::rgb(243, 232, 221),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(230, 212, 196),
    border_subtle: Rgba::rgb(242, 230, 218),
    text: Rgba::rgb(42, 25, 18),
    subtext: Rgba::rgb(134, 103, 88),
    accent: Rgba::rgb(234, 88, 12),
    accent_end: Rgba::rgb(217, 119, 6),
    accent_soft: Rgba::new(234, 88, 12, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(48, 24, 10, 30),
};

// 8. Rose Quartz (Pastel / Soft)
const ROSE_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(22, 15, 22),
    sidebar: Rgba::rgb(29, 20, 29),
    panel: Rgba::rgb(39, 27, 39),
    raised: Rgba::rgb(52, 36, 52),
    hover: Rgba::rgb(66, 46, 66),
    input_bg: Rgba::rgb(25, 17, 25),
    border: Rgba::rgb(86, 58, 86),
    border_subtle: Rgba::rgb(56, 39, 56),
    text: Rgba::rgb(252, 242, 248),
    subtext: Rgba::rgb(192, 158, 184),
    accent: Rgba::rgb(244, 114, 182),
    accent_end: Rgba::rgb(192, 132, 252),
    accent_soft: Rgba::new(244, 114, 182, 36),
    accent_text: Rgba::rgb(28, 14, 26),
    danger: Rgba::rgb(251, 113, 133),
    success: Rgba::rgb(74, 222, 128),
    shadow: Rgba::new(10, 4, 12, 115),
};

const ROSE_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(253, 245, 249),
    sidebar: Rgba::rgb(248, 236, 243),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(250, 241, 246),
    hover: Rgba::rgb(244, 228, 237),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(232, 206, 222),
    border_subtle: Rgba::rgb(244, 228, 237),
    text: Rgba::rgb(44, 22, 38),
    subtext: Rgba::rgb(138, 102, 126),
    accent: Rgba::rgb(219, 39, 119),
    accent_end: Rgba::rgb(147, 51, 234),
    accent_soft: Rgba::new(219, 39, 119, 26),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(225, 29, 72),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(50, 18, 40, 30),
};

// 9. Monochrome Pro (Minimal / OLED)
const MONO_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(10, 10, 12),
    sidebar: Rgba::rgb(16, 16, 18),
    panel: Rgba::rgb(22, 22, 25),
    raised: Rgba::rgb(32, 32, 36),
    hover: Rgba::rgb(44, 44, 49),
    input_bg: Rgba::rgb(13, 13, 15),
    border: Rgba::rgb(56, 56, 62),
    border_subtle: Rgba::rgb(34, 34, 38),
    text: Rgba::rgb(248, 248, 250),
    subtext: Rgba::rgb(154, 154, 162),
    accent: Rgba::rgb(235, 235, 240),
    accent_end: Rgba::rgb(185, 185, 195),
    accent_soft: Rgba::new(255, 255, 255, 24),
    accent_text: Rgba::rgb(14, 14, 16),
    danger: Rgba::rgb(239, 68, 68),
    success: Rgba::rgb(34, 197, 94),
    shadow: Rgba::new(0, 0, 0, 135),
};

const MONO_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(245, 245, 246),
    sidebar: Rgba::rgb(237, 237, 239),
    panel: Rgba::rgb(255, 255, 255),
    raised: Rgba::rgb(242, 242, 244),
    hover: Rgba::rgb(230, 230, 234),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(210, 210, 216),
    border_subtle: Rgba::rgb(230, 230, 235),
    text: Rgba::rgb(18, 18, 22),
    subtext: Rgba::rgb(104, 104, 114),
    accent: Rgba::rgb(32, 32, 38),
    accent_end: Rgba::rgb(68, 68, 78),
    accent_soft: Rgba::new(24, 24, 28, 20),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(0, 0, 0, 32),
};

// 10. Retro Terminal (Genre: Hacker / CRT)
const TERMINAL_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(7, 13, 9),
    sidebar: Rgba::rgb(11, 19, 13),
    panel: Rgba::rgb(15, 26, 18),
    raised: Rgba::rgb(21, 38, 26),
    hover: Rgba::rgb(28, 52, 35),
    input_bg: Rgba::rgb(9, 16, 11),
    border: Rgba::rgb(36, 78, 48),
    border_subtle: Rgba::rgb(23, 46, 30),
    text: Rgba::rgb(214, 252, 224),
    subtext: Rgba::rgb(118, 176, 134),
    accent: Rgba::rgb(34, 197, 94),
    accent_end: Rgba::rgb(132, 204, 22),
    accent_soft: Rgba::new(34, 197, 94, 38),
    accent_text: Rgba::rgb(8, 20, 10),
    danger: Rgba::rgb(248, 113, 113),
    success: Rgba::rgb(74, 222, 128),
    shadow: Rgba::new(0, 0, 0, 135),
};

const TERMINAL_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(242, 247, 242),
    sidebar: Rgba::rgb(232, 241, 232),
    panel: Rgba::rgb(252, 255, 252),
    raised: Rgba::rgb(238, 246, 238),
    hover: Rgba::rgb(223, 237, 224),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(186, 214, 190),
    border_subtle: Rgba::rgb(214, 232, 216),
    text: Rgba::rgb(16, 38, 22),
    subtext: Rgba::rgb(74, 114, 84),
    accent: Rgba::rgb(21, 128, 61),
    accent_end: Rgba::rgb(77, 124, 15),
    accent_soft: Rgba::new(21, 128, 61, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(10, 32, 16, 30),
};

// 11. Arcane Gold (Genre: Fantasy / Royal)
const ARCANE_DARK: Theme = Theme {
    dark: true,
    bg: Rgba::rgb(16, 15, 23),
    sidebar: Rgba::rgb(22, 20, 31),
    panel: Rgba::rgb(30, 27, 42),
    raised: Rgba::rgb(42, 38, 56),
    hover: Rgba::rgb(55, 49, 72),
    input_bg: Rgba::rgb(19, 17, 27),
    border: Rgba::rgb(82, 70, 56),
    border_subtle: Rgba::rgb(48, 43, 60),
    text: Rgba::rgb(250, 246, 236),
    subtext: Rgba::rgb(178, 168, 148),
    accent: Rgba::rgb(245, 197, 66),
    accent_end: Rgba::rgb(217, 119, 6),
    accent_soft: Rgba::new(245, 197, 66, 34),
    accent_text: Rgba::rgb(26, 20, 12),
    danger: Rgba::rgb(239, 68, 68),
    success: Rgba::rgb(52, 211, 153),
    shadow: Rgba::new(4, 3, 10, 125),
};

const ARCANE_LIGHT: Theme = Theme {
    dark: false,
    bg: Rgba::rgb(250, 247, 240),
    sidebar: Rgba::rgb(244, 238, 226),
    panel: Rgba::rgb(255, 254, 250),
    raised: Rgba::rgb(247, 242, 231),
    hover: Rgba::rgb(238, 230, 214),
    input_bg: Rgba::rgb(255, 255, 255),
    border: Rgba::rgb(220, 206, 178),
    border_subtle: Rgba::rgb(236, 226, 206),
    text: Rgba::rgb(34, 28, 20),
    subtext: Rgba::rgb(122, 108, 86),
    accent: Rgba::rgb(180, 124, 18),
    accent_end: Rgba::rgb(146, 64, 14),
    accent_soft: Rgba::new(180, 124, 18, 28),
    accent_text: Rgba::WHITE,
    danger: Rgba::rgb(220, 38, 38),
    success: Rgba::rgb(22, 163, 74),
    shadow: Rgba::new(42, 30, 12, 32),
};

pub const PRESETS: &[PresetMeta] = &[
    PresetMeta { id: ThemePreset::Studio, name: "Modern Studio", tagline: "Slate & cyan-indigo", dark: STUDIO_DARK, light: STUDIO_LIGHT },
    PresetMeta { id: ThemePreset::Crimson, name: "Crimson Noir", tagline: "Obsidian & ruby red", dark: CRIMSON_DARK, light: CRIMSON_LIGHT },
    PresetMeta { id: ThemePreset::Amethyst, name: "Midnight Amethyst", tagline: "Void & deep purple", dark: AMETHYST_DARK, light: AMETHYST_LIGHT },
    PresetMeta { id: ThemePreset::Cyber, name: "Cyber Neon", tagline: "Synthwave violet-pink", dark: CYBER_DARK, light: CYBER_LIGHT },
    PresetMeta { id: ThemePreset::Nord, name: "Nord Frost", tagline: "Arctic blue-slate", dark: NORD_DARK, light: NORD_LIGHT },
    PresetMeta { id: ThemePreset::Forest, name: "Emerald Forest", tagline: "Deep pine & teal", dark: FOREST_DARK, light: FOREST_LIGHT },
    PresetMeta { id: ThemePreset::Sunset, name: "Sunset Ember", tagline: "Warm coral & amber", dark: SUNSET_DARK, light: SUNSET_LIGHT },
    PresetMeta { id: ThemePreset::Rose, name: "Rose Quartz", tagline: "Plum & soft orchid", dark: ROSE_DARK, light: ROSE_LIGHT },
    PresetMeta { id: ThemePreset::Mono, name: "Monochrome Pro", tagline: "Minimal OLED ink", dark: MONO_DARK, light: MONO_LIGHT },
    PresetMeta { id: ThemePreset::Terminal, name: "Retro Terminal", tagline: "CRT matrix green", dark: TERMINAL_DARK, light: TERMINAL_LIGHT },
    PresetMeta { id: ThemePreset::Arcane, name: "Arcane Gold", tagline: "Velvet & royal gold", dark: ARCANE_DARK, light: ARCANE_LIGHT },
];

pub fn preset_meta(id: ThemePreset) -> &'static PresetMeta {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

pub fn system_is_dark() -> bool {
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

thread_local! {
    /// Temporary live-preview override while Settings is open.
    static PREVIEW: Cell<Option<(Pref, ThemePreset)>> = const { Cell::new(None) };
}

/// Temporarily previews `(pref, preset)` in every open Snap It window; `None` restores saved settings.
pub fn set_preview(p: Option<(Pref, ThemePreset)>) {
    let prev = PREVIEW.with(|c| c.replace(p));
    if prev != p {
        super::broadcast_theme();
    }
}

/// Clears any active preview without broadcasting (used right before saving new settings).
pub fn end_preview() -> bool {
    PREVIEW.with(|c| c.replace(None)).is_some()
}

pub fn resolve(pref: Pref, preset: ThemePreset) -> Theme {
    let dark = match pref {
        Pref::Dark => true,
        Pref::Light => false,
        Pref::System => system_is_dark(),
    };
    let meta = preset_meta(preset);
    if dark { meta.dark } else { meta.light }
}

pub fn current() -> Theme {
    let (pref, preset) = PREVIEW.with(|c| c.get()).unwrap_or_else(|| {
        let s = crate::app::settings();
        (s.theme, s.theme_preset)
    });
    resolve(pref, preset)
}

/// Applies Windows 10/11 DWM frame styling (dark mode, rounded corners, border colour).
pub fn apply_titlebar(hwnd: HWND, t: &Theme) {
    let dark: u32 = t.dark as u32;
    let corner: u32 = DWMWCP_ROUND.0 as u32;
    let border_cref: u32 = (t.border.r as u32) | ((t.border.g as u32) << 8) | ((t.border.b as u32) << 16);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border_cref as *const _ as *const _, 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_resolve_dark_and_light() {
        assert_eq!(PRESETS.len(), 11);
        for p in PRESETS {
            let d = resolve(Pref::Dark, p.id);
            let l = resolve(Pref::Light, p.id);
            assert!(d.dark, "{} dark variant must be dark", p.name);
            assert!(!l.dark, "{} light variant must be light", p.name);
            let _ = d.accent_paint(0.0, 0.0, 100.0, 40.0);
            let _ = l.accent_hover_paint(0.0, 0.0, 100.0, 40.0);
        }
    }
}
