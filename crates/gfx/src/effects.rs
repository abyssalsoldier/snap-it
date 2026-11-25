//! Pixel effects: blur, pixelate, resize, border, shadow, rounded corners, beautify.
//! All operate on premultiplied BGRA pixmaps; channel order never matters here.

use serde::{Deserialize, Serialize};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pattern,
    Pixmap, PixmapMut, Point, SpreadMode, Transform,
};

use crate::{Rect, Rgba};

fn region(pm: &PixmapMut, r: Rect) -> Option<Rect> {
    r.intersect(Rect::new(0, 0, pm.width() as i32, pm.height() as i32))
}

/// Approximate Gaussian blur of a region: three box-blur passes each way.
pub fn blur(pm: &mut PixmapMut, r: Rect, radius: u32) {
    let Some(r) = region(pm, r) else { return };
    let radius = radius.max(1) as i32;
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    let (w, h) = (r.w as usize, r.h as usize);
    let mut buf = vec![0u8; w.max(h) * 4];
    for _ in 0..3 {
        for y in 0..h {
            let row = (r.y as usize + y) * stride + r.x as usize * 4;
            box_line(&mut data[row..row + w * 4], 4, w, radius, &mut buf);
        }
        for x in 0..w {
            let col = r.y as usize * stride + (r.x as usize + x) * 4;
            box_line(&mut data[col..], stride, h, radius, &mut buf);
        }
    }
}

/// Sliding-window box blur along one line of `n` pixels spaced `step` bytes apart.
fn box_line(line: &mut [u8], step: usize, n: usize, radius: i32, tmp: &mut [u8]) {
    if n == 0 {
        return;
    }
    for i in 0..n {
        tmp[i * 4..i * 4 + 4].copy_from_slice(&line[i * step..i * step + 4]);
    }
    let at = |i: i32| -> usize { (i.clamp(0, n as i32 - 1) as usize) * 4 };
    let win = (radius * 2 + 1) as u32;
    let mut sum = [0u32; 4];
    for i in -radius..=radius {
        let p = at(i);
        for c in 0..4 {
            sum[c] += tmp[p + c] as u32;
        }
    }
    for i in 0..n as i32 {
        let o = i as usize * step;
        for c in 0..4 {
            line[o + c] = (sum[c] / win) as u8;
        }
        let add = at(i + radius + 1);
        let sub = at(i - radius);
        for c in 0..4 {
            sum[c] = sum[c] + tmp[add + c] as u32 - tmp[sub + c] as u32;
        }
    }
}

/// Replaces each `block`×`block` cell of the region with its average colour.
pub fn pixelate(pm: &mut PixmapMut, r: Rect, block: u32) {
    let Some(r) = region(pm, r) else { return };
    let block = block.max(2) as i32;
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    let mut by = r.y;
    while by < r.bottom() {
        let bh = block.min(r.bottom() - by);
        let mut bx = r.x;
        while bx < r.right() {
            let bw = block.min(r.right() - bx);
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = y as usize * stride + x as usize * 4;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u32;
                    }
                }
            }
            let n = (bw * bh) as u32;
            let avg = [sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = y as usize * stride + x as usize * 4;
                    for c in 0..4 {
                        data[i + c] = avg[c] as u8;
                    }
                }
            }
            bx += block;
        }
        by += block;
    }
}

/// High-quality resize (Lanczos3).
pub fn resize(pm: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    if w == 0 || h == 0 {
        return None;
    }
    let img = image::RgbaImage::from_raw(pm.width(), pm.height(), pm.data().to_vec())?;
    let out = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
    Pixmap::from_vec(out.into_raw(), tiny_skia::IntSize::from_wh(w, h)?)
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w, h)?));
    }
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Draws `img` at (x, y), clipped to a rounded rectangle with anti-aliased edges.
pub fn draw_rounded(dst: &mut PixmapMut, img: &Pixmap, x: f32, y: f32, radius: f32) {
    let Some(path) = rounded_rect(x, y, img.width() as f32, img.height() as f32, radius) else { return };
    let paint = Paint {
        shader: Pattern::new(img.as_ref(), SpreadMode::Pad, FilterQuality::Nearest, 1.0, Transform::from_translate(x, y)),
        anti_alias: true,
        ..Default::default()
    };
    dst.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Adds a solid border outside the image.
pub fn border(pm: &Pixmap, width: u32, color: Rgba) -> Option<Pixmap> {
    let mut out = Pixmap::new(pm.width() + width * 2, pm.height() + width * 2)?;
    out.fill(color.color());
    crate::blit(&mut out.as_mut(), pm, width as i32, width as i32);
    Some(out)
}

/// Rounds the image's corners (transparent outside).
pub fn round_corners(pm: &Pixmap, radius: f32) -> Option<Pixmap> {
    let mut out = Pixmap::new(pm.width(), pm.height())?;
    draw_rounded(&mut out.as_mut(), pm, 0.0, 0.0, radius);
    Some(out)
}

fn paint_shadow(dst: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, radius: f32, blur_r: u32, opacity: u8) {
    if let Some(path) = rounded_rect(x, y, w, h, radius) {
        dst.fill_path(&path, &Rgba::new(0, 0, 0, opacity).paint(), FillRule::Winding, Transform::identity(), None);
    }
    let full = Rect::new(0, 0, dst.width() as i32, dst.height() as i32);
    blur(dst, full, blur_r);
}

/// Drop shadow on a transparent canvas, padded so the shadow fits.
pub fn drop_shadow(pm: &Pixmap, size: u32) -> Option<Pixmap> {
    let pad = size * 2;
    let mut out = Pixmap::new(pm.width() + pad * 2, pm.height() + pad * 2)?;
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    paint_shadow(&mut out.as_mut(), pad as f32, pad as f32 + size as f32 / 2.0, w, h, 0.0, size, 140);
    crate::blit_over(&mut out.as_mut(), pm, pad as i32, pad as i32);
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Beautify {
    pub padding: u32,
    pub radius: f32,
    pub shadow: bool,
    pub from: Rgba,
    pub to: Rgba,
}

impl Default for Beautify {
    fn default() -> Self {
        BEAUTIFY_PRESETS[0]
    }
}

/// Built-in background gradients for the beautifier.
pub const BEAUTIFY_PRESETS: [Beautify; 4] = [
    Beautify { padding: 64, radius: 12.0, shadow: true, from: Rgba::rgb(99, 102, 241), to: Rgba::rgb(236, 72, 153) },
    Beautify { padding: 64, radius: 12.0, shadow: true, from: Rgba::rgb(20, 184, 166), to: Rgba::rgb(59, 130, 246) },
    Beautify { padding: 64, radius: 12.0, shadow: true, from: Rgba::rgb(251, 146, 60), to: Rgba::rgb(239, 68, 68) },
    Beautify { padding: 64, radius: 12.0, shadow: true, from: Rgba::rgb(55, 65, 81), to: Rgba::rgb(17, 24, 39) },
];

/// Padding + diagonal gradient background + rounded corners + soft shadow.
pub fn beautify(pm: &Pixmap, b: &Beautify) -> Option<Pixmap> {
    let p = b.padding;
    let (ow, oh) = (pm.width() + p * 2, pm.height() + p * 2);
    let mut out = Pixmap::new(ow, oh)?;
    let grad = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(ow as f32, oh as f32),
        vec![GradientStop::new(0.0, b.from.color()), GradientStop::new(1.0, b.to.color())],
        SpreadMode::Pad,
        Transform::identity(),
    )?;
    let paint = Paint { shader: grad, ..Default::default() };
    out.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, ow as f32, oh as f32)?, &paint, Transform::identity(), None);

    let (w, h) = (pm.width() as f32, pm.height() as f32);
    if b.shadow {
        let mut shadow = Pixmap::new(ow, oh)?;
        let s = (p / 3).max(4);
        paint_shadow(&mut shadow.as_mut(), p as f32, p as f32 + s as f32 / 2.0, w, h, b.radius, s, 110);
        out.draw_pixmap(0, 0, shadow.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
    }
    draw_rounded(&mut out.as_mut(), pm, p as f32, p as f32, b.radius);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pixel_at, rgba};

    fn checker(w: u32, h: u32) -> Pixmap {
        let mut pm = Pixmap::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                pm.data_mut()[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        pm
    }

    #[test]
    fn blur_smooths_and_stays_in_region() {
        let mut pm = checker(20, 20);
        blur(&mut pm.as_mut(), Rect::new(5, 5, 10, 10), 2);
        let mid = pixel_at(&pm, 10, 10).unwrap();
        assert!(mid.r > 60 && mid.r < 200, "{mid:?}");
        let outside = pixel_at(&pm, 0, 0).unwrap();
        assert_eq!(outside.r, 255);
    }

    #[test]
    fn pixelate_averages_blocks() {
        let mut pm = checker(8, 8);
        pixelate(&mut pm.as_mut(), Rect::new(0, 0, 8, 8), 4);
        let a = pixel_at(&pm, 0, 0).unwrap();
        let b = pixel_at(&pm, 3, 3).unwrap();
        assert_eq!(a, b);
        assert!((a.r as i32 - 127).abs() <= 1);
    }

    #[test]
    fn size_changing_effects() {
        let mut pm = Pixmap::new(10, 6).unwrap();
        pm.fill(rgba(255, 0, 0, 255));
        assert_eq!(border(&pm, 3, Rgba::BLACK).unwrap().width(), 16);
        assert_eq!(resize(&pm, 20, 12).unwrap().height(), 12);
        let s = drop_shadow(&pm, 4).unwrap();
        assert_eq!((s.width(), s.height()), (26, 22));
        let b = beautify(&pm, &Beautify::default()).unwrap();
        assert_eq!(b.width(), 10 + 128);
        assert_eq!(pixel_at(&b, 69, 67).unwrap(), Rgba::rgb(255, 0, 0));
        let rc = round_corners(&checker(20, 20), 6.0).unwrap();
        assert_eq!(pixel_at(&rc, 0, 0).unwrap().a, 0);
    }
}

/// Rotates 90° clockwise (`cw`) or counter-clockwise.
pub fn rotate90(pm: &Pixmap, cw: bool) -> Option<Pixmap> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut out = Pixmap::new(h as u32, w as u32)?;
    let s = pm.data();
    let d = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = if cw { (h - 1 - y, x) } else { (y, w - 1 - x) };
            let si = (y * w + x) * 4;
            let di = (ny * h + nx) * 4;
            d[di..di + 4].copy_from_slice(&s[si..si + 4]);
        }
    }
    Some(out)
}

/// Mirrors horizontally (`horizontal`) or vertically.
pub fn flip(pm: &Pixmap, horizontal: bool) -> Pixmap {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut out = pm.clone();
    let s = pm.data();
    let d = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = if horizontal { (w - 1 - x, y) } else { (x, h - 1 - y) };
            let si = (y * w + x) * 4;
            let di = (ny * w + nx) * 4;
            d[di..di + 4].copy_from_slice(&s[si..si + 4]);
        }
    }
    out
}

/// Removes columns `[a, b)` (`vertical_band`) or rows `[a, b)` and joins the rest.
pub fn cut_out(pm: &Pixmap, a: u32, b: u32, vertical_band: bool) -> Option<Pixmap> {
    let (w, h) = (pm.width(), pm.height());
    let lim = if vertical_band { w } else { h };
    let (a, b) = (a.min(b).min(lim), a.max(b).min(lim));
    let cut = b - a;
    if cut == 0 || cut >= lim {
        return None;
    }
    if vertical_band {
        let mut out = Pixmap::new(w - cut, h)?;
        crate::blit_rect(&mut out.as_mut(), pm, Rect::new(0, 0, a as i32, h as i32), 0, 0);
        crate::blit_rect(&mut out.as_mut(), pm, Rect::new(b as i32, 0, (w - b) as i32, h as i32), a as i32, 0);
        Some(out)
    } else {
        let mut out = Pixmap::new(w, h - cut)?;
        crate::blit_rect(&mut out.as_mut(), pm, Rect::new(0, 0, w as i32, a as i32), 0, 0);
        crate::blit_rect(&mut out.as_mut(), pm, Rect::new(0, b as i32, w as i32, (h - b) as i32), 0, a as i32);
        Some(out)
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use crate::{pixel_at, rgba};

    #[test]
    fn rotate_flip_cut() {
        let mut pm = Pixmap::new(3, 2).unwrap();
        pm.fill(rgba(0, 0, 0, 255));
        pm.data_mut()[0..4].copy_from_slice(&[0, 0, 255, 255]); // red at (0,0)
        let r = rotate90(&pm, true).unwrap();
        assert_eq!((r.width(), r.height()), (2, 3));
        assert_eq!(pixel_at(&r, 1, 0), Some(Rgba::rgb(255, 0, 0)));
        let l = rotate90(&pm, false).unwrap();
        assert_eq!(pixel_at(&l, 0, 2), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(pixel_at(&flip(&pm, true), 2, 0), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(pixel_at(&flip(&pm, false), 0, 1), Some(Rgba::rgb(255, 0, 0)));
        let c = cut_out(&pm, 1, 2, true).unwrap();
        assert_eq!(c.width(), 2);
        assert_eq!(cut_out(&pm, 0, 1, false).unwrap().height(), 1);
        assert!(cut_out(&pm, 0, 3, true).is_none());
    }
}
