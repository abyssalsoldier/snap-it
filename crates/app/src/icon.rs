//! Runtime HICONs for the tray and windows, rendered from the shared brand art.

use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

/// Builds an HICON (straight-alpha BGRA) at `size` pixels.
pub fn hicon(size: u32) -> Option<HICON> {
    let pm = snapit_gfx::brand::icon(size);
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
            let a = s[3] as u32;
            let un = |c: u8| (c as u32 * 255).checked_div(a).unwrap_or(0).min(255) as u8;
            d.copy_from_slice(&[un(s[0]), un(s[1]), un(s[2]), s[3]]);
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
