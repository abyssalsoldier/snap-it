//! Annotation model and renderer.
//!
//! Items use floating-point coordinates in "document space" (the overlay uses
//! virtual-screen pixels relative to the frozen screenshot, the editor uses image
//! pixels). [`render`] draws them onto a pixmap with an offset, so the same
//! items render identically in the overlay, the editor and the exported image.

pub mod stickers;

use std::sync::Arc;

use snapit_gfx::effects;
use snapit_gfx::text;
use serde::{Deserialize, Serialize};
use snapit_gfx::tiny_skia::{
    BlendMode, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pattern,
    Pixmap, PixmapMut, PixmapPaint, SpreadMode, Stroke, StrokeDash, Transform,
};
use snapit_gfx::{PointF, Rect, Rgba};

pub use stickers::Sticker;

/// An axis-aligned box from two corners (not necessarily normalised).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bx {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bx {
    pub fn new(a: PointF, b: PointF) -> Bx {
        Bx { x0: a.x, y0: a.y, x1: b.x, y1: b.y }
    }
    pub fn norm(self) -> Bx {
        Bx { x0: self.x0.min(self.x1), y0: self.y0.min(self.y1), x1: self.x0.max(self.x1), y1: self.y0.max(self.y1) }
    }
    pub fn w(&self) -> f32 {
        (self.x1 - self.x0).abs()
    }
    pub fn h(&self) -> f32 {
        (self.y1 - self.y0).abs()
    }
    pub fn offset(self, dx: f32, dy: f32) -> Bx {
        Bx { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy }
    }
    pub fn to_rect(self) -> Rect {
        let n = self.norm();
        Rect::from_ltrb(n.x0.floor() as i32, n.y0.floor() as i32, n.x1.ceil() as i32, n.y1.ceil() as i32)
    }
    pub fn skia(self) -> Option<snapit_gfx::tiny_skia::Rect> {
        let n = self.norm();
        snapit_gfx::tiny_skia::Rect::from_ltrb(n.x0, n.y0, n.x1, n.y1)
    }
    pub fn contains(&self, p: PointF) -> bool {
        let n = self.norm();
        p.x >= n.x0 && p.x <= n.x1 && p.y >= n.y0 && p.y <= n.y1
    }
}

/// Outline pattern for strokes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dash {
    #[default]
    Solid,
    Dash,
    Dot,
    DashDot,
}

impl Dash {
    pub const ALL: [Dash; 4] = [Dash::Solid, Dash::Dash, Dash::Dot, Dash::DashDot];
    pub fn name(self) -> &'static str {
        match self {
            Dash::Solid => "Solid",
            Dash::Dash => "Dashed",
            Dash::Dot => "Dotted",
            Dash::DashDot => "Dash-dot",
        }
    }
    /// The on/off pattern for a stroke of `width`.
    fn pattern(self, width: f32) -> Option<Vec<f32>> {
        let w = width.max(1.0);
        // Dots are zero-length dashes drawn with round caps.
        let dot = 0.01;
        match self {
            Dash::Solid => None,
            Dash::Dash => Some(vec![w * 3.0, w * 2.0]),
            Dash::Dot => Some(vec![dot, w * 2.0]),
            Dash::DashDot => Some(vec![w * 3.0, w * 2.0, dot, w * 2.0]),
        }
    }
}

/// Which ends of an arrow get a head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArrowHeads {
    #[default]
    End,
    Start,
    Both,
}

impl ArrowHeads {
    pub const ALL: [ArrowHeads; 3] = [ArrowHeads::End, ArrowHeads::Start, ArrowHeads::Both];
    pub fn name(self) -> &'static str {
        match self {
            ArrowHeads::End => "Head at end",
            ArrowHeads::Start => "Head at start",
            ArrowHeads::Both => "Heads at both ends",
        }
    }
}

/// How step markers count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepType {
    #[default]
    Numbers,
    Upper,
    Lower,
    RomanUpper,
    RomanLower,
}

impl StepType {
    pub const ALL: [StepType; 5] = [StepType::Numbers, StepType::Upper, StepType::Lower, StepType::RomanUpper, StepType::RomanLower];
    /// Short sample shown on the option button.
    pub fn sample(self) -> &'static str {
        match self {
            StepType::Numbers => "1",
            StepType::Upper => "A",
            StepType::Lower => "a",
            StepType::RomanUpper => "I",
            StepType::RomanLower => "i",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            StepType::Numbers => "Numbers",
            StepType::Upper => "Letters (A, B, C)",
            StepType::Lower => "Letters (a, b, c)",
            StepType::RomanUpper => "Roman numerals (I, II, III)",
            StepType::RomanLower => "Roman numerals (i, ii, iii)",
        }
    }
    pub fn label(self, n: u32) -> String {
        let letters = |n: u32, base: u8| {
            // 1 → A, 26 → Z, 27 → AA (spreadsheet-style).
            let mut n = n.max(1);
            let mut s = Vec::new();
            while n > 0 {
                n -= 1;
                s.push(base + (n % 26) as u8);
                n /= 26;
            }
            s.reverse();
            String::from_utf8(s).unwrap_or_default()
        };
        let roman = |mut n: u32| {
            const R: [(u32, &str); 13] = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
            let mut s = String::new();
            for (v, r) in R {
                while n >= v {
                    s.push_str(r);
                    n -= v;
                }
            }
            s
        };
        match self {
            StepType::Numbers => n.to_string(),
            StepType::Upper => letters(n, b'A'),
            StepType::Lower => letters(n, b'a'),
            StepType::RomanUpper => roman(n.clamp(1, 3999)),
            StepType::RomanLower => roman(n.clamp(1, 3999)).to_lowercase(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Border / stroke / text colour (the body colour for steps and balloons).
    pub color: Rgba,
    /// Border size.
    pub width: f32,
    pub font_size: f32,
    /// Rectangles and ellipses are filled with `fill_color`.
    pub fill: bool,
    pub fill_color: Rgba,
    /// Rectangle corner radius.
    pub corner_radius: f32,
    pub dash: Dash,
    /// Soft drop shadow under the item.
    pub shadow: bool,
    pub heads: ArrowHeads,
    pub step_type: StepType,
    /// Blur radius, pixelate block size or spotlight dim (%). 0 derives it from `width`.
    pub strength: f32,
    /// Text outline colour (`None` = no outline) and stroke width, in document pixels.
    pub outline: Option<Rgba>,
    pub outline_width: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: Rgba::RED,
            width: 4.0,
            font_size: 22.0,
            fill: false,
            fill_color: Rgba::RED,
            corner_radius: 0.0,
            dash: Dash::Solid,
            shadow: false,
            heads: ArrowHeads::End,
            step_type: StepType::Numbers,
            strength: 0.0,
            outline: Some(Rgba::WHITE),
            outline_width: 3.0,
        }
    }
}

/// Shadow offset and spread, in document pixels.
const SHADOW_DX: f32 = 1.0;
const SHADOW_DY: f32 = 2.5;
const SHADOW_BLUR: u32 = 3;
const SHADOW_ALPHA: u32 = 120;
/// Spotlights dim the whole canvas, so their bounds are "everything".
const EVERYWHERE: f32 = 1.0e7;
pub const SPOTLIGHT_DIM: u8 = 55;

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Bx),
    Ellipse(Bx),
    Line(PointF, PointF),
    Arrow(PointF, PointF),
    Pen(Vec<PointF>),
    /// A freehand stroke with arrow heads (`Style::heads`) that follow the path at its ends.
    FreeArrow(Vec<PointF>),
    Highlight(Bx),
    Blur(Bx),
    Pixelate(Bx),
    /// Smart eraser: fills with the colour sampled where the drag started.
    Eraser(Bx, Rgba),
    Step { at: PointF, n: u32 },
    Text { at: PointF, text: String, background: bool },
    Balloon { r: Bx, tail: PointF, text: String },
    Magnify { r: Bx, zoom: f32 },
    Image { r: Bx, img: Arc<Pixmap> },
    Sticker { r: Bx, kind: Sticker },
    /// Dims everything outside its box (all spotlights share one dimmed layer).
    Spotlight { r: Bx, dim: u8, ellipse: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub shape: Shape,
    pub style: Style,
}

/// The drawing tools. `Pointer` selects and moves items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Pointer,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Highlight,
    Blur,
    Pixelate,
    Eraser,
    Step,
    Text,
    Balloon,
    Magnify,
    Spotlight,
    Sticker(Sticker),
}

impl Tool {
    /// Tools that create an item from a single click rather than a drag.
    pub fn is_click(self) -> bool {
        matches!(self, Tool::Step | Tool::Text)
    }
    pub fn takes_text(self) -> bool {
        matches!(self, Tool::Text | Tool::Balloon)
    }
}

fn contrast(c: Rgba) -> Rgba {
    if c.is_light() {
        Rgba::BLACK
    } else {
        Rgba::WHITE
    }
}

pub fn next_step(items: &[Item]) -> u32 {
    items
        .iter()
        .filter_map(|i| if let Shape::Step { n, .. } = i.shape { Some(n) } else { None })
        .max()
        .unwrap_or(0)
        + 1
}

impl Item {
    /// Starts a new item for `tool` at `p`. `sample` is the base-image colour at `p`
    /// (used by the smart eraser). Returns `None` for the pointer tool.
    pub fn begin(tool: Tool, p: PointF, style: Style, items: &[Item], sample: Rgba) -> Option<Item> {
        let b = Bx::new(p, p);
        let shape = match tool {
            Tool::Pointer => return None,
            Tool::Rect => Shape::Rect(b),
            Tool::Ellipse => Shape::Ellipse(b),
            Tool::Line => Shape::Line(p, p),
            Tool::Arrow => Shape::Arrow(p, p),
            Tool::Pen => Shape::Pen(vec![p]),
            Tool::Highlight => Shape::Highlight(b),
            Tool::Blur => Shape::Blur(b),
            Tool::Pixelate => Shape::Pixelate(b),
            Tool::Eraser => Shape::Eraser(b, sample),
            Tool::Step => Shape::Step { at: p, n: next_step(items) },
            Tool::Text => Shape::Text { at: p, text: String::new(), background: style.fill },
            Tool::Balloon => Shape::Balloon { r: b, tail: p, text: String::new() },
            Tool::Magnify => Shape::Magnify { r: b, zoom: 2.0 },
            Tool::Spotlight => {
                let dim = if style.strength > 0.0 { (style.strength * 2.55).clamp(0.0, 255.0) as u8 } else { (SPOTLIGHT_DIM as f32 * 2.55) as u8 };
                Shape::Spotlight { r: b, dim, ellipse: false }
            }
            Tool::Sticker(kind) => Shape::Sticker { r: b, kind },
        };
        Some(Item { shape, style })
    }

    /// The tool that makes this kind of item (for showing its options).
    pub fn tool(&self) -> Option<Tool> {
        Some(match &self.shape {
            Shape::Rect(_) => Tool::Rect,
            Shape::Ellipse(_) => Tool::Ellipse,
            Shape::Line(..) => Tool::Line,
            Shape::Arrow(..) | Shape::FreeArrow(_) => Tool::Arrow,
            Shape::Pen(_) => Tool::Pen,
            Shape::Highlight(_) => Tool::Highlight,
            Shape::Blur(_) => Tool::Blur,
            Shape::Pixelate(_) => Tool::Pixelate,
            Shape::Eraser(..) => Tool::Eraser,
            Shape::Step { .. } => Tool::Step,
            Shape::Text { .. } => Tool::Text,
            Shape::Balloon { .. } => Tool::Balloon,
            Shape::Magnify { .. } => Tool::Magnify,
            Shape::Spotlight { .. } => Tool::Spotlight,
            Shape::Sticker { kind, .. } => Tool::Sticker(*kind),
            Shape::Image { .. } => return None,
        })
    }

    /// The box to outline when the item is selected (a spotlight's bounds are the whole canvas).
    pub fn outline(&self) -> Bx {
        match &self.shape {
            Shape::Spotlight { r, .. } => r.norm(),
            _ => self.bounds(),
        }
    }

    /// Updates the item while the mouse drags from its start to `p`.
    /// `constrain` (Shift) makes boxes square and snaps lines to 45°.
    pub fn drag(&mut self, start: PointF, p: PointF, constrain: bool) {
        let boxed = |p: PointF| {
            if constrain {
                let d = (p.x - start.x).abs().max((p.y - start.y).abs());
                PointF::new(start.x + d * (p.x - start.x).signum(), start.y + d * (p.y - start.y).signum())
            } else {
                p
            }
        };
        let snap = |p: PointF| {
            if !constrain {
                return p;
            }
            let (dx, dy) = (p.x - start.x, p.y - start.y);
            let len = (dx * dx + dy * dy).sqrt();
            let ang = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
            PointF::new(start.x + len * ang.cos(), start.y + len * ang.sin())
        };
        match &mut self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => {
                *b = Bx::new(start, boxed(p))
            }
            Shape::Magnify { r, .. } | Shape::Sticker { r, .. } | Shape::Spotlight { r, .. } => *r = Bx::new(start, boxed(p)),
            Shape::Balloon { r, tail, .. } => {
                *r = Bx::new(start, p);
                let n = r.norm();
                *tail = PointF::new(n.x0 + n.w() * 0.25, n.y1 + (n.h() * 0.5).clamp(16.0, 48.0));
            }
            Shape::Line(_, b) | Shape::Arrow(_, b) => *b = snap(p),
            Shape::Pen(pts) | Shape::FreeArrow(pts) => {
                if pts.last().is_none_or(|l| l.dist(p) >= 1.5) {
                    pts.push(p);
                }
            }
            Shape::Step { at, .. } | Shape::Text { at, .. } => *at = p,
            Shape::Image { .. } => {}
        }
    }

    /// True if the item is too small to keep after the drag ends.
    pub fn is_degenerate(&self) -> bool {
        match &self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => b.w() < 3.0 || b.h() < 3.0,
            Shape::Magnify { r, .. } | Shape::Sticker { r, .. } | Shape::Image { r, .. } | Shape::Spotlight { r, .. } => r.w() < 6.0 || r.h() < 6.0,
            Shape::Line(a, b) | Shape::Arrow(a, b) => a.dist(*b) < 4.0,
            Shape::Pen(p) => p.len() < 2,
            Shape::FreeArrow(p) => p.len() < 2 || path_len(p) < 4.0,
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Balloon { .. } | Shape::Step { .. } => false,
        }
    }

    /// The resizable box of box-shaped items (images, stickers, rectangles…).
    pub fn boxed_mut(&mut self) -> Option<&mut Bx> {
        match &mut self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => Some(b),
            Shape::Magnify { r, .. } | Shape::Image { r, .. } | Shape::Sticker { r, .. } | Shape::Spotlight { r, .. } => Some(r),
            _ => None,
        }
    }

    pub fn boxed(&self) -> Option<Bx> {
        self.clone().boxed_mut().map(|b| b.norm())
    }

    pub fn text_mut(&mut self) -> Option<&mut String> {
        match &mut self.shape {
            Shape::Text { text, .. } | Shape::Balloon { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.shape {
            Shape::Text { text, .. } | Shape::Balloon { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Top-left of the first line of an editable item's text, as it is drawn.
    pub fn text_origin(&self) -> Option<PointF> {
        match &self.shape {
            Shape::Text { at, .. } => Some(*at),
            Shape::Balloon { r, .. } => {
                let n = r.norm();
                let pad = self.style.font_size * 0.5;
                Some(PointF::new(n.x0 + pad, n.y0 + pad))
            }
            _ => None,
        }
    }

    fn text_line_height(&self) -> f32 {
        text::layout(" ", self.style.font_size, false).line_height
    }

    /// Byte index of the caret position nearest to `p` in an editable item's text.
    pub fn caret_index_at(&self, p: PointF) -> Option<usize> {
        let s = self.text()?;
        let o = self.text_origin()?;
        let lh = self.text_line_height();
        let lines: Vec<&str> = s.split('\n').collect();
        let li = (((p.y - o.y) / lh).floor().max(0.0) as usize).min(lines.len() - 1);
        let start: usize = lines[..li].iter().map(|l| l.len() + 1).sum();
        let line = lines[li];
        let pos = text::caret_positions(line, self.style.font_size, false);
        let x = p.x - o.x;
        let ci = (0..pos.len()).min_by(|&a, &b| (pos[a] - x).abs().total_cmp(&(pos[b] - x).abs())).unwrap_or(0);
        Some(start + line.char_indices().nth(ci).map_or(line.len(), |(b, _)| b))
    }

    /// Where to draw the caret at byte index `caret`: (x, top y, height) in document space.
    pub fn caret_point(&self, caret: usize) -> Option<(f32, f32, f32)> {
        let s = self.text()?;
        let o = self.text_origin()?;
        let lh = self.text_line_height();
        let c = floor_boundary(s, caret);
        let before = &s[..c];
        let li = before.matches('\n').count();
        let ls = before.rfind('\n').map_or(0, |i| i + 1);
        let x = text::caret_positions(&s[ls..c], self.style.font_size, false).last().copied().unwrap_or(0.0);
        Some((o.x + x, o.y + li as f32 * lh, lh))
    }

    /// Inserts `s` at byte index `caret` and moves the caret past it.
    pub fn insert_at_caret(&mut self, caret: &mut usize, s: &str) -> bool {
        let Some(t) = self.text_mut() else { return false };
        let c = floor_boundary(t, *caret);
        t.insert_str(c, s);
        *caret = c + s.len();
        true
    }

    /// Deletes the char before (Backspace) or after (Delete) the caret.
    pub fn delete_at_caret(&mut self, caret: &mut usize, forward: bool) -> bool {
        let Some(t) = self.text_mut() else { return false };
        let c = floor_boundary(t, *caret);
        let range = if forward {
            let Some(ch) = t[c..].chars().next() else { return false };
            c..c + ch.len_utf8()
        } else {
            let Some(ch) = t[..c].chars().next_back() else { return false };
            c - ch.len_utf8()..c
        };
        *caret = range.start;
        t.replace_range(range, "");
        true
    }

    /// Removes the text between byte indices `a` and `b` (either order). Returns the start.
    pub fn delete_range(&mut self, a: usize, b: usize) -> usize {
        let Some(t) = self.text_mut() else { return a.min(b) };
        let (lo, hi) = (floor_boundary(t, a.min(b)), floor_boundary(t, a.max(b)));
        t.replace_range(lo..hi, "");
        lo
    }

    /// Highlight boxes (document space) covering the text between `a` and `b`, one per line.
    pub fn selection_rects(&self, a: usize, b: usize) -> Vec<Bx> {
        let (Some(t), Some(o)) = (self.text(), self.text_origin()) else { return Vec::new() };
        let (lo, hi) = (floor_boundary(t, a.min(b)), floor_boundary(t, a.max(b)));
        if lo == hi {
            return Vec::new();
        }
        let size = self.style.font_size;
        let lh = self.text_line_height();
        let mut out = Vec::new();
        let mut ls = 0;
        for (li, line) in t.split('\n').enumerate() {
            let le = ls + line.len();
            let (s, e) = (lo.max(ls), hi.min(le));
            let covers_break = lo <= le && hi > le;
            if s < e || covers_break {
                let pos = text::caret_positions(line, size, false);
                let x_of = |i: usize| pos[line[..i - ls].chars().count()];
                // A selected line break shows as a little extra width, as in Windows.
                let x1 = x_of(e) + if covers_break { size * 0.3 } else { 0.0 };
                let y = o.y + li as f32 * lh;
                out.push(Bx { x0: o.x + x_of(s), y0: y, x1: o.x + x1, y1: y + lh });
            }
            ls = le + 1;
        }
        out
    }

    /// The word (or run of spaces / punctuation) around byte index `i`, for double-click.
    pub fn word_at(&self, i: usize) -> (usize, usize) {
        let Some(t) = self.text() else { return (i, i) };
        let i = floor_boundary(t, i);
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c == '\n' { 2 } else if c.is_whitespace() { 1 } else { 3 };
        let Some(k) = t[i..].chars().next().or_else(|| t[..i].chars().next_back()).map(class) else { return (i, i) };
        if k == 2 {
            return (i, i);
        }
        let start = t[..i].char_indices().rev().take_while(|&(_, c)| class(c) == k).last().map_or(i, |(b, _)| b);
        let end = t[i..].char_indices().find(|&(_, c)| class(c) != k).map_or(t.len(), |(b, _)| i + b);
        (start, end)
    }

    /// The caret index after an arrow / Home / End key.
    pub fn move_caret(&self, caret: usize, m: CaretMove) -> usize {
        let Some(t) = self.text() else { return caret };
        let c = floor_boundary(t, caret);
        let ls = t[..c].rfind('\n').map_or(0, |i| i + 1);
        let le = t[c..].find('\n').map_or(t.len(), |i| c + i);
        match m {
            CaretMove::Left => t[..c].chars().next_back().map_or(c, |ch| c - ch.len_utf8()),
            CaretMove::Right => t[c..].chars().next().map_or(c, |ch| c + ch.len_utf8()),
            CaretMove::Home => ls,
            CaretMove::End => le,
            CaretMove::Up | CaretMove::Down => {
                let Some((x, y, h)) = self.caret_point(c) else { return c };
                let y = if m == CaretMove::Up { y - h / 2.0 } else { y + h * 1.5 };
                if (m == CaretMove::Up && ls == 0) || (m == CaretMove::Down && le == t.len()) {
                    return c;
                }
                self.caret_index_at(PointF::new(x, y)).unwrap_or(c)
            }
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut PointF| *p = p.offset(dx, dy);
        match &mut self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => *b = b.offset(dx, dy),
            Shape::Magnify { r, .. } | Shape::Image { r, .. } | Shape::Sticker { r, .. } | Shape::Spotlight { r, .. } => *r = r.offset(dx, dy),
            Shape::Balloon { r, tail, .. } => {
                *r = r.offset(dx, dy);
                mv(tail);
            }
            Shape::Line(a, b) | Shape::Arrow(a, b) => {
                mv(a);
                mv(b);
            }
            Shape::Pen(pts) | Shape::FreeArrow(pts) => pts.iter_mut().for_each(mv),
            Shape::Step { at, .. } | Shape::Text { at, .. } => mv(at),
        }
    }

    fn step_radius(&self) -> f32 {
        (self.style.font_size * 0.75).max(10.0)
    }

    fn balloon_box(r: Bx, text: &str, size: f32) -> Bx {
        let n = r.norm();
        let pad = size * 0.5;
        let l = text::layout(if text.is_empty() { " " } else { text }, size, false);
        Bx { x0: n.x0, y0: n.y0, x1: n.x0 + n.w().max(l.width + pad * 2.0), y1: n.y0 + n.h().max(l.height + pad * 2.0) }
    }

    /// Whether this kind of item can cast a drop shadow.
    fn casts_shadow(&self) -> bool {
        matches!(
            self.shape,
            Shape::Rect(_) | Shape::Ellipse(_) | Shape::Line(..) | Shape::Arrow(..) | Shape::Pen(_) | Shape::FreeArrow(_) | Shape::Step { .. } | Shape::Text { .. } | Shape::Balloon { .. } | Shape::Magnify { .. }
        )
    }

    /// Bounding box in document space, including stroke width, decorations and shadow.
    pub fn bounds(&self) -> Bx {
        let b = self.shape_bounds();
        if self.style.shadow && self.casts_shadow() {
            let d = SHADOW_BLUR as f32 + 1.0;
            Bx { x0: b.x0 - d, y0: b.y0 - d, x1: b.x1 + SHADOW_DX + d, y1: b.y1 + SHADOW_DY + d }
        } else {
            b
        }
    }

    fn shape_bounds(&self) -> Bx {
        let w = self.style.width;
        let grow = |b: Bx, d: f32| {
            let n = b.norm();
            Bx { x0: n.x0 - d, y0: n.y0 - d, x1: n.x1 + d, y1: n.y1 + d }
        };
        match &self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) => grow(*b, w),
            Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => grow(*b, 1.0),
            Shape::Magnify { r, .. } => grow(*r, w.clamp(2.0, 6.0)),
            Shape::Image { r, .. } | Shape::Sticker { r, .. } => grow(*r, 2.0),
            Shape::Spotlight { .. } => Bx { x0: -EVERYWHERE, y0: -EVERYWHERE, x1: EVERYWHERE, y1: EVERYWHERE },
            Shape::Line(a, b) => grow(Bx::new(*a, *b), w),
            Shape::Arrow(a, b) => grow(Bx::new(*a, *b), w * 4.0 + 12.0),
            Shape::Pen(pts) | Shape::FreeArrow(pts) => {
                let mut b = Bx::new(pts[0], pts[0]).norm();
                for p in pts {
                    b = Bx { x0: b.x0.min(p.x), y0: b.y0.min(p.y), x1: b.x1.max(p.x), y1: b.y1.max(p.y) };
                }
                // Arrow heads stick out sideways from the ends.
                grow(b, if matches!(self.shape, Shape::FreeArrow(_)) { w * 4.0 + 12.0 } else { w })
            }
            Shape::Step { at, .. } => {
                let r = self.step_radius() + 2.0;
                Bx { x0: at.x - r, y0: at.y - r, x1: at.x + r, y1: at.y + r }
            }
            Shape::Text { at, text, .. } => {
                let l = text::layout(if text.is_empty() { " " } else { text }, self.style.font_size, false);
                let mut pad = self.style.font_size * 0.35;
                if self.style.outline.is_some() {
                    // Half the outline stroke lies outside the glyphs.
                    pad = pad.max(self.style.outline_width / 2.0 + 2.0);
                }
                Bx { x0: at.x - pad, y0: at.y - pad, x1: at.x + l.width.max(2.0) + pad, y1: at.y + l.height + pad }
            }
            Shape::Balloon { r, tail, text } => {
                let b = Self::balloon_box(*r, text, self.style.font_size);
                let t = Bx::new(*tail, *tail);
                grow(Bx { x0: b.x0.min(t.x0), y0: b.y0.min(t.y0), x1: b.x1.max(t.x1), y1: b.y1.max(t.y1) }, 2.0)
            }
        }
    }

    /// Hit test with a few pixels of tolerance.
    pub fn hit(&self, p: PointF) -> bool {
        let tol = self.style.width / 2.0 + 5.0;
        match &self.shape {
            Shape::Line(a, b) | Shape::Arrow(a, b) => seg_dist(p, *a, *b) <= tol,
            Shape::Pen(pts) | Shape::FreeArrow(pts) => pts.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= tol) || pts[0].dist(p) <= tol,
            Shape::Rect(b) if !self.style.fill => {
                // Outline rectangles are hit only near their edges.
                let n = b.norm();
                let near_x = (p.x - n.x0).abs() <= tol || (p.x - n.x1).abs() <= tol;
                let near_y = (p.y - n.y0).abs() <= tol || (p.y - n.y1).abs() <= tol;
                let in_x = p.x >= n.x0 - tol && p.x <= n.x1 + tol;
                let in_y = p.y >= n.y0 - tol && p.y <= n.y1 + tol;
                (near_x && in_y) || (near_y && in_x)
            }
            Shape::Spotlight { r, .. } => r.norm().contains(p),
            _ => self.shape_bounds().contains(p),
        }
    }
}

fn seg_dist(p: PointF, a: PointF, b: PointF) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(PointF::new(a.x + t * dx, a.y + t * dy))
}

/// Index of the topmost item under `p`.
pub fn hit_test(items: &[Item], p: PointF) -> Option<usize> {
    items.iter().rposition(|i| i.hit(p))
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

/// A stroke in the item's border size and dash pattern.
fn styled_stroke(st: &Style, join: LineJoin) -> Stroke {
    let dash = st.dash.pattern(st.width).and_then(|p| StrokeDash::new(p, 0.0));
    // Butt caps keep mitred rectangle corners crisp; dots need round caps.
    let cap = if join == LineJoin::Miter && st.dash == Dash::Solid { LineCap::Butt } else { LineCap::Round };
    Stroke { width: st.width, line_cap: cap, line_join: join, dash, ..Default::default() }
}

fn rect_path(b: Bx, radius: f32) -> Option<Path> {
    let n = b.norm();
    let r = radius.min(n.w() / 2.0).min(n.h() / 2.0);
    if r > 0.5 {
        effects::rounded_rect(n.x0, n.y0, n.w(), n.h(), r)
    } else {
        n.skia().map(PathBuilder::from_rect)
    }
}

/// Draws all items. Item coordinates plus `(ox, oy)` give pixmap coordinates.
///
/// All spotlights are drawn together, as one dimmed layer with holes, at the
/// position of the first one: items before it are dimmed, items after it are not.
pub fn render(pm: &mut PixmapMut, items: &[Item], ox: f32, oy: f32) {
    let mut spotlit = false;
    for it in items {
        if matches!(it.shape, Shape::Spotlight { .. }) {
            if !spotlit {
                draw_spotlights(pm, items, ox, oy);
                spotlit = true;
            }
            continue;
        }
        render_item(pm, it, ox, oy);
    }
}

fn draw_spotlights(pm: &mut PixmapMut, items: &[Item], ox: f32, oy: f32) {
    let spots: Vec<(Bx, u8, bool)> = items.iter().filter_map(|i| if let Shape::Spotlight { r, dim, ellipse } = i.shape { Some((r, dim, ellipse)) } else { None }).collect();
    let Some(&(_, dim, _)) = spots.first() else { return };
    let (w, h) = (pm.width(), pm.height());
    let Some(mut mask) = Mask::new(w, h) else { return };
    let t = Transform::from_translate(ox, oy);
    for (r, _, ellipse) in &spots {
        let path = if *ellipse { r.norm().skia().and_then(PathBuilder::from_oval) } else { r.norm().skia().map(PathBuilder::from_rect) };
        if let Some(p) = path {
            mask.fill_path(&p, FillRule::Winding, true, t);
        }
    }
    mask.invert();
    if let Some(all) = snapit_gfx::tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
        pm.fill_rect(all, &Rgba::new(0, 0, 0, dim).paint(), Transform::identity(), Some(&mask));
    }
}

/// Draws the item's silhouette, softened and offset, as its drop shadow.
fn draw_shadow(pm: &mut PixmapMut, it: &Item, ox: f32, oy: f32) {
    let mut ghost = it.clone();
    ghost.style.shadow = false;
    // A lens shows the pixels under it; its shadow is the whole disc.
    if let Shape::Magnify { r, .. } = ghost.shape {
        ghost.shape = Shape::Ellipse(r);
        ghost.style.fill = true;
    }
    let b = it.shape_bounds().offset(ox, oy).to_rect().inflate(SHADOW_BLUR as i32 + 2);
    let Some(r) = b.intersect(Rect::new(-(SHADOW_DX.ceil() as i32), -(SHADOW_DY.ceil() as i32), pm.width() as i32 + 4, pm.height() as i32 + 4)) else { return };
    let Some(mut tmp) = Pixmap::new(r.w.max(1) as u32, r.h.max(1) as u32) else { return };
    render_item(&mut tmp.as_mut(), &ghost, ox - r.x as f32, oy - r.y as f32);
    for px in tmp.data_mut().as_chunks_mut::<4>().0 {
        // Premultiplied black at a fraction of the item's coverage.
        let a = px[3] as u32 * SHADOW_ALPHA / 255;
        *px = [0, 0, 0, a as u8];
    }
    let full = Rect::new(0, 0, r.w, r.h);
    effects::blur(&mut tmp.as_mut(), full, SHADOW_BLUR);
    let paint = PixmapPaint::default();
    pm.draw_pixmap(0, 0, tmp.as_ref(), &paint, Transform::from_translate(r.x as f32 + SHADOW_DX, r.y as f32 + SHADOW_DY), None);
}

pub fn render_item(pm: &mut PixmapMut, it: &Item, ox: f32, oy: f32) {
    if it.style.shadow && it.casts_shadow() {
        draw_shadow(pm, it, ox, oy);
    }
    let t = Transform::from_translate(ox, oy);
    let st = &it.style;
    let paint = st.color.paint();
    match &it.shape {
        Shape::Rect(b) => {
            if let Some(path) = rect_path(*b, st.corner_radius) {
                if st.fill {
                    pm.fill_path(&path, &st.fill_color.paint(), FillRule::Winding, t, None);
                }
                if st.width > 0.0 && !(st.fill && st.fill_color == st.color && st.dash == Dash::Solid && st.corner_radius <= 0.5) {
                    let join = if st.corner_radius > 0.5 { LineJoin::Round } else { LineJoin::Miter };
                    pm.stroke_path(&path, &paint, &styled_stroke(st, join), t, None);
                }
            }
        }
        Shape::Ellipse(b) => {
            if let Some(path) = b.skia().and_then(PathBuilder::from_oval) {
                if st.fill {
                    pm.fill_path(&path, &st.fill_color.paint(), FillRule::Winding, t, None);
                }
                if st.width > 0.0 && !(st.fill && st.fill_color == st.color && st.dash == Dash::Solid) {
                    pm.stroke_path(&path, &paint, &styled_stroke(st, LineJoin::Round), t, None);
                }
            }
        }
        Shape::Line(a, b) => {
            let mut pb = PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint, &styled_stroke(st, LineJoin::Round), t, None);
            }
        }
        Shape::Arrow(a, b) => draw_arrow(pm, *a, *b, st, t),
        Shape::Pen(pts) => {
            if let Some(p) = smooth_path(pts) {
                pm.stroke_path(&p, &paint, &styled_stroke(st, LineJoin::Round), t, None);
            }
        }
        Shape::FreeArrow(pts) => draw_free_arrow(pm, pts, st, t),
        Shape::Highlight(b) => {
            if let Some(r) = b.skia() {
                let mut p = st.color.paint();
                p.blend_mode = BlendMode::Multiply;
                pm.fill_rect(r, &p, t, None);
            }
        }
        Shape::Blur(b) => {
            let radius = if st.strength > 0.0 { st.strength } else { (st.width * 2.5).max(4.0) };
            effects::blur(pm, b.offset(ox, oy).to_rect(), radius.max(1.0) as u32)
        }
        Shape::Pixelate(b) => {
            let block = if st.strength > 0.0 { st.strength } else { (st.width * 3.0).max(6.0) };
            effects::pixelate(pm, b.offset(ox, oy).to_rect(), block.max(2.0) as u32)
        }
        Shape::Spotlight { .. } => draw_spotlights(pm, std::slice::from_ref(it), ox, oy),
        Shape::Eraser(b, c) => {
            if let Some(r) = b.skia() {
                let mut p = c.paint();
                p.anti_alias = false;
                pm.fill_rect(r, &p, t, None);
            }
        }
        Shape::Step { at, n } => {
            let r = it.step_radius();
            if let Some(c) = PathBuilder::from_circle(at.x, at.y, r) {
                pm.fill_path(&c, &paint, FillRule::Winding, t, None);
                pm.stroke_path(&c, &contrast(st.color).with_alpha(200).paint(), &stroke(2.0), t, None);
            }
            let label = st.step_type.label(*n);
            // Longer labels (XVIII, AA) shrink to stay inside the disc.
            let mut size = r * 1.15;
            let mut l = text::layout(&label, size, true);
            if l.width > r * 1.6 {
                size *= r * 1.6 / l.width;
                l = text::layout(&label, size, true);
            }
            text::draw(pm, &label, ox + at.x - l.width / 2.0, oy + at.y - l.height / 2.0, size, true, contrast(st.color), None);
        }
        Shape::Text { at, text: s, background } => {
            if s.is_empty() {
                return;
            }
            let size = st.font_size;
            let outline = st.outline.map(|c| (c, st.outline_width.max(0.5)));
            if *background {
                let l = text::layout(s, size, false);
                let pad = size * 0.3;
                if let Some(bg) = effects::rounded_rect(at.x - pad, at.y - pad, l.width + pad * 2.0, l.height + pad * 2.0, pad) {
                    pm.fill_path(&bg, &paint, FillRule::Winding, t, None);
                }
                text::draw(pm, s, ox + at.x, oy + at.y, size, false, contrast(st.color), outline);
            } else {
                text::draw(pm, s, ox + at.x, oy + at.y, size, false, st.color, outline);
            }
        }
        Shape::Balloon { r, tail, text: s } => {
            let size = st.font_size;
            let b = Item::balloon_box(*r, s, size);
            let pad = size * 0.5;
            if let Some(body) = effects::rounded_rect(b.x0, b.y0, b.w(), b.h(), pad) {
                pm.fill_path(&body, &paint, FillRule::Winding, t, None);
            }
            // Tail: a triangle from the nearest horizontal edge to the tail point.
            let below = tail.y > (b.y0 + b.y1) / 2.0;
            let ey = if below { b.y1 - 1.0 } else { b.y0 + 1.0 };
            let cx = tail.x.clamp(b.x0 + pad * 1.5, (b.x1 - pad * 1.5).max(b.x0 + pad * 1.5));
            let hw = (b.w() * 0.12).clamp(6.0, 18.0);
            let mut pb = PathBuilder::new();
            pb.move_to(cx - hw, ey);
            pb.line_to(tail.x, tail.y);
            pb.line_to(cx + hw, ey);
            pb.close();
            if let Some(p) = pb.finish() {
                pm.fill_path(&p, &paint, FillRule::Winding, t, None);
            }
            if !s.is_empty() {
                text::draw(pm, s, ox + b.x0 + pad, oy + b.y0 + pad, size, false, contrast(st.color), None);
            }
        }
        Shape::Magnify { r, zoom } => draw_magnify(pm, *r, *zoom, st, ox, oy),
        Shape::Image { r, img } => draw_image(pm, img, *r, t),
        Shape::Sticker { r, kind } => stickers::draw(pm, *kind, r.offset(ox, oy)),
    }
}

/// A pen-style path through `pts`, smoothed with quadratic curves between midpoints.
fn smooth_path(pts: &[PointF]) -> Option<snapit_gfx::tiny_skia::Path> {
    let first = pts.first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    if pts.len() == 1 {
        pb.line_to(first.x + 0.1, first.y);
    }
    for w in pts.windows(2).skip(1) {
        let mid = PointF::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
        pb.quad_to(w[0].x, w[0].y, mid.x, mid.y);
    }
    if let Some(l) = pts.last() {
        pb.line_to(l.x, l.y);
    }
    pb.finish()
}

/// Total length of a polyline.
fn path_len(pts: &[PointF]) -> f32 {
    pts.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// The point `d` along a polyline from its start (clamped to its ends).
fn point_along(pts: &[PointF], d: f32) -> PointF {
    let mut left = d.max(0.0);
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        if l > 0.0 && l >= left {
            let k = left / l;
            return PointF::new(w[0].x + (w[1].x - w[0].x) * k, w[0].y + (w[1].y - w[0].y) * k);
        }
        left -= l;
    }
    pts.last().copied().unwrap_or(PointF::new(0.0, 0.0))
}

/// The part of a polyline between distances `a` and `b` from its start.
fn sub_path(pts: &[PointF], a: f32, b: f32) -> Vec<PointF> {
    let mut out = vec![point_along(pts, a)];
    let mut acc = 0.0;
    for w in pts.windows(2) {
        acc += w[0].dist(w[1]);
        if acc > a && acc < b {
            out.push(w[1]);
        }
    }
    out.push(point_along(pts, b));
    out
}

/// Which ends get a head: (start, end).
fn head_ends(h: ArrowHeads) -> (bool, bool) {
    match h {
        ArrowHeads::End => (false, true),
        ArrowHeads::Start => (true, false),
        ArrowHeads::Both => (true, true),
    }
}

/// Head length for an arrow of `len` with `st`'s width and heads.
fn head_size(st: &Style, len: f32) -> f32 {
    let (a, b) = head_ends(st.heads);
    let heads = if a && b { 2.0 } else { 1.0 };
    (st.width * 3.5 + 8.0).min(len * 0.6 / heads)
}

/// A filled arrow head with its tip at `tip`, pointing along the unit vector (ux, uy).
fn draw_head(pm: &mut PixmapMut, tip: PointF, ux: f32, uy: f32, head: f32, paint: &Paint, t: Transform) {
    let half = head * 0.6;
    let base = PointF::new(tip.x - ux * head, tip.y - uy * head);
    let mut pb = PathBuilder::new();
    pb.move_to(tip.x, tip.y);
    pb.line_to(base.x - uy * half, base.y + ux * half);
    pb.line_to(base.x + uy * half, base.y - ux * half);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, paint, FillRule::Winding, t, None);
        let s = Stroke { width: 1.5, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, paint, &s, t, None);
    }
}

fn draw_arrow(pm: &mut PixmapMut, a: PointF, b: PointF, st: &Style, t: Transform) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (at_start, at_end) = head_ends(st.heads);
    let head = head_size(st, len);
    let paint = st.color.paint();
    // The shaft stops inside each head so its round cap does not poke out of the tip.
    let s0 = if at_start { PointF::new(a.x + ux * head * 0.7, a.y + uy * head * 0.7) } else { a };
    let s1 = if at_end { PointF::new(b.x - ux * head * 0.7, b.y - uy * head * 0.7) } else { b };
    let mut pb = PathBuilder::new();
    pb.move_to(s0.x, s0.y);
    pb.line_to(s1.x, s1.y);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint, &styled_stroke(st, LineJoin::Round), t, None);
    }
    if at_end {
        draw_head(pm, b, ux, uy, head, &paint, t);
    }
    if at_start {
        draw_head(pm, a, -ux, -uy, head, &paint, t);
    }
}

/// A freehand arrow: the smoothed stroke, with heads aimed along the path's last
/// (or first) stretch of about one head length, so a wobble at the very end does
/// not swing the head around.
fn draw_free_arrow(pm: &mut PixmapMut, pts: &[PointF], st: &Style, t: Transform) {
    let len = path_len(pts);
    if pts.len() < 2 || len < 1.0 {
        return;
    }
    let (at_start, at_end) = head_ends(st.heads);
    let head = head_size(st, len);
    let paint = st.color.paint();
    let trim = head * 0.7;
    let shaft = sub_path(pts, if at_start { trim } else { 0.0 }, if at_end { len - trim } else { len });
    if let Some(p) = smooth_path(&shaft) {
        pm.stroke_path(&p, &paint, &styled_stroke(st, LineJoin::Round), t, None);
    }
    let aim = |tip: PointF, from: PointF| {
        let (dx, dy) = (tip.x - from.x, tip.y - from.y);
        let l = (dx * dx + dy * dy).sqrt();
        (l > 0.01).then(|| (dx / l, dy / l))
    };
    if at_end {
        let tip = pts[pts.len() - 1];
        if let Some((ux, uy)) = aim(tip, point_along(pts, len - head)) {
            draw_head(pm, tip, ux, uy, head, &paint, t);
        }
    }
    if at_start {
        let tip = pts[0];
        if let Some((ux, uy)) = aim(tip, point_along(pts, head)) {
            draw_head(pm, tip, ux, uy, head, &paint, t);
        }
    }
}

fn draw_magnify(pm: &mut PixmapMut, r: Bx, zoom: f32, st: &Style, ox: f32, oy: f32) {
    let n = r.norm().offset(ox, oy);
    let (sw, sh) = (n.w() / zoom, n.h() / zoom);
    let (cx, cy) = ((n.x0 + n.x1) / 2.0, (n.y0 + n.y1) / 2.0);
    let src_r = Bx { x0: cx - sw / 2.0, y0: cy - sh / 2.0, x1: cx + sw / 2.0, y1: cy + sh / 2.0 }.to_rect();
    let (w, h) = (pm.width(), pm.height());
    let Some(src) = snapit_gfx::crop_bytes(pm.data_mut(), w, h, src_r) else { return };
    let Some(path) = n.skia().and_then(PathBuilder::from_oval) else { return };
    // Map source pixels so the source centre lands on the lens centre.
    let tx = cx - (cx - src_r.x as f32) * zoom;
    let ty = cy - (cy - src_r.y as f32) * zoom;
    let paint = Paint {
        shader: Pattern::new(src.as_ref(), SpreadMode::Pad, FilterQuality::Bicubic, 1.0, Transform::from_row(zoom, 0.0, 0.0, zoom, tx, ty)),
        anti_alias: true,
        ..Default::default()
    };
    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    let ring = Style { width: st.width.clamp(2.0, 6.0), ..*st };
    pm.stroke_path(&path, &st.color.paint(), &styled_stroke(&ring, LineJoin::Round), Transform::identity(), None);
}

pub fn draw_image(pm: &mut PixmapMut, img: &Pixmap, r: Bx, t: Transform) {
    let n = r.norm();
    let Some(rect) = n.skia() else { return };
    let (sx, sy) = (n.w() / img.width() as f32, n.h() / img.height() as f32);
    let paint = Paint {
        shader: Pattern::new(img.as_ref(), SpreadMode::Pad, FilterQuality::Bicubic, 1.0, Transform::from_row(sx, 0.0, 0.0, sy, n.x0, n.y0)),
        anti_alias: true,
        ..Default::default()
    };
    pm.fill_rect(rect, &paint, t, None);
}

/// Caret keys while typing into a text or balloon item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaretMove {
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
}

/// `i` clamped into `s` and moved back to a char boundary.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut c = i.min(s.len());
    while !s.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// Simple snapshot-based undo/redo.
#[derive(Clone, Debug, Default)]
pub struct History<T: Clone> {
    undo: Vec<T>,
    redo: Vec<T>,
}

impl<T: Clone> History<T> {
    pub fn new() -> Self {
        Self { undo: Vec::new(), redo: Vec::new() }
    }
    /// Record `before` (the state prior to a change).
    pub fn push(&mut self, before: T) {
        self.undo.push(before);
        self.redo.clear();
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }
    pub fn undo(&mut self, current: T) -> Option<T> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }
    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapit_gfx::{pixel_at, rgba};

    fn canvas() -> Pixmap {
        let mut pm = Pixmap::new(200, 120).unwrap();
        pm.fill(rgba(255, 255, 255, 255));
        pm
    }

    fn all_tools() -> Vec<Tool> {
        let mut v = vec![
            Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Pen, Tool::Highlight, Tool::Blur,
            Tool::Pixelate, Tool::Eraser, Tool::Step, Tool::Text, Tool::Balloon, Tool::Magnify,
        ];
        v.extend(Sticker::ALL.iter().map(|s| Tool::Sticker(*s)));
        v
    }

    #[test]
    fn caret_maps_clicks_to_text_positions_and_back() {
        if !text::available() {
            return; // no system fonts (non-Windows CI)
        }
        let mut it = Item::begin(Tool::Text, PointF::new(10.0, 20.0), Style::default(), &[], Rgba::WHITE).unwrap();
        it.text_mut().unwrap().push_str("Hello\nWörld");
        let s = it.text().unwrap().to_string();
        // Every char boundary round-trips: its caret point clicks back to it.
        for (b, _) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
            let (x, y, h) = it.caret_point(b).unwrap();
            assert_eq!(it.caret_index_at(PointF::new(x, y + h / 2.0)), Some(b), "byte {b}");
        }
        // Clicks left of, right of, above and below the text clamp to the nearest position.
        let (_, y2, h) = it.caret_point(7).unwrap();
        assert_eq!(it.caret_index_at(PointF::new(-50.0, 0.0)), Some(0));
        assert_eq!(it.caret_index_at(PointF::new(500.0, 25.0)), Some(5));
        assert_eq!(it.caret_index_at(PointF::new(500.0, y2 + h * 5.0)), Some(s.len()));
        assert!(Item::begin(Tool::Rect, PointF::new(0.0, 0.0), Style::default(), &[], Rgba::WHITE).unwrap().caret_point(0).is_none());

        // Typing, deleting and moving happen at the caret, never inside a multi-byte char.
        let mut c = 1;
        assert!(it.insert_at_caret(&mut c, "é"));
        assert_eq!((it.text().unwrap(), c), ("Héello\nWörld", 3));
        assert!(it.delete_at_caret(&mut c, false));
        assert_eq!((it.text().unwrap(), c), ("Hello\nWörld", 1));
        assert!(it.delete_at_caret(&mut c, true));
        assert_eq!((it.text().unwrap(), c), ("Hllo\nWörld", 1));
        assert!(!it.delete_at_caret(&mut 0, false));
        assert_eq!(it.move_caret(1, CaretMove::End), 4);
        assert_eq!(it.move_caret(4, CaretMove::Right), 5);
        assert_eq!(it.move_caret(6, CaretMove::Right), 8); // over "ö"
        assert_eq!(it.move_caret(8, CaretMove::Left), 6);
        assert_eq!(it.move_caret(7, CaretMove::Left), 5); // mid-char index snaps back first
        assert_eq!(it.move_caret(6, CaretMove::Home), 5);
        assert!(it.move_caret(2, CaretMove::Down) > 5);
        assert!(it.move_caret(8, CaretMove::Up) <= 4);
        assert_eq!(it.move_caret(2, CaretMove::Up), 2);
        assert_eq!(it.move_caret(8, CaretMove::Down), 8);

        // Selections: one highlight per touched line, words for double-click, range delete.
        assert!(it.selection_rects(3, 3).is_empty());
        assert_eq!(it.selection_rects(1, 3).len(), 1);
        let two = it.selection_rects(8, 2); // "lo\nWö" — either order
        assert_eq!(two.len(), 2);
        assert!(two[0].y1 <= two[1].y0 + 0.01 && two[0].x0 > two[1].x0);
        assert_eq!(it.word_at(2), (0, 4));
        assert_eq!(it.word_at(8), (5, 11));
        assert_eq!(it.word_at(4), (4, 4)); // the line break itself
        assert_eq!(it.delete_range(8, 2), 2);
        assert_eq!(it.text().unwrap(), "Hlrld");
    }

    #[test]
    fn every_tool_creates_renders_and_moves() {
        let style = Style::default();
        let mut items = Vec::new();
        for tool in all_tools() {
            let start = PointF::new(20.0, 20.0);
            let mut it = Item::begin(tool, start, style, &items, Rgba::WHITE).unwrap();
            it.drag(start, PointF::new(60.0, 50.0), false);
            it.drag(start, PointF::new(90.0, 70.0), true);
            if let Some(t) = it.text_mut() {
                t.push_str("Hi");
            }
            assert!(!it.is_degenerate(), "{tool:?}");
            let b = it.bounds();
            it.translate(5.0, 5.0);
            assert!((it.bounds().x0 - b.x0 - 5.0).abs() < 0.01, "{tool:?}");
            items.push(it);
        }
        let mut pm = canvas();
        render(&mut pm.as_mut(), &items, 0.0, 0.0);
        assert!(pm.data().as_chunks::<4>().0.iter().any(|p| p[0] != 255));
        assert!(Item::begin(Tool::Pointer, PointF::default(), style, &items, Rgba::WHITE).is_none());
    }

    #[test]
    fn rect_draws_in_style_colour_and_offset_applies() {
        let mut it = Item::begin(Tool::Rect, PointF::new(10.0, 10.0), Style { fill: true, ..Style::default() }, &[], Rgba::WHITE).unwrap();
        it.drag(PointF::new(10.0, 10.0), PointF::new(30.0, 30.0), false);
        let mut pm = canvas();
        render(&mut pm.as_mut(), &[it], 50.0, 0.0);
        assert_eq!(pixel_at(&pm, 70, 20), Some(Rgba::RED));
        assert_eq!(pixel_at(&pm, 20, 20), Some(Rgba::WHITE));
    }

    #[test]
    fn steps_number_themselves_and_hit_testing() {
        let s = Style::default();
        let a = Item::begin(Tool::Step, PointF::new(10.0, 10.0), s, &[], Rgba::WHITE).unwrap();
        let b = Item::begin(Tool::Step, PointF::new(50.0, 10.0), s, std::slice::from_ref(&a), Rgba::WHITE).unwrap();
        assert!(matches!(b.shape, Shape::Step { n: 2, .. }));
        let mut line = Item::begin(Tool::Line, PointF::new(0.0, 100.0), s, &[], Rgba::WHITE).unwrap();
        line.drag(PointF::new(0.0, 100.0), PointF::new(100.0, 100.0), false);
        let items = vec![a, b, line];
        assert_eq!(hit_test(&items, PointF::new(50.0, 10.0)), Some(1));
        assert_eq!(hit_test(&items, PointF::new(50.0, 102.0)), Some(2));
        assert_eq!(hit_test(&items, PointF::new(50.0, 60.0)), None);
    }

    #[test]
    fn smart_eraser_uses_sampled_colour_and_history_works() {
        let mut e = Item::begin(Tool::Eraser, PointF::new(0.0, 0.0), Style::default(), &[], Rgba::rgb(1, 2, 3)).unwrap();
        e.drag(PointF::new(0.0, 0.0), PointF::new(10.0, 10.0), false);
        let mut pm = canvas();
        render(&mut pm.as_mut(), &[e], 0.0, 0.0);
        assert_eq!(pixel_at(&pm, 5, 5), Some(Rgba::rgb(1, 2, 3)));

        let mut h = History::new();
        h.push(vec![1]);
        let cur = h.undo(vec![1, 2]).unwrap();
        assert_eq!(cur, vec![1]);
        assert_eq!(h.redo(cur).unwrap(), vec![1, 2]);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn spotlights_share_one_dim_layer() {
        let s = Style::default();
        let mut a = Item::begin(Tool::Spotlight, PointF::new(10.0, 10.0), s, &[], Rgba::WHITE).unwrap();
        a.drag(PointF::new(10.0, 10.0), PointF::new(50.0, 50.0), false);
        let mut b = a.clone();
        b.translate(100.0, 0.0);
        assert_eq!(a.outline(), Bx { x0: 10.0, y0: 10.0, x1: 50.0, y1: 50.0 });
        assert!(a.hit(PointF::new(30.0, 30.0)) && !a.hit(PointF::new(80.0, 30.0)));
        let mut pm = canvas();
        render(&mut pm.as_mut(), &[a, b], 0.0, 0.0);
        // Inside either spotlight: untouched. Outside both: dimmed once (not twice).
        assert_eq!(pixel_at(&pm, 30, 30), Some(Rgba::WHITE));
        assert_eq!(pixel_at(&pm, 130, 30), Some(Rgba::WHITE));
        let out = pixel_at(&pm, 80, 90).unwrap();
        let once = 255 - (255.0 * SPOTLIGHT_DIM as f32 / 100.0) as u8;
        assert!(out.r.abs_diff(once) <= 3, "{out:?}");
    }

    #[test]
    fn step_labels() {
        assert_eq!(StepType::Numbers.label(12), "12");
        assert_eq!(StepType::Upper.label(1), "A");
        assert_eq!(StepType::Upper.label(27), "AA");
        assert_eq!(StepType::Lower.label(3), "c");
        assert_eq!(StepType::RomanUpper.label(14), "XIV");
        assert_eq!(StepType::RomanLower.label(9), "ix");
    }

    #[test]
    fn shadow_falls_below_and_fill_uses_fill_colour() {
        let st = Style { fill: true, fill_color: Rgba::rgb(0, 0, 255), shadow: true, corner_radius: 6.0, ..Style::default() };
        let mut it = Item::begin(Tool::Rect, PointF::new(20.0, 20.0), st, &[], Rgba::WHITE).unwrap();
        it.drag(PointF::new(20.0, 20.0), PointF::new(80.0, 60.0), false);
        let mut pm = canvas();
        render(&mut pm.as_mut(), &[it.clone()], 0.0, 0.0);
        assert_eq!(pixel_at(&pm, 50, 40), Some(Rgba::rgb(0, 0, 255)));
        // Just below the bottom border: shadowed (darker than the white canvas).
        let below = pixel_at(&pm, 50, 64).unwrap();
        assert!(below.r < 250, "{below:?}");
        assert!(it.bounds().y1 > it.shape_bounds().y1);
    }

    #[test]
    fn arrows_can_have_two_heads() {
        let st = Style { heads: ArrowHeads::Both, width: 3.0, ..Style::default() };
        let mut it = Item::begin(Tool::Arrow, PointF::new(20.0, 60.0), st, &[], Rgba::WHITE).unwrap();
        it.drag(PointF::new(20.0, 60.0), PointF::new(180.0, 60.0), false);
        let mut pm = canvas();
        render(&mut pm.as_mut(), &[it], 0.0, 0.0);
        // Heads are much wider than the 3 px shaft, at both ends.
        assert_eq!(pixel_at(&pm, 34, 65), Some(Rgba::RED));
        assert_eq!(pixel_at(&pm, 166, 65), Some(Rgba::RED));
        assert_eq!(pixel_at(&pm, 100, 65), Some(Rgba::WHITE));
    }

    #[test]
    fn freehand_arrow_heads_follow_the_path() {
        let st = Style { heads: ArrowHeads::Both, width: 3.0, ..Style::default() };
        let s = PointF::new(20.0, 30.0);
        let mut it = Item { shape: Shape::FreeArrow(vec![s]), style: st };
        // Right along y = 30, then down along x = 100.
        for i in 1..=8 {
            it.drag(s, PointF::new(20.0 + i as f32 * 10.0, 30.0), false);
        }
        for i in 1..=7 {
            it.drag(s, PointF::new(100.0, 30.0 + i as f32 * 10.0), false);
        }
        assert_eq!(it.tool(), Some(Tool::Arrow));
        assert!(!it.is_degenerate());
        let mut pm = canvas();
        render(&mut pm.as_mut(), std::slice::from_ref(&it), 0.0, 0.0);
        // The start head points left (wide in y), the end head points down (wide in x).
        assert_eq!(pixel_at(&pm, 34, 36), Some(Rgba::RED));
        assert_eq!(pixel_at(&pm, 106, 86), Some(Rgba::RED));
        assert_eq!(pixel_at(&pm, 60, 36), Some(Rgba::WHITE));
        let b = it.bounds();
        assert!(b.x1 >= 108.0 && b.y0 <= 22.0);
    }

    #[test]
    fn shift_constrains_lines_to_45_degrees() {
        let s = PointF::new(0.0, 0.0);
        let mut l = Item::begin(Tool::Line, s, Style::default(), &[], Rgba::WHITE).unwrap();
        l.drag(s, PointF::new(100.0, 10.0), true);
        if let Shape::Line(_, b) = l.shape {
            assert!(b.y.abs() < 0.01 && (b.x - 100.5).abs() < 1.0);
        }
    }
}
