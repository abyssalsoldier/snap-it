//! Custom-drawn toolbar and popups, shared by the quick-annotate overlay and the editor.

use snapit_annotate::{stickers, Bx, Sticker};
use snapit_gfx::effects::rounded_rect;
use snapit_gfx::text;
use snapit_gfx::tiny_skia::{FillRule, PathBuilder, PixmapMut, Stroke, Transform};
use snapit_gfx::{Rect, Rgba};

use super::icons::{self, Icon};
use super::theme::Theme;

pub struct Button<B> {
    pub id: B,
    pub icon: Icon,
    pub tip: &'static str,
    pub sep_before: bool,
    /// Horizontal toolbars only: start a new row with this button.
    pub row_before: bool,
}

pub fn b<B>(id: B, icon: Icon, tip: &'static str) -> Button<B> {
    Button { id, icon, tip, sep_before: false, row_before: false }
}

pub fn sep<B>(id: B, icon: Icon, tip: &'static str) -> Button<B> {
    Button { id, icon, tip, sep_before: true, row_before: false }
}

/// First button of a new row (horizontal toolbars).
pub fn row<B>(id: B, icon: Icon, tip: &'static str) -> Button<B> {
    Button { id, icon, tip, sep_before: false, row_before: true }
}

pub struct Toolbar<B> {
    pub buttons: Vec<Button<B>>,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub hover: Option<usize>,
    pub vertical: bool,
    /// Button size in unscaled pixels.
    pub btn: f32,
}

pub const BTN: f32 = 34.0;
const PAD: f32 = 5.0;
const SEP: f32 = 9.0;
/// Gap between rows.
const ROW_GAP: f32 = 2.0;

impl<B: Copy + PartialEq> Toolbar<B> {
    pub fn new(buttons: Vec<Button<B>>, scale: f32) -> Self {
        Toolbar { buttons, x: 0.0, y: 0.0, scale, hover: None, vertical: false, btn: BTN }
    }

    /// Unscaled (row, offset along the row) of each button.
    fn slots(&self) -> Vec<(usize, f32)> {
        let mut row = 0;
        let mut along = PAD;
        let mut out = Vec::with_capacity(self.buttons.len());
        for (j, b) in self.buttons.iter().enumerate() {
            if b.row_before && !self.vertical && j > 0 {
                row += 1;
                along = PAD;
            } else if b.sep_before && j > 0 {
                along += SEP;
            }
            out.push((row, along));
            along += self.btn;
        }
        out
    }

    pub fn size(&self) -> (f32, f32) {
        let s = self.scale;
        let slots = self.slots();
        let rows = slots.last().map_or(1, |l| l.0 + 1) as f32;
        let long = slots.iter().map(|(_, a)| a + self.btn).fold(0.0, f32::max) + PAD;
        let short = rows * self.btn + (rows - 1.0) * ROW_GAP + PAD * 2.0;
        if self.vertical { (short * s, long * s) } else { (long * s, short * s) }
    }

    pub fn rect(&self) -> Rect {
        let (w, h) = self.size();
        Rect::new(self.x as i32, self.y as i32, w.ceil() as i32, h.ceil() as i32)
    }

    /// Bounds including the tooltip area below, for dirty-rect tracking.
    pub fn paint_bounds(&self) -> Rect {
        let r = self.rect();
        let tip_h = (30.0 * self.scale) as i32;
        Rect::new(r.x - 200, r.y - tip_h, r.w + 400, r.h + tip_h * 2)
    }

    fn button_rect(&self, i: usize) -> (f32, f32, f32) {
        let s = self.scale;
        let (row, along) = self.slots().get(i).copied().unwrap_or((0, PAD));
        let across = PAD + row as f32 * (self.btn + ROW_GAP);
        if self.vertical {
            (self.x + across * s, self.y + along * s, self.btn * s)
        } else {
            (self.x + along * s, self.y + across * s, self.btn * s)
        }
    }

    pub fn button_bounds(&self, id: B) -> Option<Rect> {
        let i = self.buttons.iter().position(|b| b.id == id)?;
        let (x, y, sz) = self.button_rect(i);
        Some(Rect::new(x as i32, y as i32, sz as i32, sz as i32))
    }

    pub fn index_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.rect().contains(x, y) {
            return None;
        }
        (0..self.buttons.len()).find(|&i| {
            let (bx, by, s) = self.button_rect(i);
            (x as f32) >= bx && (x as f32) < bx + s && (y as f32) >= by && (y as f32) < by + s
        })
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<B> {
        self.index_at(x, y).map(|i| self.buttons[i].id)
    }

    /// Updates hover; returns true if it changed.
    pub fn set_hover(&mut self, x: i32, y: i32) -> bool {
        let h = self.index_at(x, y);
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    /// `active` marks toggled buttons; `swatch` replaces one button's icon with a colour dot.
    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme, active: impl Fn(B) -> bool, swatch: Option<(B, Rgba, f32)>) {
        let s = self.scale;
        let (w, h) = self.size();
        if let Some(bg) = rounded_rect(self.x, self.y, w, h, 9.0 * s) {
            // soft shadow
            if let Some(sh) = rounded_rect(self.x + 1.0, self.y + 3.0 * s, w, h, 9.0 * s) {
                pm.fill_path(&sh, &Rgba::new(0, 0, 0, 60).paint(), FillRule::Winding, Transform::identity(), None);
            }
            pm.fill_path(&bg, &t.panel.paint(), FillRule::Winding, Transform::identity(), None);
            pm.stroke_path(&bg, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
        for (i, btn) in self.buttons.iter().enumerate() {
            let (bx, by, sz) = self.button_rect(i);
            if btn.sep_before && !(btn.row_before && !self.vertical) && i > 0 {
                let sx = bx - SEP * s / 2.0;
                let mut pb = PathBuilder::new();
                pb.move_to(sx, by + sz * 0.2);
                pb.line_to(sx, by + sz * 0.8);
                if let Some(p) = pb.finish() {
                    pm.stroke_path(&p, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
                }
            }
            let on = active(btn.id);
            let fill = if on { Some(t.accent) } else if self.hover == Some(i) { Some(t.hover) } else { None };
            if let Some(f) = fill {
                if let Some(p) = rounded_rect(bx + 1.0, by + 1.0, sz - 2.0, sz - 2.0, 6.0 * s) {
                    pm.fill_path(&p, &f.paint(), FillRule::Winding, Transform::identity(), None);
                }
            }
            let fg = if on { t.accent_text } else { t.text };
            match swatch {
                Some((id, c, width)) if id == btn.id => {
                    let r = (sz * 0.18 + width.min(16.0) * 0.6 * s).min(sz * 0.36);
                    if let Some(dot) = PathBuilder::from_circle(bx + sz / 2.0, by + sz / 2.0, r) {
                        pm.fill_path(&dot, &c.paint(), FillRule::Winding, Transform::identity(), None);
                        pm.stroke_path(&dot, &t.text.with_alpha(160).paint(), &Stroke { width: 1.2, ..Default::default() }, Transform::identity(), None);
                    }
                }
                _ => {
                    // Bigger buttons get proportionally bigger glyphs.
                    let k = if self.btn > BTN { 0.68 } else { 0.64 };
                    let m = (1.0 - k) / 2.0;
                    icons::draw(pm, btn.icon, bx + sz * m, by + sz * m, sz * k, fg)
                }
            }
        }
        if let Some(i) = self.hover {
            let (bx, by, sz) = self.button_rect(i);
            let tip = self.buttons[i].tip;
            if self.vertical {
                let l = text::layout(tip, 12.5 * s, false);
                let w = l.width + 12.0 * s;
                draw_tip(pm, t, tip, bx + sz + PAD * s + 8.0 * s + w / 2.0, by + sz * 0.2, s);
            } else {
                // Below the whole bar, so a tip never covers a lower row.
                draw_tip(pm, t, tip, bx + sz / 2.0, self.y + h + 6.0 * s, s);
            }
        }
    }
}

/// A small label bubble centred horizontally on `cx`, top at `y`.
pub fn draw_tip(pm: &mut PixmapMut, t: &Theme, tip: &str, cx: f32, y: f32, s: f32) {
    let size = 12.5 * s;
    let l = text::layout(tip, size, false);
    let pad = 6.0 * s;
    let (w, h) = (l.width + pad * 2.0, l.height + pad);
    let x = (cx - w / 2.0).max(2.0);
    if let Some(p) = rounded_rect(x, y, w, h, 5.0 * s) {
        pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, Transform::identity(), None);
    }
    text::draw(pm, tip, x + pad, y + pad / 2.0, size, false, t.text, None);
}

/// Colour, width, text size and fill choices.
pub struct StylePopup {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub palette: Vec<Rgba>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StyleHit {
    Color(Rgba),
    Width(f32),
    FontSize(f32),
    ToggleFill,
}

pub const WIDTHS: [f32; 4] = [2.0, 4.0, 7.0, 12.0];
pub const FONT_SIZES: [f32; 4] = [16.0, 22.0, 32.0, 48.0];
const SW: f32 = 26.0;

impl StylePopup {
    fn cells(&self) -> Vec<(Rect, StyleHit)> {
        let s = self.scale;
        let mut v = Vec::new();
        let (x0, y0) = (self.x + 8.0 * s, self.y + 8.0 * s);
        for (i, c) in self.palette.iter().enumerate() {
            let x = x0 + i as f32 * (SW + 4.0) * s;
            v.push((Rect::new(x as i32, y0 as i32, (SW * s) as i32, (SW * s) as i32), StyleHit::Color(*c)));
        }
        let y1 = y0 + (SW + 8.0) * s;
        for (i, w) in WIDTHS.iter().enumerate() {
            let x = x0 + i as f32 * (SW + 4.0) * s;
            v.push((Rect::new(x as i32, y1 as i32, (SW * s) as i32, (SW * s) as i32), StyleHit::Width(*w)));
        }
        let xf = x0 + 4.5 * (SW + 4.0) * s;
        for (i, f) in FONT_SIZES.iter().enumerate() {
            let x = xf + i as f32 * (SW + 4.0) * s;
            v.push((Rect::new(x as i32, y1 as i32, (SW * s) as i32, (SW * s) as i32), StyleHit::FontSize(*f)));
        }
        let xt = xf + 4.0 * (SW + 4.0) * s + 6.0 * s;
        v.push((Rect::new(xt as i32, y1 as i32, (SW * s) as i32, (SW * s) as i32), StyleHit::ToggleFill));
        v
    }

    pub fn rect(&self) -> Rect {
        let s = self.scale;
        let cols = (self.palette.len() as f32).max(10.0);
        Rect::new(self.x as i32, self.y as i32, ((cols * (SW + 4.0) + 12.0) * s) as i32, ((SW * 2.0 + 24.0) * s) as i32)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<StyleHit> {
        self.cells().into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h)
    }

    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme, color: Rgba, width: f32, font: f32, fill: bool) {
        let s = self.scale;
        let r = self.rect();
        if let Some(bg) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 9.0 * s) {
            pm.fill_path(&bg, &t.panel.paint(), FillRule::Winding, Transform::identity(), None);
            pm.stroke_path(&bg, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
        for (cell, hit) in self.cells() {
            let (x, y, w) = (cell.x as f32, cell.y as f32, cell.w as f32);
            let selected = match hit {
                StyleHit::Color(c) => c == color,
                StyleHit::Width(v) => v == width,
                StyleHit::FontSize(v) => v == font,
                StyleHit::ToggleFill => fill,
            };
            if selected {
                if let Some(p) = rounded_rect(x - 2.0, y - 2.0, w + 4.0, w + 4.0, 6.0 * s) {
                    pm.stroke_path(&p, &t.accent.paint(), &Stroke { width: 2.0 * s, ..Default::default() }, Transform::identity(), None);
                }
            }
            match hit {
                StyleHit::Color(c) => {
                    if let Some(p) = rounded_rect(x, y, w, w, 5.0 * s) {
                        pm.fill_path(&p, &c.paint(), FillRule::Winding, Transform::identity(), None);
                        pm.stroke_path(&p, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
                    }
                }
                StyleHit::Width(v) => {
                    if let Some(d) = PathBuilder::from_circle(x + w / 2.0, y + w / 2.0, (v * 0.55 * s).max(1.5)) {
                        pm.fill_path(&d, &t.text.paint(), FillRule::Winding, Transform::identity(), None);
                    }
                }
                StyleHit::FontSize(v) => {
                    let size = (8.0 + v * 0.25) * s;
                    let l = text::layout("A", size, true);
                    text::draw(pm, "A", x + (w - l.width) / 2.0, y + (w - l.height) / 2.0, size, true, t.text, None);
                }
                StyleHit::ToggleFill => icons::draw(pm, Icon::Fill, x + w * 0.1, y + w * 0.1, w * 0.8, t.text),
            }
        }
    }
}

pub struct StickerPopup {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

impl StickerPopup {
    const CELL: f32 = 40.0;
    pub fn rect(&self) -> Rect {
        let s = self.scale;
        Rect::new(self.x as i32, self.y as i32, ((Sticker::ALL.len() as f32 * Self::CELL + 12.0) * s) as i32, ((Self::CELL + 12.0) * s) as i32)
    }
    fn cell(&self, i: usize) -> Rect {
        let s = self.scale;
        Rect::new((self.x + (6.0 + i as f32 * Self::CELL) * s) as i32, (self.y + 6.0 * s) as i32, (Self::CELL * s) as i32, (Self::CELL * s) as i32)
    }
    pub fn hit(&self, x: i32, y: i32) -> Option<Sticker> {
        (0..Sticker::ALL.len()).find(|&i| self.cell(i).contains(x, y)).map(|i| Sticker::ALL[i])
    }
    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme, current: Option<Sticker>) {
        let s = self.scale;
        let r = self.rect();
        if let Some(bg) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 9.0 * s) {
            pm.fill_path(&bg, &t.panel.paint(), FillRule::Winding, Transform::identity(), None);
            pm.stroke_path(&bg, &t.border.paint(), &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
        for (i, k) in Sticker::ALL.iter().enumerate() {
            let c = self.cell(i);
            if current == Some(*k) {
                if let Some(p) = rounded_rect(c.x as f32, c.y as f32, c.w as f32, c.h as f32, 6.0 * s) {
                    pm.fill_path(&p, &t.hover.paint(), FillRule::Winding, Transform::identity(), None);
                }
            }
            let inset = c.w as f32 * 0.14;
            stickers::draw(pm, *k, Bx { x0: c.x as f32 + inset, y0: c.y as f32 + inset, x1: c.right() as f32 - inset, y1: c.bottom() as f32 - inset });
        }
    }
}
