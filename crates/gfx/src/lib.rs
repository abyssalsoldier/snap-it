//! Shared graphics for Snap It.
//!
//! **Pixel-order convention:** every `Pixmap` in Snap It stores **BGRA** bytes
//! (Windows GDI order) even though tiny-skia calls its channels RGBA. That lets
//! a pixmap's memory be blitted straight to a window with no per-frame swizzle.
//! The only places that must know this are [`rgba`] (which swaps R/B when
//! building a paint colour) and the import/export helpers in this crate.

pub mod brand;
pub mod edges;
pub mod effects;
pub mod geom;
pub mod io;
pub mod text;

pub use geom::{PointF, Rect};
pub use tiny_skia;

use serde::{Deserialize, Serialize};
use tiny_skia::{Color, Paint, Pixmap, PixmapMut};

/// Builds a tiny-skia colour for our BGRA pixmaps from normal RGBA components.
pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(b, g, r, a)
}

/// A user-facing colour (settings, annotation styles). Stored as real RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    pub const RED: Rgba = Rgba::rgb(232, 38, 38);
    pub const YELLOW: Rgba = Rgba::rgb(255, 221, 0);

    pub fn color(self) -> Color {
        rgba(self.r, self.g, self.b, self.a)
    }
    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
    pub fn paint(self) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color(self.color());
        p.anti_alias = true;
        p
    }
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    /// Relative luminance, used to pick readable text colour on top of this one.
    pub fn is_light(self) -> bool {
        (self.r as u32 * 299 + self.g as u32 * 587 + self.b as u32 * 114) / 1000 > 150
    }
}

/// Reads one pixel of a BGRA pixmap back as a real RGBA colour.
pub fn pixel_at(pm: &Pixmap, x: i32, y: i32) -> Option<Rgba> {
    if x < 0 || y < 0 || x >= pm.width() as i32 || y >= pm.height() as i32 {
        return None;
    }
    let i = (y as usize * pm.width() as usize + x as usize) * 4;
    let d = pm.data();
    let a = d[i + 3];
    let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
    Some(Rgba::new(un(d[i + 2]), un(d[i + 1]), un(d[i]), a))
}

/// Copies `src` into `dst` at `(x, y)` (straight copy, no blending), clipped.
pub fn blit(dst: &mut PixmapMut, src: &Pixmap, x: i32, y: i32) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + sw).min(dw);
    let y1 = (y + sh).min(dh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_bytes = ((x1 - x0) * 4) as usize;
    let s = src.data();
    let d = dst.data_mut();
    for yy in y0..y1 {
        let si = (((yy - y) * sw + (x0 - x)) * 4) as usize;
        let di = ((yy * dw + x0) * 4) as usize;
        d[di..di + row_bytes].copy_from_slice(&s[si..si + row_bytes]);
    }
}

/// Draws `src` over `dst` at `(x, y)` with normal alpha blending.
pub fn blit_over(dst: &mut PixmapMut, src: &Pixmap, x: i32, y: i32) {
    dst.draw_pixmap(x, y, src.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
}

/// Copies a sub-rectangle of `src` into a new pixmap. Returns `None` for an empty rect.
pub fn crop(src: &Pixmap, r: Rect) -> Option<Pixmap> {
    let r = r.intersect(Rect::new(0, 0, src.width() as i32, src.height() as i32))?;
    let mut out = Pixmap::new(r.w as u32, r.h as u32)?;
    blit(&mut out.as_mut(), src, -r.x, -r.y);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_swaps_into_bgra_memory() {
        let mut pm = Pixmap::new(1, 1).unwrap();
        pm.fill(rgba(10, 20, 30, 255));
        assert_eq!(pm.data(), &[30, 20, 10, 255]);
        assert_eq!(pixel_at(&pm, 0, 0), Some(Rgba::rgb(10, 20, 30)));
    }

    #[test]
    fn crop_and_blit() {
        let mut pm = Pixmap::new(4, 4).unwrap();
        pm.fill(rgba(0, 0, 0, 255));
        let mut red = Pixmap::new(2, 2).unwrap();
        red.fill(rgba(255, 0, 0, 255));
        blit(&mut pm.as_mut(), &red, 1, 1);
        let c = crop(&pm, Rect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(pixel_at(&c, 0, 0), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(pixel_at(&pm, 0, 0), Some(Rgba::rgb(0, 0, 0)));
        assert!(crop(&pm, Rect::new(10, 10, 2, 2)).is_none());
    }
}

/// Copies `src_rect` of `src` into `dst` at `(dx, dy)` (straight copy), clipped to both.
pub fn blit_rect(dst: &mut PixmapMut, src: &Pixmap, src_rect: Rect, dx: i32, dy: i32) {
    let Some(sr) = src_rect.intersect(Rect::new(0, 0, src.width() as i32, src.height() as i32)) else { return };
    let (dx, dy) = (dx + (sr.x - src_rect.x), dy + (sr.y - src_rect.y));
    let Some(dr) = Rect::new(dx, dy, sr.w, sr.h).intersect(Rect::new(0, 0, dst.width() as i32, dst.height() as i32)) else { return };
    let (sx, sy) = (sr.x + (dr.x - dx), sr.y + (dr.y - dy));
    let sw = src.width() as usize;
    let dw = dst.width() as usize;
    let row = dr.w as usize * 4;
    let s = src.data();
    let d = dst.data_mut();
    for i in 0..dr.h as usize {
        let si = ((sy as usize + i) * sw + sx as usize) * 4;
        let di = ((dr.y as usize + i) * dw + dr.x as usize) * 4;
        d[di..di + row].copy_from_slice(&s[si..si + row]);
    }
}

/// Copies a sub-rectangle out of raw BGRA bytes (e.g. a `PixmapMut`'s buffer).
pub fn crop_bytes(data: &[u8], w: u32, h: u32, r: Rect) -> Option<Pixmap> {
    let r = r.intersect(Rect::new(0, 0, w as i32, h as i32))?;
    let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
    for y in r.y..r.bottom() {
        let i = (y as usize * w as usize + r.x as usize) * 4;
        out.extend_from_slice(&data[i..i + r.w as usize * 4]);
    }
    Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(r.w as u32, r.h as u32)?)
}

#[cfg(test)]
mod blit_rect_tests {
    use super::*;

    #[test]
    fn blit_rect_clips_both_sides() {
        let mut src = Pixmap::new(4, 4).unwrap();
        src.fill(rgba(9, 9, 9, 255));
        let mut dst = Pixmap::new(4, 4).unwrap();
        blit_rect(&mut dst.as_mut(), &src, Rect::new(-2, -2, 4, 4), 3, 3);
        // src (-2..2) clipped to (0..2) lands at dst (5..7) -> off-canvas; nothing written.
        assert!(dst.data().iter().all(|&b| b == 0));
        blit_rect(&mut dst.as_mut(), &src, Rect::new(1, 1, 2, 2), 0, 0);
        assert_eq!(pixel_at(&dst, 1, 1), Some(Rgba::rgb(9, 9, 9)));
        assert_eq!(pixel_at(&dst, 2, 2).unwrap().a, 0);
        let c = crop_bytes(src.data(), 4, 4, Rect::new(3, 3, 5, 5)).unwrap();
        assert_eq!((c.width(), c.height()), (1, 1));
    }
}
