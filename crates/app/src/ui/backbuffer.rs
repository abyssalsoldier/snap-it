//! A GDI DIB section whose memory tiny-skia draws into directly, so presenting a
//! frame is one `BitBlt` with no pixel conversion (see the BGRA note in `snapit-gfx`).

use snapit_gfx::tiny_skia::PixmapMut;
use snapit_gfx::Rect;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};

pub struct Backbuffer {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: u32,
    pub h: u32,
}

impl Backbuffer {
    pub fn new(w: u32, h: u32) -> Option<Backbuffer> {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bmp.into());
            Some(Backbuffer { dc, bmp, old, bits: bits as *mut u8, w, h })
        }
    }

    /// Recreates the buffer if the size changed. Contents are not preserved.
    pub fn ensure(slot: &mut Option<Backbuffer>, w: u32, h: u32) {
        if slot.as_ref().is_none_or(|b| b.w != w.max(1) || b.h != h.max(1)) {
            *slot = Backbuffer::new(w, h);
        }
    }

    pub fn pixmap(&mut self) -> PixmapMut<'_> {
        let len = self.w as usize * self.h as usize * 4;
        let data = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        PixmapMut::from_bytes(data, self.w, self.h).expect("valid size")
    }

    /// A copy of the current contents (opaque).
    pub fn to_pixmap(&self) -> Option<snapit_gfx::tiny_skia::Pixmap> {
        let len = self.w as usize * self.h as usize * 4;
        let data = unsafe { std::slice::from_raw_parts(self.bits, len) }.to_vec();
        snapit_gfx::io::from_bgra_opaque(self.w, self.h, data)
    }

    pub fn present(&self, hdc: HDC, r: Rect) {
        unsafe {
            let _ = BitBlt(hdc, r.x, r.y, r.w, r.h, Some(self.dc), r.x, r.y, SRCCOPY);
        }
    }
}

impl Drop for Backbuffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Handles WM_PAINT by blitting the dirty part of `bb`.
pub fn paint(hwnd: windows::Win32::Foundation::HWND, bb: Option<&Backbuffer>) {
    use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if let Some(bb) = bb {
            let r = ps.rcPaint;
            bb.present(hdc, Rect::from_ltrb(r.left, r.top, r.right, r.bottom));
        }
        let _ = EndPaint(hwnd, &ps);
    }
}
