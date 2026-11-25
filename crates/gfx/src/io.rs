//! Conversions between our BGRA premultiplied pixmaps and straight-RGBA images,
//! plus encode/decode for the formats Snap It saves and opens.

use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use tiny_skia::{IntSize, Pixmap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    Jpg,
    Webp,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Webp => "webp",
        }
    }
    pub fn from_path(p: &Path) -> Option<Format> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpg),
            "webp" => Some(Format::Webp),
            _ => None,
        }
    }
}

/// Straight RGBA bytes (row-major, no padding). Used for the clipboard and encoders.
pub fn to_rgba(pm: &Pixmap) -> RgbaImage {
    let mut out = Vec::with_capacity(pm.data().len());
    for px in pm.data().as_chunks::<4>().0 {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        let un = |c: u8| {
            if a == 255 || a == 0 {
                c
            } else {
                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
            }
        };
        out.extend_from_slice(&[un(r), un(g), un(b), a]);
    }
    RgbaImage::from_raw(pm.width(), pm.height(), out).expect("size matches")
}

/// From straight RGBA bytes into a BGRA premultiplied pixmap.
pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<Pixmap> {
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut data = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        let pm = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        data.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
    }
    Pixmap::from_vec(data, IntSize::from_wh(w, h)?)
}

/// From BGRA bytes that GDI produced (alpha may be garbage, so it is forced opaque).
pub fn from_bgra_opaque(w: u32, h: u32, mut bgra: Vec<u8>) -> Option<Pixmap> {
    for px in bgra.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Pixmap::from_vec(bgra, IntSize::from_wh(w, h)?)
}

pub fn encode(pm: &Pixmap, fmt: Format, jpeg_quality: u8) -> Result<Vec<u8>, String> {
    let img = to_rgba(pm);
    let mut buf = Cursor::new(Vec::new());
    match fmt {
        Format::Png => img.write_to(&mut buf, ImageFormat::Png).map_err(|e| e.to_string())?,
        Format::Webp => img.write_to(&mut buf, ImageFormat::WebP).map_err(|e| e.to_string())?,
        Format::Jpg => {
            // JPEG has no alpha: flatten onto white.
            let rgb = image::DynamicImage::ImageRgba8(flatten_on_white(img)).to_rgb8();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, jpeg_quality.clamp(1, 100));
            rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
        }
    }
    Ok(buf.into_inner())
}

fn flatten_on_white(mut img: RgbaImage) -> RgbaImage {
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        if a < 255 {
            for ch in p.0.iter_mut().take(3) {
                *ch = ((*ch as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
            p.0[3] = 255;
        }
    }
    img
}

pub fn save(pm: &Pixmap, path: &Path, fmt: Format, jpeg_quality: u8) -> Result<(), String> {
    let bytes = encode(pm, fmt, jpeg_quality)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn decode(bytes: &[u8]) -> Result<Pixmap, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    from_rgba(img.width(), img.height(), img.as_raw()).ok_or_else(|| "bad image size".into())
}

pub fn load(path: &Path) -> Result<Pixmap, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pixel_at, rgba, Rgba};

    #[test]
    fn png_round_trip_keeps_colour_and_alpha() {
        let mut pm = Pixmap::new(3, 2).unwrap();
        pm.fill(rgba(200, 100, 50, 255));
        let bytes = encode(&pm, Format::Png, 90).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(pixel_at(&back, 2, 1), Some(Rgba::rgb(200, 100, 50)));

        let semi = from_rgba(1, 1, &[255, 0, 0, 128]).unwrap();
        let back = decode(&encode(&semi, Format::Png, 90).unwrap()).unwrap();
        let p = pixel_at(&back, 0, 0).unwrap();
        assert_eq!(p.a, 128);
        assert!(p.r >= 253);
    }

    #[test]
    fn jpg_and_webp_encode() {
        let mut pm = Pixmap::new(8, 8).unwrap();
        pm.fill(rgba(0, 128, 255, 255));
        assert!(encode(&pm, Format::Jpg, 90).unwrap().starts_with(&[0xFF, 0xD8]));
        assert!(&encode(&pm, Format::Webp, 90).unwrap()[..4] == b"RIFF");
    }
}
