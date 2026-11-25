//! OCR (Windows' built-in `Windows.Media.Ocr` engine, fully on-device) and QR codes.
//! Both run on worker threads and report back through the command queue.

use snapit_gfx::tiny_skia::Pixmap;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::{self, Command};

pub const WM_TOOL_RESULT: u32 = WM_APP + 4;

pub enum ToolResult {
    Ocr(Result<String, String>),
    Qr(Option<String>),
}

fn post_result(r: ToolResult) {
    let hwnd = app::main_hwnd().0 as isize;
    let raw = Box::into_raw(Box::new(r));
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), WM_TOOL_RESULT, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

pub fn ocr(img: Pixmap) {
    let hwnd = app::main_hwnd().0 as isize;
    std::thread::spawn(move || {
        let r = recognize(&img);
        let _ = hwnd;
        post_result(ToolResult::Ocr(r));
    });
}

pub fn scan_qr(img: Pixmap) {
    std::thread::spawn(move || post_result(ToolResult::Qr(decode_qr(&img))));
}

fn preview(s: &str) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 120 {
        format!("{}…", one_line.chars().take(120).collect::<String>())
    } else {
        one_line
    }
}

pub fn on_result(r: ToolResult) {
    match r {
        ToolResult::Ocr(Ok(text)) if text.trim().is_empty() => {
            app::post(Command::Notify { title: "No text found".into(), body: "Try selecting a larger or sharper area.".into() })
        }
        ToolResult::Ocr(Ok(text)) => {
            let _ = crate::clipboard::set_text(&text);
            app::post(Command::Notify { title: "Text copied".into(), body: preview(&text) });
        }
        ToolResult::Ocr(Err(e)) => app::post(Command::Notify { title: "Text recognition failed".into(), body: e }),
        ToolResult::Qr(Some(content)) => {
            let _ = crate::clipboard::set_text(&content);
            let is_link = content.starts_with("https://");
            if is_link {
                app::with(|s| s.pending_click = Some(crate::app::Click::Url(content.clone())));
            }
            let body = if is_link { format!("{}\nCopied. Click to open the link.", preview(&content)) } else { format!("{}\nCopied to the clipboard.", preview(&content)) };
            app::post(Command::Notify { title: "QR code found".into(), body });
        }
        ToolResult::Qr(None) => app::post(Command::Notify { title: "No QR code found".into(), body: "Select the code a little more generously and try again.".into() }),
    }
}

// ---------------- OCR ----------------

fn recognize(img: &Pixmap) -> Result<String, String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let err = |e: windows::core::Error| e.message().to_string();
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| "No OCR language is installed. Add a language with OCR support in Windows Settings > Time & language.".to_string())?;

    // Small text recognises far better when upscaled; very large images must be shrunk.
    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000) as f32;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let k = if h < 120.0 { (3.0f32).min(max / w.max(h)) } else if w.max(h) > max { max / w.max(h) } else { 1.0 };
    let scaled;
    let src = if (k - 1.0).abs() > 0.01 {
        scaled = snapit_gfx::effects::resize(img, (w * k).round() as u32, (h * k).round() as u32).ok_or("resize failed")?;
        &scaled
    } else {
        img
    };

    let writer = DataWriter::new().map_err(err)?;
    writer.WriteBytes(src.data()).map_err(err)?;
    let buffer = writer.DetachBuffer().map_err(err)?;
    let bmp = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, src.width() as i32, src.height() as i32, BitmapAlphaMode::Premultiplied).map_err(err)?;
    let result = engine.RecognizeAsync(&bmp).map_err(err)?.get().map_err(err)?;
    let mut out = Vec::new();
    for line in result.Lines().map_err(err)? {
        out.push(line.Text().map_err(err)?.to_string());
    }
    Ok(out.join("\n"))
}

// ---------------- QR ----------------

fn luma(pm: &Pixmap) -> Vec<u8> {
    pm.data().as_chunks::<4>().0.iter().map(|p| ((p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000) as u8).collect()
}

pub fn decode_qr(pm: &Pixmap) -> Option<String> {
    let attempt = |pm: &Pixmap| -> Option<String> {
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        let g = luma(pm);
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| g[y * w + x]);
        prepared.detect_grids().into_iter().find_map(|grid| grid.decode().ok().map(|(_, s)| s))
    };
    attempt(pm).or_else(|| {
        // Tiny codes: try again at 3x.
        if pm.width().max(pm.height()) < 600 {
            let big = snapit_gfx::effects::resize(pm, pm.width() * 3, pm.height() * 3)?;
            attempt(&big)
        } else {
            None
        }
    })
}

/// Renders `text` as a QR code with a 4-module quiet zone, `module` pixels per module.
pub fn make_qr(text: &str, module: u32) -> Option<Pixmap> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width() as u32;
    let quiet = 4;
    let size = (n + quiet * 2) * module;
    let mut pm = Pixmap::new(size, size)?;
    pm.fill(snapit_gfx::Rgba::WHITE.color());
    let colors = code.to_colors();
    let black = snapit_gfx::Rgba::BLACK.paint();
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i as u32 % n + quiet) * module, (i as u32 / n + quiet) * module);
            if let Some(r) = snapit_gfx::tiny_skia::Rect::from_xywh(x as f32, y as f32, module as f32, module as f32) {
                let mut p = black.clone();
                p.anti_alias = false;
                pm.fill_rect(r, &p, snapit_gfx::tiny_skia::Transform::identity(), None);
            }
        }
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_round_trip() {
        let text = "https://example.com/snap-it?q=1";
        let img = make_qr(text, 4).unwrap();
        assert_eq!(decode_qr(&img).as_deref(), Some(text));
        // Embedded in a larger, noisy-ish canvas.
        let mut canvas = Pixmap::new(img.width() + 80, img.height() + 60).unwrap();
        canvas.fill(snapit_gfx::Rgba::rgb(230, 230, 230).color());
        snapit_gfx::blit(&mut canvas.as_mut(), &img, 40, 30);
        assert_eq!(decode_qr(&canvas).as_deref(), Some(text));
        assert!(decode_qr(&Pixmap::new(50, 50).unwrap()).is_none());
    }

    #[test]
    fn ocr_reads_rendered_text() {
        // Uses the on-device Windows OCR engine; skipped when no OCR language is installed.
        let mut pm = Pixmap::new(420, 80).unwrap();
        pm.fill(snapit_gfx::Rgba::WHITE.color());
        snapit_gfx::text::draw(&mut pm.as_mut(), "Snap It reads text", 12.0, 18.0, 36.0, false, snapit_gfx::Rgba::BLACK, None);
        match recognize(&pm) {
            Ok(t) => assert!(t.to_lowercase().contains("snap"), "got {t:?}"),
            Err(e) => eprintln!("OCR unavailable here: {e}"),
        }
    }
}
