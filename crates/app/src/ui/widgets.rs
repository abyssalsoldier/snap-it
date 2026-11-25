//! Reusable custom-drawn Modern Studio widgets built on `tiny-skia` and `ab_glyph`.
//!
//! Shared across Settings, Welcome, QR Maker, and History so every window shares the
//! same tactile cards, toggle switches, segmented controls, buttons, text inputs,
//! hotkey keycap recorders, and slim scrollbars.

use std::cell::Cell;

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, LineCap, PathBuilder, PixmapMut, Stroke, Transform};
use snapit_gfx::{text, Rect, Rgba};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};

use super::icons::{self, Icon};
use super::theme::Theme;
use crate::hotkeys::Chord;

// ---------- Typography helpers ----------

/// Word-wraps `s` to fit within `max_w` pixels at font `size`. Respects explicit `\n`.
pub fn wrap_text(s: &str, size: f32, bold: bool, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in s.split('\n') {
        if para.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && text::layout(&cand, size, bold).width > max_w {
                lines.push(cur);
                cur = word.to_string();
            } else {
                cur = cand;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draws word-wrapped text starting at `(x, y)` and returns the total height drawn.
pub fn draw_wrapped(
    pm: &mut PixmapMut,
    s: &str,
    x: f32,
    y: f32,
    max_w: f32,
    size: f32,
    bold: bool,
    color: Rgba,
) -> f32 {
    let lines = wrap_text(s, size, bold, max_w);
    let lh = text::layout("Ag", size, bold).line_height * 1.15;
    for (i, line) in lines.iter().enumerate() {
        if !line.is_empty() {
            text::draw(pm, line, x, y + i as f32 * lh, size, bold, color, None);
        }
    }
    lines.len() as f32 * lh
}

/// Truncates `s` with `…` so it fits within `max_w` pixels.
pub fn ellipsize(s: &str, size: f32, bold: bool, max_w: f32) -> String {
    if text::layout(s, size, bold).width <= max_w {
        return s.to_string();
    }
    let mut out = s.to_string();
    while !out.is_empty() {
        out.pop();
        let cand = format!("{out}…");
        if text::layout(&cand, size, bold).width <= max_w {
            return cand;
        }
    }
    "…".to_string()
}

// ---------- Surfaces & Cards ----------

/// Draws an elevated rounded card surface with a 1px subtle hairline border.
pub fn draw_card(pm: &mut PixmapMut, t: &Theme, r: Rect, radius: f32, scale: f32) {
    let id = Transform::identity();
    let rad = radius * scale;
    if let Some(p) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, rad) {
        pm.fill_path(&p, &t.panel.paint(), FillRule::Winding, id, None);
        pm.stroke_path(&p, &t.border_subtle.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
    }
}

/// Draws a subtle pill badge (e.g. `(new)` or `region`).
pub fn draw_badge(pm: &mut PixmapMut, t: &Theme, x: f32, y: f32, label: &str, accent: bool, scale: f32) -> f32 {
    let id = Transform::identity();
    let fs = 10.5 * scale;
    let l = text::layout(label, fs, true);
    let pw = l.width + 12.0 * scale;
    let ph = 18.0 * scale;
    if let Some(p) = rounded_rect(x, y, pw, ph, ph / 2.0) {
        if accent {
            pm.fill_path(&p, &t.accent_paint(x, y, x + pw, y + ph), FillRule::Winding, id, None);
        } else {
            pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
            pm.stroke_path(&p, &t.border_subtle.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
    }
    let fg = if accent { t.accent_text } else { t.subtext };
    text::draw(pm, label, x + 6.0 * scale, y + (ph - l.height) / 2.0, fs, true, fg, None);
    pw
}

// ---------- Buttons ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtnStyle {
    Primary,
    Secondary,
    Ghost,
}

pub fn draw_button(
    pm: &mut PixmapMut,
    t: &Theme,
    r: Rect,
    label: &str,
    icon: Option<Icon>,
    style: BtnStyle,
    hovered: bool,
    disabled: bool,
    scale: f32,
) {
    let id = Transform::identity();
    let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    let rad = 8.0 * scale;
    if let Some(p) = rounded_rect(x, y, w, h, rad) {
        match style {
            BtnStyle::Primary => {
                if disabled {
                    pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
                } else {
                    let paint = if hovered { t.accent_hover_paint(x, y, x + w, y + h) } else { t.accent_paint(x, y, x + w, y + h) };
                    pm.fill_path(&p, &paint, FillRule::Winding, id, None);
                }
            }
            BtnStyle::Secondary => {
                let fill = if disabled { t.panel } else if hovered { t.hover } else { t.raised };
                pm.fill_path(&p, &fill.paint(), FillRule::Winding, id, None);
                pm.stroke_path(&p, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
            }
            BtnStyle::Ghost => {
                if hovered && !disabled {
                    pm.fill_path(&p, &t.hover.paint(), FillRule::Winding, id, None);
                }
            }
        }
    }
    let fg = if disabled {
        t.subtext.with_alpha(120)
    } else if style == BtnStyle::Primary {
        t.accent_text
    } else if style == BtnStyle::Ghost && !hovered {
        t.subtext
    } else {
        t.text
    };
    let fs = 13.0 * scale;
    let bold = style == BtnStyle::Primary;
    let tl = text::layout(label, fs, bold);
    let gap = if icon.is_some() && !label.is_empty() { 6.0 * scale } else { 0.0 };
    let isz = if icon.is_some() { 15.0 * scale } else { 0.0 };
    let total_w = isz + gap + tl.width;
    let sx = x + (w - total_w) / 2.0;
    if let Some(ic) = icon {
        icons::draw(pm, ic, sx, y + (h - isz) / 2.0, isz, fg);
    }
    if !label.is_empty() {
        text::draw(pm, label, sx + isz + gap, y + (h - tl.height) / 2.0, fs, bold, fg, None);
    }
}

// ---------- Toggle Switch ----------

/// Draws a pill toggle switch inside `r` (centered vertically, right-aligned or filling `r`).
pub fn draw_toggle(pm: &mut PixmapMut, t: &Theme, r: Rect, on: bool, hovered: bool, scale: f32) {
    let id = Transform::identity();
    let tw = 38.0 * scale;
    let th = 22.0 * scale;
    let x = r.x as f32 + (r.w as f32 - tw) / 2.0;
    let y = r.y as f32 + (r.h as f32 - th) / 2.0;
    if let Some(track) = rounded_rect(x, y, tw, th, th / 2.0) {
        if on {
            let paint = if hovered { t.accent_hover_paint(x, y, x + tw, y + th) } else { t.accent_paint(x, y, x + tw, y + th) };
            pm.fill_path(&track, &paint, FillRule::Winding, id, None);
        } else {
            let bg = if hovered { t.hover } else { t.raised };
            pm.fill_path(&track, &bg.paint(), FillRule::Winding, id, None);
            pm.stroke_path(&track, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
    }
    let knob_r = (th - 6.0 * scale) / 2.0;
    let kx = if on { x + tw - 3.0 * scale - knob_r } else { x + 3.0 * scale + knob_r };
    let ky = y + th / 2.0;
    if let Some(knob) = PathBuilder::from_circle(kx, ky, knob_r) {
        let kc = if on { Rgba::WHITE } else { t.subtext };
        pm.fill_path(&knob, &kc.paint(), FillRule::Winding, id, None);
    }
}

// ---------- Segmented Control ----------

/// Draws a horizontal segmented pill selector. Returns the rect of each option.
pub fn segmented_rects(r: Rect, count: usize, scale: f32) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let pad = (3.0 * scale).round() as i32;
    let inner_w = (r.w - pad * 2).max(1);
    let seg_w = inner_w / count as i32;
    (0..count)
        .map(|i| {
            let x = r.x + pad + i as i32 * seg_w;
            let w = if i + 1 == count { r.right() - pad - x } else { seg_w };
            Rect::new(x, r.y + pad, w.max(1), (r.h - pad * 2).max(1))
        })
        .collect()
}

pub fn draw_segmented(
    pm: &mut PixmapMut,
    t: &Theme,
    r: Rect,
    options: &[&str],
    selected: usize,
    hover_idx: Option<usize>,
    scale: f32,
) {
    let id = Transform::identity();
    if let Some(bg) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 8.0 * scale) {
        pm.fill_path(&bg, &t.input_bg.paint(), FillRule::Winding, id, None);
        pm.stroke_path(&bg, &t.border_subtle.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
    }
    let rects = segmented_rects(r, options.len(), scale);
    let fs = 12.5 * scale;
    for (i, (opt, sr)) in options.iter().zip(&rects).enumerate() {
        let on = i == selected;
        let hov = hover_idx == Some(i);
        if on || hov {
            if let Some(p) = rounded_rect(sr.x as f32, sr.y as f32, sr.w as f32, sr.h as f32, 6.0 * scale) {
                if on {
                    pm.fill_path(
                        &p,
                        &t.accent_paint(sr.x as f32, sr.y as f32, sr.right() as f32, sr.bottom() as f32),
                        FillRule::Winding,
                        id,
                        None,
                    );
                } else {
                    pm.fill_path(&p, &t.hover.paint(), FillRule::Winding, id, None);
                }
            }
        }
        let fg = if on { t.accent_text } else if hov { t.text } else { t.subtext };
        let l = text::layout(opt, fs, on);
        text::draw(
            pm,
            opt,
            sr.x as f32 + (sr.w as f32 - l.width) / 2.0,
            sr.y as f32 + (sr.h as f32 - l.height) / 2.0,
            fs,
            on,
            fg,
            None,
        );
    }
}

// ---------- Custom TextInput ----------

#[derive(Clone, Debug)]
pub struct TextInput {
    pub value: String,
    pub placeholder: &'static str,
    pub numeric: bool,
    pub multiline: bool,
    /// Cursor position in char indices (`0..=chars().count()`).
    pub cursor: usize,
    pub select_all: bool,
}

impl TextInput {
    pub fn new(value: impl Into<String>, placeholder: &'static str, numeric: bool, multiline: bool) -> Self {
        let v = value.into();
        let len = v.chars().count();
        TextInput { value: v, placeholder, numeric, multiline, cursor: len, select_all: false }
    }

    pub fn set(&mut self, s: impl Into<String>) {
        self.value = s.into();
        self.cursor = self.value.chars().count();
        self.select_all = false;
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.value.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(self.value.len())
    }

    fn clear_if_selected(&mut self) {
        if self.select_all {
            self.value.clear();
            self.cursor = 0;
            self.select_all = false;
        }
    }

    /// Handles `WM_CHAR`. Returns `true` if the value changed.
    pub fn on_char(&mut self, ch: char) -> bool {
        if ch.is_control() {
            if ch == '\r' && self.multiline {
                self.clear_if_selected();
                let b = self.byte_idx(self.cursor);
                self.value.insert(b, '\n');
                self.cursor += 1;
                return true;
            }
            return false;
        }
        if self.numeric && !ch.is_ascii_digit() {
            return false;
        }
        self.clear_if_selected();
        let b = self.byte_idx(self.cursor);
        self.value.insert(b, ch);
        self.cursor += 1;
        true
    }

    /// Handles `WM_KEYDOWN`. Returns `true` if state/value changed.
    pub fn on_key(&mut self, vk: u32, ctrl: bool) -> bool {
        let len = self.value.chars().count();
        if ctrl {
            match vk {
                0x41 => {
                    // Ctrl+A
                    self.select_all = !self.value.is_empty();
                    self.cursor = len;
                    return true;
                }
                0x43 => {
                    // Ctrl+C
                    let _ = crate::clipboard::set_text(&self.value);
                    return false;
                }
                0x58 => {
                    // Ctrl+X
                    let _ = crate::clipboard::set_text(&self.value);
                    self.value.clear();
                    self.cursor = 0;
                    self.select_all = false;
                    return true;
                }
                0x56 => {
                    // Ctrl+V
                    if let Some(clip) = crate::clipboard::get_text() {
                        let clean: String = if self.numeric {
                            clip.chars().filter(|c| c.is_ascii_digit()).collect()
                        } else if self.multiline {
                            clip.replace("\r\n", "\n")
                        } else {
                            clip.chars().filter(|&c| c != '\r' && c != '\n').collect()
                        };
                        if !clean.is_empty() {
                            self.clear_if_selected();
                            let b = self.byte_idx(self.cursor);
                            self.value.insert_str(b, &clean);
                            self.cursor += clean.chars().count();
                            return true;
                        }
                    }
                    return false;
                }
                _ => return false,
            }
        }
        match vk {
            0x08 => {
                // Backspace
                if self.select_all {
                    self.value.clear();
                    self.cursor = 0;
                    self.select_all = false;
                    return true;
                }
                if self.cursor > 0 {
                    let b0 = self.byte_idx(self.cursor - 1);
                    let b1 = self.byte_idx(self.cursor);
                    self.value.replace_range(b0..b1, "");
                    self.cursor -= 1;
                    return true;
                }
                false
            }
            0x2E => {
                // Delete
                if self.select_all {
                    self.value.clear();
                    self.cursor = 0;
                    self.select_all = false;
                    return true;
                }
                if self.cursor < len {
                    let b0 = self.byte_idx(self.cursor);
                    let b1 = self.byte_idx(self.cursor + 1);
                    self.value.replace_range(b0..b1, "");
                    return true;
                }
                false
            }
            0x25 => {
                // Left
                self.select_all = false;
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                true
            }
            0x27 => {
                // Right
                self.select_all = false;
                if self.cursor < len {
                    self.cursor += 1;
                }
                true
            }
            0x24 => {
                // Home
                self.select_all = false;
                self.cursor = 0;
                true
            }
            0x23 => {
                // End
                self.select_all = false;
                self.cursor = len;
                true
            }
            _ => false,
        }
    }

    /// Draws the custom text input box.
    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme, r: Rect, focused: bool, hovered: bool, scale: f32) {
        let id = Transform::identity();
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let rad = 7.0 * scale;
        if let Some(p) = rounded_rect(x, y, w, h, rad) {
            pm.fill_path(&p, &t.input_bg.paint(), FillRule::Winding, id, None);
            let (bc, bw) = if focused {
                (t.accent, 1.6 * scale)
            } else if hovered {
                (t.subtext.with_alpha(160), 1.0)
            } else {
                (t.border, 1.0)
            };
            pm.stroke_path(&p, &bc.paint(), &Stroke { width: bw, ..Default::default() }, id, None);
        }
        let pad_x = 10.0 * scale;
        let fs = 13.0 * scale;
        let inner_w = (w - pad_x * 2.0).max(10.0);

        if self.value.is_empty() {
            if !self.placeholder.is_empty() {
                let l = text::layout(self.placeholder, fs, false);
                let ty = if self.multiline { y + 8.0 * scale } else { y + (h - l.height) / 2.0 };
                let ph = ellipsize(self.placeholder, fs, false, inner_w);
                text::draw(pm, &ph, x + pad_x, ty, fs, false, t.subtext.with_alpha(150), None);
            }
            if focused {
                let cy = if self.multiline { y + 8.0 * scale } else { y + 6.0 * scale };
                let ch = if self.multiline { 16.0 * scale } else { h - 12.0 * scale };
                if let Some(cr) = rounded_rect((x + pad_x).round(), cy.round(), (2.0 * scale).round().max(1.0), ch.round().max(2.0), 0.5) {
                    pm.fill_path(&cr, &t.accent.paint(), FillRule::Winding, id, None);
                }
            }
            return;
        }

        if self.multiline {
            let lines = wrap_text(&self.value, fs, false, inner_w);
            let lh = text::layout("Ag", fs, false).line_height * 1.15;
            let max_lines = ((h - 14.0 * scale) / lh).floor().max(1.0) as usize;
            let start = lines.len().saturating_sub(max_lines);
            let visible = &lines[start..];
            if focused && self.select_all {
                if let Some(sr) = rounded_rect(x + pad_x - 2.0, y + 6.0 * scale, inner_w + 4.0, visible.len() as f32 * lh, 4.0) {
                    pm.fill_path(&sr, &t.accent_soft.paint(), FillRule::Winding, id, None);
                }
            }
            for (i, line) in visible.iter().enumerate() {
                text::draw(pm, line, x + pad_x, y + 7.0 * scale + i as f32 * lh, fs, false, t.text, None);
            }
            if focused && !self.select_all {
                let last_line = visible.last().map(String::as_str).unwrap_or("");
                let lw = text::layout(last_line, fs, false).width.min(inner_w);
                let cx = (x + pad_x + lw + 1.0).round();
                let cy = (y + 7.0 * scale + (visible.len().saturating_sub(1)) as f32 * lh).round();
                if let Some(cr) = rounded_rect(cx, cy, (2.0 * scale).round().max(1.0), (lh * 0.9).round().max(2.0), 0.5) {
                    pm.fill_path(&cr, &t.accent.paint(), FillRule::Winding, id, None);
                }
            }
        } else {
            // Single-line: scroll text left if cursor extends past inner_w.
            let prefix: String = self.value.chars().take(self.cursor).collect();
            let full_l = text::layout(&self.value, fs, false);
            let pref_w = text::layout(&prefix, fs, false).width;
            let ty = y + (h - full_l.height) / 2.0;

            let shown = if full_l.width <= inner_w {
                self.value.clone()
            } else if focused {
                // Keep the cursor visible near the right edge.
                let mut s = self.value.clone();
                while text::layout(&s, fs, false).width > inner_w && s.chars().count() > 1 {
                    s.remove(0);
                }
                s
            } else {
                ellipsize(&self.value, fs, false, inner_w)
            };
            let shown_w = text::layout(&shown, fs, false).width;

            if focused && self.select_all {
                if let Some(sr) = rounded_rect(x + pad_x - 2.0, y + 4.0 * scale, shown_w.min(inner_w) + 4.0, h - 8.0 * scale, 4.0) {
                    pm.fill_path(&sr, &t.accent_soft.paint(), FillRule::Winding, id, None);
                }
            }
            text::draw(pm, &shown, x + pad_x, ty, fs, false, t.text, None);
            if focused && !self.select_all {
                let cx = if full_l.width <= inner_w {
                    x + pad_x + pref_w
                } else {
                    x + pad_x + shown_w.min(inner_w)
                };
                if let Some(cr) = rounded_rect(cx.round(), (y + 6.0 * scale).round(), (2.0 * scale).round().max(1.0), (h - 12.0 * scale).round().max(2.0), 0.5) {
                    pm.fill_path(&cr, &t.accent.paint(), FillRule::Winding, id, None);
                }
            }
        }
    }
}

// ---------- Hotkey Recorder & Keycaps ----------

thread_local! {
    /// Global hotkeys are unregistered while any custom hotkey field is focused.
    static HOTKEYS_PAUSED: Cell<bool> = const { Cell::new(false) };
}

pub fn pause_hotkeys() {
    if !HOTKEYS_PAUSED.with(|p| p.replace(true)) {
        crate::suspend_hotkeys();
    }
}

pub fn resume_hotkeys() {
    if HOTKEYS_PAUSED.with(|p| p.replace(false)) {
        let failed = crate::reregister_hotkeys();
        if !failed.is_empty() {
            crate::log(&format!("hotkeys unavailable: {}", failed.join(", ")));
        }
    }
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

pub fn current_mods() -> u32 {
    let mut m = 0;
    if key_down(0x11) {
        m |= MOD_CONTROL.0;
    }
    if key_down(0x12) {
        m |= MOD_ALT.0;
    }
    if key_down(0x10) {
        m |= MOD_SHIFT.0;
    }
    if key_down(0x5B) || key_down(0x5C) {
        m |= MOD_WIN.0;
    }
    m
}

/// The chord for `vk` pressed with `mods`, if it is one Snap It can save and register.
pub fn chord_for(mods: u32, vk: u32) -> Option<Chord> {
    if is_modifier(vk) {
        return None;
    }
    let c = Chord { mods, vk };
    (Chord::parse(&c.display()) == Some(c)).then_some(c)
}

/// Handles a keydown/syskeydown or PrintScreen keyup on a focused hotkey field.
/// Returns `Some(new_text)` if the field changed (empty string means cleared), `None` otherwise.
pub fn handle_hotkey_msg(is_keyup: bool, vk: u32) -> Option<String> {
    const VK_BACK: u32 = 0x08;
    const VK_SNAPSHOT: u32 = 0x2C;
    const VK_DELETE: u32 = 0x2E;

    let mods = current_mods();
    if is_keyup {
        if vk == VK_SNAPSHOT {
            return chord_for(mods, vk).map(|c| c.display());
        }
        return None;
    }
    match vk {
        VK_BACK | VK_DELETE if mods == 0 => Some(String::new()),
        _ => chord_for(mods, vk).map(|c| c.display()),
    }
}

/// Draws a hotkey input box showing styled keycap badges (e.g. `[Ctrl]` `[Shift]` `[S]`).
pub fn draw_hotkey_field(
    pm: &mut PixmapMut,
    t: &Theme,
    r: Rect,
    value: &str,
    focused: bool,
    hovered: bool,
    scale: f32,
) {
    let id = Transform::identity();
    let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    if let Some(p) = rounded_rect(x, y, w, h, 7.0 * scale) {
        pm.fill_path(&p, &t.input_bg.paint(), FillRule::Winding, id, None);
        let (bc, bw) = if focused {
            (t.accent, 1.6 * scale)
        } else if hovered {
            (t.subtext.with_alpha(160), 1.0)
        } else {
            (t.border, 1.0)
        };
        pm.stroke_path(&p, &bc.paint(), &Stroke { width: bw, ..Default::default() }, id, None);
    }

    let trimmed = value.trim();
    if trimmed.is_empty() {
        let msg = if focused { "Press keys…" } else { "Not set" };
        let fs = 12.0 * scale;
        let l = text::layout(msg, fs, false);
        let c = if focused { t.accent } else { t.subtext.with_alpha(140) };
        text::draw(pm, msg, x + 10.0 * scale, y + (h - l.height) / 2.0, fs, false, c, None);
        return;
    }

    // Draw each key in the chord as a tactile keycap pill.
    let fs = 11.5 * scale;
    let kh = (h - 8.0 * scale).max(16.0 * scale);
    let ky = y + (h - kh) / 2.0;
    let mut kx = x + 6.0 * scale;
    for (i, part) in trimmed.split('+').map(str::trim).filter(|p| !p.is_empty()).enumerate() {
        if i > 0 {
            let plus = text::layout("+", fs, false);
            text::draw(pm, "+", kx, y + (h - plus.height) / 2.0, fs, false, t.subtext, None);
            kx += plus.width + 4.0 * scale;
        }
        let l = text::layout(part, fs, true);
        let kw = (l.width + 12.0 * scale).min((x + w - 6.0 * scale - kx).max(16.0 * scale));
        if let Some(kp) = rounded_rect(kx, ky, kw, kh, 5.0 * scale) {
            pm.fill_path(&kp, &t.raised.paint(), FillRule::Winding, id, None);
            pm.stroke_path(&kp, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, id, None);
        }
        text::draw(pm, part, kx + 6.0 * scale, ky + (kh - l.height) / 2.0, fs, true, t.text, None);
        kx += kw + 4.0 * scale;
        if kx > x + w - 12.0 * scale {
            break;
        }
    }
}

// ---------- Slim Scrollbar ----------

pub fn draw_scrollbar(
    pm: &mut PixmapMut,
    t: &Theme,
    track: Rect,
    scroll: f32,
    content_h: f32,
    scale: f32,
) {
    let view_h = track.h as f32;
    if content_h <= view_h + 1.0 {
        return;
    }
    let ratio = (view_h / content_h).clamp(0.08, 1.0);
    let thumb_h = (view_h * ratio).max(28.0 * scale);
    let max_scroll = (content_h - view_h).max(1.0);
    let frac = (scroll / max_scroll).clamp(0.0, 1.0);
    let thumb_y = track.y as f32 + frac * (view_h - thumb_h);
    let w = 5.0 * scale;
    let x = track.right() as f32 - w - 3.0 * scale;
    if let Some(p) = rounded_rect(x, thumb_y, w, thumb_h, w / 2.0) {
        pm.fill_path(&p, &t.border.with_alpha(190).paint(), FillRule::Winding, Transform::identity(), None);
    }
}

/// Draws a crisp hairline horizontal separator.
pub fn draw_divider(pm: &mut PixmapMut, t: &Theme, x0: f32, x1: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y);
    pb.line_to(x1, y);
    if let Some(p) = pb.finish() {
        let st = Stroke { width: 1.0, line_cap: LineCap::Butt, ..Default::default() };
        pm.stroke_path(&p, &t.border_subtle.paint(), &st, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_chords_round_trip() {
        let ctrl_shift = MOD_CONTROL.0 | MOD_SHIFT.0;
        assert_eq!(chord_for(ctrl_shift, 0x2C).unwrap().display(), "Ctrl+Shift+PrintScreen");
        assert_eq!(chord_for(MOD_ALT.0, 0x70).unwrap().display(), "Alt+F1");
        assert_eq!(chord_for(0, 0x2C).unwrap().display(), "PrintScreen");
        assert_eq!(chord_for(MOD_WIN.0 | MOD_SHIFT.0, b'S' as u32).unwrap().display(), "Shift+Win+S");
        assert!(chord_for(MOD_CONTROL.0, 0x11).is_none());
        assert!(chord_for(0, 0xFF).is_none());
    }
}
