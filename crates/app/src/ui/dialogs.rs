//! Standard Windows file dialogs and message boxes.

use std::path::{Path, PathBuf};

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDCANCEL, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNOCANCEL,
    MESSAGEBOX_STYLE,
};

const IMAGE_FILTER: &str = "Images (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif\0All files\0*.*\0\0";
const SAVE_FILTER: &str = "PNG image\0*.png\0JPEG image\0*.jpg\0WebP image (lossless)\0*.webp\0\0";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn open_image(owner: Option<HWND>) -> Option<PathBuf> {
    let filter = wide(IMAGE_FILTER);
    let mut buf = vec![0u16; 4096];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.unwrap_or_default(),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe { GetOpenFileNameW(&mut ofn) }.as_bool().then(|| path_from(&buf))
}

/// Returns the chosen path with an extension matching the chosen filter.
pub fn save_image(owner: Option<HWND>, suggested: &Path) -> Option<PathBuf> {
    let filter = wide(SAVE_FILTER);
    let mut buf = vec![0u16; 4096];
    let name = wide(&suggested.to_string_lossy());
    buf[..name.len().min(4095)].copy_from_slice(&name[..name.len().min(4095)]);
    let def_ext = wide("png\0");
    let index = match suggested.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        Some(e) if e == "jpg" || e == "jpeg" => 2,
        Some(e) if e == "webp" => 3,
        _ => 1,
    };
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.unwrap_or_default(),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: index,
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrDefExt: PCWSTR(def_ext.as_ptr()),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetSaveFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let mut p = path_from(&buf);
    let want = match ofn.nFilterIndex {
        2 => "jpg",
        3 => "webp",
        _ => "png",
    };
    let has_known = snapit_gfx::io::Format::from_path(&p).is_some();
    if !has_known {
        p.set_extension(want);
    }
    Some(p)
}

fn path_from(buf: &[u16]) -> PathBuf {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

pub fn ask_save(owner: HWND, what: &str) -> Answer {
    let r = unsafe {
        MessageBoxW(Some(owner), &HSTRING::from(format!("Save changes to {what}?")), &HSTRING::from("Snap It"), MB_YESNOCANCEL | MB_ICONQUESTION)
    };
    match r {
        IDYES => Answer::Yes,
        IDCANCEL => Answer::Cancel,
        _ => Answer::No,
    }
}

pub fn info(owner: Option<HWND>, title: &str, body: &str) {
    unsafe {
        MessageBoxW(owner, &HSTRING::from(body), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION | MESSAGEBOX_STYLE(0));
    }
}
