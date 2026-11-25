//! Text as vector paths. Glyph outlines come from the system's installed fonts
//! (Segoe UI, with fallbacks) through `ab_glyph`, and are turned into tiny-skia
//! paths so text can be filled, outlined and transformed like any other shape.

use std::path::PathBuf;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, GlyphId, OutlineCurve};
use tiny_skia::{FillRule, Path, PathBuilder, PixmapMut, Stroke, Transform};

use crate::Rgba;

struct Fonts {
    regular: Vec<FontVec>,
    bold: Vec<FontVec>,
}

fn fonts_dir() -> PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(windir).join("Fonts")
}

fn load(names: &[&str]) -> Vec<FontVec> {
    let dir = fonts_dir();
    names
        .iter()
        .filter_map(|n| std::fs::read(dir.join(n)).ok())
        .filter_map(|d| FontVec::try_from_vec(d).ok())
        .collect()
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| Fonts {
        regular: load(&["segoeui.ttf", "seguisym.ttf", "arial.ttf"]),
        bold: load(&["segoeuib.ttf", "seguisym.ttf", "arialbd.ttf"]),
    })
}

/// True when at least one system font loaded (always the case on normal Windows installs).
pub fn available() -> bool {
    !fonts().regular.is_empty()
}

pub struct TextLayout {
    /// Glyph outlines with the text's top-left at (0, 0). `None` for blank text.
    pub path: Option<Path>,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    /// Width of the final line, for placing a caret after the last character.
    pub last_line_width: f32,
    pub lines: usize,
}

fn pick(fonts: &[FontVec], c: char) -> Option<(&FontVec, GlyphId)> {
    fonts.iter().find_map(|f| {
        let id = f.glyph_id(c);
        (id.0 != 0).then_some((f, id))
    })
}

pub fn layout(text: &str, size: f32, bold: bool) -> TextLayout {
    let set = if bold { &fonts().bold } else { &fonts().regular };
    let Some(primary) = set.first() else {
        return TextLayout { path: None, width: 0.0, height: size, line_height: size, last_line_width: 0.0, lines: 1 };
    };
    let upem = primary.units_per_em().unwrap_or(2048.0);
    let s = size / upem;
    let ascent = primary.ascent_unscaled() * s;
    let line_height = (primary.ascent_unscaled() - primary.descent_unscaled() + primary.line_gap_unscaled()) * s;

    let mut pb = PathBuilder::new();
    let mut max_w: f32 = 0.0;
    let mut line_w = 0.0;
    let mut lines = 0;
    for (li, line) in text.split('\n').enumerate() {
        lines += 1;
        let baseline = ascent + li as f32 * line_height;
        let mut x = 0.0f32;
        let mut prev: Option<(&FontVec, GlyphId)> = None;
        for c in line.chars() {
            let Some((font, id)) = pick(set, c).or_else(|| Some((primary, primary.glyph_id(c)))) else { continue };
            let fs = size / font.units_per_em().unwrap_or(upem);
            if let Some((pf, pid)) = prev {
                if std::ptr::eq(pf, font) {
                    x += font.kern_unscaled(pid, id) * fs;
                }
            }
            if let Some(outline) = font.outline(id) {
                append_outline(&mut pb, &outline.curves, x, baseline, fs);
            }
            x += font.h_advance_unscaled(id) * fs;
            prev = Some((font, id));
        }
        max_w = max_w.max(x);
        line_w = x;
    }
    TextLayout {
        path: pb.finish(),
        width: max_w,
        height: lines as f32 * line_height,
        line_height,
        last_line_width: line_w,
        lines,
    }
}

fn append_outline(pb: &mut PathBuilder, curves: &[OutlineCurve], ox: f32, baseline: f32, s: f32) {
    let tx = |p: ab_glyph::Point| (ox + p.x * s, baseline - p.y * s);
    let mut last: Option<ab_glyph::Point> = None;
    let start = |pb: &mut PathBuilder, p: ab_glyph::Point, last: &Option<ab_glyph::Point>| {
        let moved = last.is_none_or(|l| (l.x - p.x).abs() > 0.01 || (l.y - p.y).abs() > 0.01);
        if moved {
            if last.is_some() {
                pb.close();
            }
            let (x, y) = tx(p);
            pb.move_to(x, y);
        }
    };
    for c in curves {
        match *c {
            OutlineCurve::Line(a, b) => {
                start(pb, a, &last);
                let (x, y) = tx(b);
                pb.line_to(x, y);
                last = Some(b);
            }
            OutlineCurve::Quad(a, b, c2) => {
                start(pb, a, &last);
                let (x1, y1) = tx(b);
                let (x, y) = tx(c2);
                pb.quad_to(x1, y1, x, y);
                last = Some(c2);
            }
            OutlineCurve::Cubic(a, b, c2, d) => {
                start(pb, a, &last);
                let (x1, y1) = tx(b);
                let (x2, y2) = tx(c2);
                let (x, y) = tx(d);
                pb.cubic_to(x1, y1, x2, y2, x, y);
                last = Some(d);
            }
        }
    }
    if last.is_some() {
        pb.close();
    }
}

/// Draws text with its top-left at (x, y). `outline` adds a stroke behind the fill.
pub fn draw(pm: &mut PixmapMut, text: &str, x: f32, y: f32, size: f32, bold: bool, color: Rgba, outline: Option<(Rgba, f32)>) {
    let l = layout(text, size, bold);
    if let Some(path) = &l.path {
        let t = Transform::from_translate(x, y);
        if let Some((oc, ow)) = outline {
            let stroke = Stroke { width: ow, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
            pm.stroke_path(path, &oc.paint(), &stroke, t, None);
        }
        pm.fill_path(path, &color.paint(), FillRule::Winding, t, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_measures_and_draws() {
        if !available() {
            return; // no system fonts (non-Windows CI)
        }
        let one = layout("Hello", 20.0, false);
        let two = layout("Hello\nWorld!", 20.0, false);
        assert!(one.width > 30.0 && one.path.is_some());
        assert_eq!(two.lines, 2);
        assert!((two.height - 2.0 * one.height).abs() < 0.01);
        assert!(layout("   ", 20.0, false).path.is_none());

        let mut pm = tiny_skia::Pixmap::new(80, 30).unwrap();
        draw(&mut pm.as_mut(), "Hi", 2.0, 2.0, 20.0, true, Rgba::BLACK, Some((Rgba::WHITE, 3.0)));
        assert!(pm.data().as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }
}
