//! Toolbar icons drawn as vector strokes (original line-art, no icon font or assets).

use snapit_annotate::{stickers, Bx, Sticker};
use snapit_gfx::effects::rounded_rect;
use snapit_gfx::text;
use snapit_gfx::tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, PixmapMut, Stroke, Transform};
use snapit_gfx::Rgba;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
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
    Sticker,
    Image,
    Crop,
    CutOut,
    Effects,
    Undo,
    Redo,
    Copy,
    Save,
    SaveAs,
    Pin,
    Editor,
    Close,
    Done,
    Fill,
    ZoomFit,
    Folder,
    Spotlight,
}

struct Pen<'a, 'b> {
    pm: &'a mut PixmapMut<'b>,
    x: f32,
    y: f32,
    s: f32,
    c: Rgba,
    w: f32,
}

impl Pen<'_, '_> {
    fn pt(&self, px: f32, py: f32) -> (f32, f32) {
        (self.x + px * self.s, self.y + py * self.s)
    }
    fn poly(&mut self, pts: &[(f32, f32)], close: bool) {
        let mut pb = PathBuilder::new();
        let (x, y) = self.pt(pts[0].0, pts[0].1);
        pb.move_to(x, y);
        for p in &pts[1..] {
            let (x, y) = self.pt(p.0, p.1);
            pb.line_to(x, y);
        }
        if close {
            pb.close();
        }
        if let Some(p) = pb.finish() {
            let st = Stroke { width: self.w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
            self.pm.stroke_path(&p, &self.c.paint(), &st, Transform::identity(), None);
        }
    }
    fn fill_poly(&mut self, pts: &[(f32, f32)]) {
        let mut pb = PathBuilder::new();
        let (x, y) = self.pt(pts[0].0, pts[0].1);
        pb.move_to(x, y);
        for p in &pts[1..] {
            let (x, y) = self.pt(p.0, p.1);
            pb.line_to(x, y);
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.pm.fill_path(&p, &self.c.paint(), FillRule::Winding, Transform::identity(), None);
        }
    }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, fill: bool) {
        let (ax, ay) = self.pt(x0, y0);
        let (bx, by) = self.pt(x1, y1);
        if let Some(p) = rounded_rect(ax, ay, bx - ax, by - ay, radius * self.s) {
            if fill {
                self.pm.fill_path(&p, &self.c.paint(), FillRule::Winding, Transform::identity(), None);
            } else {
                let st = Stroke { width: self.w, line_join: LineJoin::Round, ..Default::default() };
                self.pm.stroke_path(&p, &self.c.paint(), &st, Transform::identity(), None);
            }
        }
    }
    fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: bool) {
        let (x, y) = self.pt(cx, cy);
        if let Some(p) = PathBuilder::from_circle(x, y, r * self.s) {
            if fill {
                self.pm.fill_path(&p, &self.c.paint(), FillRule::Winding, Transform::identity(), None);
            } else {
                let st = Stroke { width: self.w, ..Default::default() };
                self.pm.stroke_path(&p, &self.c.paint(), &st, Transform::identity(), None);
            }
        }
    }
    fn curve(&mut self, pts: &[(f32, f32)]) {
        // Smooth curve through points using quadratic midpoints.
        let mut pb = PathBuilder::new();
        let (x, y) = self.pt(pts[0].0, pts[0].1);
        pb.move_to(x, y);
        for w in pts.windows(2) {
            let (cx, cy) = self.pt(w[0].0, w[0].1);
            let (mx, my) = self.pt((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0);
            pb.quad_to(cx, cy, mx, my);
        }
        let l = pts[pts.len() - 1];
        let (x, y) = self.pt(l.0, l.1);
        pb.line_to(x, y);
        if let Some(p) = pb.finish() {
            let st = Stroke { width: self.w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
            self.pm.stroke_path(&p, &self.c.paint(), &st, Transform::identity(), None);
        }
    }
    fn label(&mut self, s: &str, cx: f32, cy: f32, size: f32, bold: bool) {
        let size = size * self.s;
        let l = text::layout(s, size, bold);
        let (x, y) = self.pt(cx, cy);
        text::draw(self.pm, s, x - l.width / 2.0, y - l.height / 2.0, size, bold, self.c, None);
    }
}

/// Draws `icon` in the square at (x, y) with side `size`.
pub fn draw(pm: &mut PixmapMut, icon: Icon, x: f32, y: f32, size: f32, c: Rgba) {
    let w = (size * 0.085).max(1.3);
    let mut p = Pen { pm, x, y, s: size, c, w };
    match icon {
        Icon::Pointer => p.fill_poly(&[(0.3, 0.18), (0.3, 0.8), (0.45, 0.65), (0.56, 0.86), (0.65, 0.82), (0.54, 0.61), (0.74, 0.61)]),
        Icon::Rect => p.rect(0.2, 0.26, 0.8, 0.74, 0.04, false),
        Icon::Ellipse => {
            let (cx, cy) = p.pt(0.5, 0.5);
            if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(cx - 0.32 * size, cy - 0.24 * size, 0.64 * size, 0.48 * size) {
                if let Some(path) = PathBuilder::from_oval(r) {
                    let st = Stroke { width: w, ..Default::default() };
                    p.pm.stroke_path(&path, &c.paint(), &st, Transform::identity(), None);
                }
            }
        }
        Icon::Line => p.poly(&[(0.22, 0.78), (0.78, 0.22)], false),
        Icon::Arrow => {
            p.poly(&[(0.22, 0.78), (0.7, 0.3)], false);
            p.fill_poly(&[(0.8, 0.2), (0.48, 0.3), (0.7, 0.52)]);
        }
        Icon::Pen => p.curve(&[(0.18, 0.7), (0.32, 0.3), (0.5, 0.62), (0.66, 0.3), (0.82, 0.5)]),
        Icon::Highlight => {
            p.fill_poly(&[(0.58, 0.18), (0.82, 0.42), (0.5, 0.74), (0.26, 0.5)]);
            p.poly(&[(0.26, 0.5), (0.2, 0.74), (0.5, 0.74)], false);
            p.c = c.with_alpha(130);
            p.rect(0.16, 0.8, 0.84, 0.88, 0.02, true);
        }
        Icon::Blur => {
            p.c = c.with_alpha(70);
            p.circle(0.5, 0.5, 0.32, true);
            p.c = c.with_alpha(140);
            p.circle(0.5, 0.5, 0.22, true);
            p.c = c;
            p.circle(0.5, 0.5, 0.12, true);
        }
        Icon::Pixelate => {
            for (i, (gx, gy)) in [(0, 0), (2, 0), (1, 1), (0, 2), (2, 2), (1, 0), (0, 1), (2, 1), (1, 2)].iter().enumerate() {
                p.c = c.with_alpha(if i < 5 { 255 } else { 110 });
                let (x0, y0) = (0.2 + *gx as f32 * 0.2, 0.2 + *gy as f32 * 0.2);
                p.rect(x0, y0, x0 + 0.19, y0 + 0.19, 0.0, true);
            }
        }
        Icon::Eraser => {
            p.poly(&[(0.2, 0.62), (0.52, 0.3), (0.78, 0.56), (0.5, 0.84), (0.34, 0.84)], true);
            p.poly(&[(0.36, 0.46), (0.62, 0.72)], false);
            p.poly(&[(0.5, 0.84), (0.84, 0.84)], false);
        }
        Icon::Step => {
            p.circle(0.5, 0.5, 0.32, false);
            p.label("1", 0.5, 0.5, 0.42, true);
        }
        Icon::Text => p.label("T", 0.5, 0.5, 0.7, true),
        Icon::Balloon => {
            p.rect(0.16, 0.2, 0.84, 0.64, 0.12, false);
            p.poly(&[(0.32, 0.64), (0.28, 0.84), (0.48, 0.64)], false);
        }
        Icon::Spotlight => {
            // A dimmed frame with a bright hole in it.
            p.c = c.with_alpha(110);
            p.rect(0.14, 0.2, 0.86, 0.8, 0.06, true);
            p.c = c;
            p.rect(0.14, 0.2, 0.86, 0.8, 0.06, false);
            if let Some(o) = snapit_gfx::tiny_skia::Rect::from_xywh(x + size * 0.32, y + size * 0.34, size * 0.36, size * 0.32).and_then(PathBuilder::from_oval) {
                p.pm.fill_path(&o, &c.paint(), FillRule::Winding, Transform::identity(), None);
            }
        }
        Icon::Magnify => {
            p.circle(0.44, 0.44, 0.22, false);
            p.poly(&[(0.6, 0.6), (0.8, 0.8)], false);
        }
        Icon::Sticker => stickers::draw(p.pm, Sticker::Star, Bx { x0: x + size * 0.14, y0: y + size * 0.14, x1: x + size * 0.86, y1: y + size * 0.86 }),
        Icon::Image => {
            p.rect(0.16, 0.22, 0.84, 0.78, 0.06, false);
            p.poly(&[(0.22, 0.72), (0.42, 0.5), (0.56, 0.62), (0.66, 0.54), (0.8, 0.7)], false);
            p.circle(0.66, 0.36, 0.06, true);
        }
        Icon::Crop => {
            p.poly(&[(0.3, 0.14), (0.3, 0.7), (0.86, 0.7)], false);
            p.poly(&[(0.14, 0.3), (0.7, 0.3), (0.7, 0.86)], false);
        }
        Icon::CutOut => {
            p.circle(0.3, 0.72, 0.1, false);
            p.circle(0.7, 0.72, 0.1, false);
            p.poly(&[(0.36, 0.64), (0.66, 0.18)], false);
            p.poly(&[(0.64, 0.64), (0.34, 0.18)], false);
        }
        Icon::Effects => {
            p.fill_poly(&[(0.42, 0.14), (0.5, 0.36), (0.72, 0.44), (0.5, 0.52), (0.42, 0.74), (0.34, 0.52), (0.12, 0.44), (0.34, 0.36)]);
            p.fill_poly(&[(0.74, 0.56), (0.78, 0.68), (0.9, 0.72), (0.78, 0.76), (0.74, 0.88), (0.7, 0.76), (0.58, 0.72), (0.7, 0.68)]);
        }
        Icon::Undo => {
            p.curve(&[(0.24, 0.42), (0.5, 0.3), (0.78, 0.42), (0.78, 0.66), (0.52, 0.74)]);
            p.fill_poly(&[(0.16, 0.42), (0.36, 0.26), (0.36, 0.56)]);
        }
        Icon::Redo => {
            p.curve(&[(0.76, 0.42), (0.5, 0.3), (0.22, 0.42), (0.22, 0.66), (0.48, 0.74)]);
            p.fill_poly(&[(0.84, 0.42), (0.64, 0.26), (0.64, 0.56)]);
        }
        Icon::Copy => {
            p.rect(0.34, 0.16, 0.82, 0.66, 0.06, false);
            p.poly(&[(0.24, 0.32), (0.18, 0.32), (0.18, 0.84), (0.66, 0.84), (0.66, 0.78)], false);
        }
        Icon::Save | Icon::SaveAs => {
            p.poly(&[(0.5, 0.16), (0.5, 0.6)], false);
            p.poly(&[(0.32, 0.44), (0.5, 0.62), (0.68, 0.44)], false);
            p.poly(&[(0.18, 0.64), (0.18, 0.82), (0.82, 0.82), (0.82, 0.64)], false);
            if icon == Icon::SaveAs {
                p.circle(0.82, 0.22, 0.07, true);
            }
        }
        Icon::Pin => {
            p.fill_poly(&[(0.36, 0.16), (0.64, 0.16), (0.6, 0.42), (0.74, 0.56), (0.26, 0.56), (0.4, 0.42)]);
            p.poly(&[(0.5, 0.56), (0.5, 0.86)], false);
        }
        Icon::Editor => {
            p.rect(0.16, 0.2, 0.78, 0.8, 0.06, false);
            p.poly(&[(0.42, 0.62), (0.84, 0.2)], false);
            p.poly(&[(0.38, 0.66), (0.34, 0.7)], false);
        }
        Icon::Close => {
            p.poly(&[(0.28, 0.28), (0.72, 0.72)], false);
            p.poly(&[(0.72, 0.28), (0.28, 0.72)], false);
        }
        Icon::Done => p.poly(&[(0.22, 0.52), (0.42, 0.72), (0.8, 0.3)], false),
        Icon::Fill => p.rect(0.22, 0.26, 0.78, 0.74, 0.06, true),
        Icon::ZoomFit => {
            p.poly(&[(0.18, 0.36), (0.18, 0.18), (0.36, 0.18)], false);
            p.poly(&[(0.64, 0.18), (0.82, 0.18), (0.82, 0.36)], false);
            p.poly(&[(0.82, 0.64), (0.82, 0.82), (0.64, 0.82)], false);
            p.poly(&[(0.36, 0.82), (0.18, 0.82), (0.18, 0.64)], false);
        }
        Icon::Folder => p.poly(&[(0.14, 0.3), (0.38, 0.3), (0.46, 0.38), (0.86, 0.38), (0.86, 0.78), (0.14, 0.78)], true),
    }
}
