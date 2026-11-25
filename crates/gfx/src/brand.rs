//! The Snap It icon, drawn in code: a rounded gradient tile with viewfinder corners.

use tiny_skia::{FillRule, GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode, Stroke, Transform};

use crate::effects::rounded_rect;
use crate::Rgba;

pub fn icon(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let s = size as f32;
    if let (Some(tile), Some(grad)) = (
        rounded_rect(s * 0.04, s * 0.04, s * 0.92, s * 0.92, s * 0.22),
        LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(s, s),
            vec![GradientStop::new(0.0, Rgba::rgb(56, 189, 248).color()), GradientStop::new(1.0, Rgba::rgb(99, 102, 241).color())],
            SpreadMode::Pad,
            Transform::identity(),
        ),
    ) {
        let paint = Paint { shader: grad, anti_alias: true, ..Default::default() };
        pm.fill_path(&tile, &paint, FillRule::Winding, Transform::identity(), None);
    }
    let (a, b, l) = (s * 0.26, s * 0.74, s * 0.16);
    let mut pb = PathBuilder::new();
    for (x, y, dx, dy) in [(a, a, 1.0, 1.0), (b, a, -1.0, 1.0), (a, b, 1.0, -1.0), (b, b, -1.0, -1.0)] {
        pb.move_to(x + dx * l, y);
        pb.line_to(x, y);
        pb.line_to(x, y + dy * l);
    }
    if let Some(p) = pb.finish() {
        let stroke = Stroke { width: (s * 0.08).max(1.5), line_cap: LineCap::Round, ..Default::default() };
        pm.stroke_path(&p, &Rgba::WHITE.paint(), &stroke, Transform::identity(), None);
    }
    if let Some(dot) = PathBuilder::from_circle(s / 2.0, s / 2.0, s * 0.08) {
        pm.fill_path(&dot, &Rgba::WHITE.paint(), FillRule::Winding, Transform::identity(), None);
    }
    pm
}

/// A multi-resolution .ico file (PNG-compressed entries, supported since Windows Vista).
pub fn ico(sizes: &[u32]) -> Result<Vec<u8>, String> {
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| crate::io::encode(&icon(s), crate::io::Format::Png, 100)).collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, png) in sizes.iter().zip(&pngs) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        out.extend_from_slice(&png);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ico_header() {
        let b = super::ico(&[16, 32, 256]).unwrap();
        assert_eq!(&b[..6], &[0, 0, 1, 0, 3, 0]);
        assert_eq!(b[6 + 32], 0); // 256 encodes as 0
    }
}
