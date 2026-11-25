//! Clipboard access. Writes happen only as a direct result of a user action, and
//! reads only when the user pastes. There is no background clipboard listener.

use std::borrow::Cow;

use snapit_gfx::{io, tiny_skia::Pixmap};

pub fn set_image(pm: &Pixmap) -> Result<(), String> {
    let rgba = io::to_rgba(pm);
    let data = arboard::ImageData {
        width: pm.width() as usize,
        height: pm.height() as usize,
        bytes: Cow::Owned(rgba.into_raw()),
    };
    arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string())
}

pub fn get_image() -> Option<Pixmap> {
    let img = arboard::Clipboard::new().ok()?.get_image().ok()?;
    io::from_rgba(img.width as u32, img.height as u32, &img.bytes)
}

pub fn set_text(s: &str) -> Result<(), String> {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(s.to_owned())).map_err(|e| e.to_string())
}

pub fn get_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}
