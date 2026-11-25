//! Built-in stickers, drawn as vector art (original shapes, no bundled image assets).

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::text;
use snapit_gfx::tiny_skia::{FillRule, LineCap, LineJoin, Path, PathBuilder, PixmapMut, Stroke, Transform};
use snapit_gfx::Rgba;

use crate::Bx;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sticker {
    Check,
    Cross,
    Star,
    Heart,
    Warning,
    Info,
    Question,
    New,
}

impl Sticker {
    pub const ALL: [Sticker; 8] = [
        Sticker::Check,
        Sticker::Cross,
        Sticker::Star,
        Sticker::Heart,
        Sticker::Warning,
        Sticker::Info,
        Sticker::Question,
        Sticker::New,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Sticker::Check => "Check",
            Sticker::Cross => "Cross",
            Sticker::Star => "Star",
            Sticker::Heart => "Heart",
            Sticker::Warning => "Warning",
            Sticker::Info => "Info",
            Sticker::Question => "Question",
            Sticker::New => "New",
        }
    }
}

fn circle(c: f32) -> Option<Path> {
    PathBuilder::from_circle(c, c, c * 0.96)
}

fn star(s: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (cx, cy) = (s / 2.0, s * 0.54);
    for i in 0..10 {
        let r = if i % 2 == 0 { s * 0.5 } else { s * 0.21 };
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

fn heart(s: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(s * 0.5, s * 0.92);
    pb.cubic_to(s * 0.1, s * 0.62, s * 0.0, s * 0.38, s * 0.12, s * 0.2);
    pb.cubic_to(s * 0.26, s * 0.02, s * 0.46, s * 0.08, s * 0.5, s * 0.26);
    pb.cubic_to(s * 0.54, s * 0.08, s * 0.74, s * 0.02, s * 0.88, s * 0.2);
    pb.cubic_to(s, s * 0.38, s * 0.9, s * 0.62, s * 0.5, s * 0.92);
    pb.close();
    pb.finish()
}

fn triangle(s: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(s * 0.5, s * 0.06);
    pb.line_to(s * 0.97, s * 0.9);
    pb.line_to(s * 0.03, s * 0.9);
    pb.close();
    pb.finish()
}

fn glyph(pm: &mut PixmapMut, s: &str, ox: f32, oy: f32, size: f32, cx: f32, cy: f32, color: Rgba) {
    let l = text::layout(s, size, true);
    text::draw(pm, s, ox + cx - l.width / 2.0, oy + cy - l.height / 2.0, size, true, color, None);
}

/// Draws `kind` fitted (square, centred) inside `r`, given in pixmap coordinates.
pub fn draw(pm: &mut PixmapMut, kind: Sticker, r: Bx) {
    let n = r.norm();
    let s = n.w().min(n.h());
    if s < 2.0 {
        return;
    }
    let ox = n.x0 + (n.w() - s) / 2.0;
    let oy = n.y0 + (n.h() - s) / 2.0;
    let t = Transform::from_translate(ox, oy);
    let fill = |pm: &mut PixmapMut, p: Option<Path>, c: Rgba| {
        if let Some(p) = p {
            pm.fill_path(&p, &c.paint(), FillRule::Winding, t, None);
        }
    };
    let line = |pm: &mut PixmapMut, pts: &[(f32, f32)], c: Rgba, w: f32| {
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0 * s, pts[0].1 * s);
        for p in &pts[1..] {
            pb.line_to(p.0 * s, p.1 * s);
        }
        if let Some(p) = pb.finish() {
            let st = Stroke { width: w * s, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
            pm.stroke_path(&p, &c.paint(), &st, t, None);
        }
    };
    match kind {
        Sticker::Check => {
            fill(pm, circle(s / 2.0), Rgba::rgb(34, 197, 94));
            line(pm, &[(0.27, 0.52), (0.43, 0.68), (0.74, 0.35)], Rgba::WHITE, 0.11);
        }
        Sticker::Cross => {
            fill(pm, circle(s / 2.0), Rgba::rgb(239, 68, 68));
            line(pm, &[(0.32, 0.32), (0.68, 0.68)], Rgba::WHITE, 0.11);
            line(pm, &[(0.68, 0.32), (0.32, 0.68)], Rgba::WHITE, 0.11);
        }
        Sticker::Star => {
            let p = star(s);
            if let Some(p) = &p {
                pm.stroke_path(p, &Rgba::rgb(234, 88, 12).paint(), &Stroke { width: s * 0.06, line_join: LineJoin::Round, ..Default::default() }, t, None);
            }
            fill(pm, p, Rgba::rgb(250, 204, 21));
        }
        Sticker::Heart => fill(pm, heart(s), Rgba::rgb(244, 63, 94)),
        Sticker::Warning => {
            let p = triangle(s);
            if let Some(p) = &p {
                pm.stroke_path(p, &Rgba::rgb(250, 204, 21).paint(), &Stroke { width: s * 0.08, line_join: LineJoin::Round, ..Default::default() }, t, None);
            }
            fill(pm, p, Rgba::rgb(250, 204, 21));
            line(pm, &[(0.5, 0.36), (0.5, 0.6)], Rgba::rgb(30, 30, 30), 0.09);
            fill(pm, PathBuilder::from_circle(s * 0.5, s * 0.75, s * 0.055), Rgba::rgb(30, 30, 30));
        }
        Sticker::Info => {
            fill(pm, circle(s / 2.0), Rgba::rgb(59, 130, 246));
            line(pm, &[(0.5, 0.45), (0.5, 0.72)], Rgba::WHITE, 0.1);
            fill(pm, PathBuilder::from_circle(s * 0.5, s * 0.29, s * 0.06), Rgba::WHITE);
        }
        Sticker::Question => {
            fill(pm, circle(s / 2.0), Rgba::rgb(168, 85, 247));
            glyph(pm, "?", ox, oy, s * 0.62, s * 0.5, s * 0.47, Rgba::WHITE);
        }
        Sticker::New => {
            let h = s * 0.5;
            fill(pm, rounded_rect(0.0, (s - h) / 2.0, s, h, h * 0.3), Rgba::rgb(249, 115, 22));
            glyph(pm, "NEW", ox, oy, h * 0.55, s * 0.5, s * 0.48, Rgba::WHITE);
        }
    }
}
