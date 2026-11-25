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
use snapit_gfx::tiny_skia::{
    BlendMode, FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pattern, Pixmap,
    PixmapMut, SpreadMode, Stroke, Transform,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Rgba,
    pub width: f32,
    pub font_size: f32,
    pub fill: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style { color: Rgba::RED, width: 4.0, font_size: 22.0, fill: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Bx),
    Ellipse(Bx),
    Line(PointF, PointF),
    Arrow(PointF, PointF),
    Pen(Vec<PointF>),
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
            Tool::Sticker(kind) => Shape::Sticker { r: b, kind },
        };
        Some(Item { shape, style })
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
            Shape::Magnify { r, .. } | Shape::Sticker { r, .. } => *r = Bx::new(start, boxed(p)),
            Shape::Balloon { r, tail, .. } => {
                *r = Bx::new(start, p);
                let n = r.norm();
                *tail = PointF::new(n.x0 + n.w() * 0.25, n.y1 + (n.h() * 0.5).clamp(16.0, 48.0));
            }
            Shape::Line(_, b) | Shape::Arrow(_, b) => *b = snap(p),
            Shape::Pen(pts) => {
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
            Shape::Magnify { r, .. } | Shape::Sticker { r, .. } | Shape::Image { r, .. } => r.w() < 6.0 || r.h() < 6.0,
            Shape::Line(a, b) | Shape::Arrow(a, b) => a.dist(*b) < 4.0,
            Shape::Pen(p) => p.len() < 2,
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Balloon { .. } | Shape::Step { .. } => false,
        }
    }

    /// The resizable box of box-shaped items (images, stickers, rectangles…).
    pub fn boxed_mut(&mut self) -> Option<&mut Bx> {
        match &mut self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => Some(b),
            Shape::Magnify { r, .. } | Shape::Image { r, .. } | Shape::Sticker { r, .. } => Some(r),
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

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut PointF| *p = p.offset(dx, dy);
        match &mut self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) | Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => *b = b.offset(dx, dy),
            Shape::Magnify { r, .. } | Shape::Image { r, .. } | Shape::Sticker { r, .. } => *r = r.offset(dx, dy),
            Shape::Balloon { r, tail, .. } => {
                *r = r.offset(dx, dy);
                mv(tail);
            }
            Shape::Line(a, b) | Shape::Arrow(a, b) => {
                mv(a);
                mv(b);
            }
            Shape::Pen(pts) => pts.iter_mut().for_each(mv),
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

    /// Bounding box in document space, including stroke width and decorations.
    pub fn bounds(&self) -> Bx {
        let w = self.style.width;
        let grow = |b: Bx, d: f32| {
            let n = b.norm();
            Bx { x0: n.x0 - d, y0: n.y0 - d, x1: n.x1 + d, y1: n.y1 + d }
        };
        match &self.shape {
            Shape::Rect(b) | Shape::Ellipse(b) => grow(*b, w),
            Shape::Highlight(b) | Shape::Blur(b) | Shape::Pixelate(b) | Shape::Eraser(b, _) => grow(*b, 1.0),
            Shape::Magnify { r, .. } | Shape::Image { r, .. } | Shape::Sticker { r, .. } => grow(*r, 2.0),
            Shape::Line(a, b) => grow(Bx::new(*a, *b), w),
            Shape::Arrow(a, b) => grow(Bx::new(*a, *b), w * 4.0 + 12.0),
            Shape::Pen(pts) => {
                let mut b = Bx::new(pts[0], pts[0]).norm();
                for p in pts {
                    b = Bx { x0: b.x0.min(p.x), y0: b.y0.min(p.y), x1: b.x1.max(p.x), y1: b.y1.max(p.y) };
                }
                grow(b, w)
            }
            Shape::Step { at, .. } => {
                let r = self.step_radius() + 2.0;
                Bx { x0: at.x - r, y0: at.y - r, x1: at.x + r, y1: at.y + r }
            }
            Shape::Text { at, text, .. } => {
                let l = text::layout(if text.is_empty() { " " } else { text }, self.style.font_size, false);
                let pad = self.style.font_size * 0.35;
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
            Shape::Pen(pts) => pts.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= tol) || pts[0].dist(p) <= tol,
            Shape::Rect(b) if !self.style.fill => {
                // Outline rectangles are hit only near their edges.
                let n = b.norm();
                let near_x = (p.x - n.x0).abs() <= tol || (p.x - n.x1).abs() <= tol;
                let near_y = (p.y - n.y0).abs() <= tol || (p.y - n.y1).abs() <= tol;
                let in_x = p.x >= n.x0 - tol && p.x <= n.x1 + tol;
                let in_y = p.y >= n.y0 - tol && p.y <= n.y1 + tol;
                (near_x && in_y) || (near_y && in_x)
            }
            _ => self.bounds().contains(p),
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

/// Draws all items. Item coordinates plus `(ox, oy)` give pixmap coordinates.
pub fn render(pm: &mut PixmapMut, items: &[Item], ox: f32, oy: f32) {
    for it in items {
        render_item(pm, it, ox, oy);
    }
}

pub fn render_item(pm: &mut PixmapMut, it: &Item, ox: f32, oy: f32) {
    let t = Transform::from_translate(ox, oy);
    let st = &it.style;
    let paint = st.color.paint();
    match &it.shape {
        Shape::Rect(b) => {
            if let Some(r) = b.skia() {
                let path = PathBuilder::from_rect(r);
                if st.fill {
                    pm.fill_path(&path, &paint, FillRule::Winding, t, None);
                } else {
                    let s = Stroke { width: st.width, line_join: LineJoin::Miter, ..Default::default() };
                    pm.stroke_path(&path, &paint, &s, t, None);
                }
            }
        }
        Shape::Ellipse(b) => {
            if let Some(path) = b.skia().and_then(PathBuilder::from_oval) {
                if st.fill {
                    pm.fill_path(&path, &paint, FillRule::Winding, t, None);
                } else {
                    pm.stroke_path(&path, &paint, &stroke(st.width), t, None);
                }
            }
        }
        Shape::Line(a, b) => {
            let mut pb = PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint, &stroke(st.width), t, None);
            }
        }
        Shape::Arrow(a, b) => draw_arrow(pm, *a, *b, st, t),
        Shape::Pen(pts) => {
            let mut pb = PathBuilder::new();
            pb.move_to(pts[0].x, pts[0].y);
            if pts.len() == 1 {
                pb.line_to(pts[0].x + 0.1, pts[0].y);
            }
            for w in pts.windows(2).skip(1) {
                let mid = PointF::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
                pb.quad_to(w[0].x, w[0].y, mid.x, mid.y);
            }
            if let Some(l) = pts.last() {
                pb.line_to(l.x, l.y);
            }
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint, &stroke(st.width), t, None);
            }
        }
        Shape::Highlight(b) => {
            if let Some(r) = b.skia() {
                let mut p = st.color.paint();
                p.blend_mode = BlendMode::Multiply;
                pm.fill_rect(r, &p, t, None);
            }
        }
        Shape::Blur(b) => effects::blur(pm, b.offset(ox, oy).to_rect(), (st.width * 2.5).max(4.0) as u32),
        Shape::Pixelate(b) => effects::pixelate(pm, b.offset(ox, oy).to_rect(), (st.width * 3.0).max(6.0) as u32),
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
            let label = n.to_string();
            let size = r * 1.15;
            let l = text::layout(&label, size, true);
            text::draw(pm, &label, ox + at.x - l.width / 2.0, oy + at.y - l.height / 2.0, size, true, contrast(st.color), None);
        }
        Shape::Text { at, text: s, background } => {
            if s.is_empty() {
                return;
            }
            let size = st.font_size;
            if *background {
                let l = text::layout(s, size, false);
                let pad = size * 0.3;
                if let Some(bg) = effects::rounded_rect(at.x - pad, at.y - pad, l.width + pad * 2.0, l.height + pad * 2.0, pad) {
                    pm.fill_path(&bg, &paint, FillRule::Winding, t, None);
                }
                text::draw(pm, s, ox + at.x, oy + at.y, size, false, contrast(st.color), None);
            } else {
                let ow = (size * 0.12).max(2.0);
                text::draw(pm, s, ox + at.x, oy + at.y, size, false, st.color, Some((contrast(st.color), ow)));
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

fn draw_arrow(pm: &mut PixmapMut, a: PointF, b: PointF, st: &Style, t: Transform) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = (st.width * 3.5 + 8.0).min(len * 0.6);
    let half = head * 0.6;
    let base = PointF::new(b.x - ux * head, b.y - uy * head);
    let shaft_end = PointF::new(b.x - ux * head * 0.7, b.y - uy * head * 0.7);
    let paint = st.color.paint();
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(shaft_end.x, shaft_end.y);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint, &stroke(st.width), t, None);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(b.x, b.y);
    pb.line_to(base.x - uy * half, base.y + ux * half);
    pb.line_to(base.x + uy * half, base.y - ux * half);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint, FillRule::Winding, t, None);
        let s = Stroke { width: 1.5, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, &paint, &s, t, None);
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
    pm.stroke_path(&path, &st.color.paint(), &stroke(st.width.clamp(2.0, 6.0)), Transform::identity(), None);
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
    fn shift_constrains_lines_to_45_degrees() {
        let s = PointF::new(0.0, 0.0);
        let mut l = Item::begin(Tool::Line, s, Style::default(), &[], Rgba::WHITE).unwrap();
        l.drag(s, PointF::new(100.0, 10.0), true);
        if let Shape::Line(_, b) = l.shape {
            assert!(b.y.abs() < 0.01 && (b.x - 100.5).abs() < 1.0);
        }
    }
}
