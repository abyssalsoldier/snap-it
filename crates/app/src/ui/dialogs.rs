//! Standard Windows file dialogs and message boxes.

use std::path::{Path, PathBuf};

use windows::core::{HSTRING, PCWSTR, PWSTR};
use snapit_gfx::Rgba;
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, GetOpenFileNameW, GetSaveFileNameW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
    OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
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

/// A Yes/No question with no owner window. True for Yes.
pub fn confirm(text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{MB_SETFOREGROUND, MB_YESNO};
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("Snap It"), MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND) == IDYES }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateChoice {
    Download,
    Notes,
    Later,
}

/// "Snap It vX is available" (a Windows task dialog). Returns the choice and
/// whether "Don't remind me about this version" was ticked. Blocks the calling
/// thread, so call it off the main thread.
pub fn update_prompt(latest: &str, current: &str, how: &str, notes: &str) -> (UpdateChoice, bool) {
    use windows::Win32::UI::Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_SIZE_TO_CONTENT, TD_INFORMATION_ICON,
    };
    const DOWNLOAD: i32 = 100;
    const NOTES: i32 = 101;
    const LATER: i32 = 102;
    let title = HSTRING::from("Snap It update");
    let main = HSTRING::from(format!("Snap It {} is available", latest.trim_start_matches('v')));
    let content = HSTRING::from(format!("You have version {current}.\n\n{how}"));
    let expanded = HSTRING::from(notes);
    let (show, hide) = (HSTRING::from("Show what's new"), HSTRING::from("Hide what's new"));
    let verify = HSTRING::from("Don't remind me about this version");
    let labels = [HSTRING::from("Download"), HSTRING::from("Release notes"), HSTRING::from("Later")];
    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: DOWNLOAD, pszButtonText: PCWSTR(labels[0].as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: NOTES, pszButtonText: PCWSTR(labels[1].as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: LATER, pszButtonText: PCWSTR(labels[2].as_ptr()) },
    ];
    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_INFORMATION_ICON },
        pszMainInstruction: PCWSTR(main.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: DOWNLOAD,
        pszVerificationText: PCWSTR(verify.as_ptr()),
        pszExpandedInformation: if notes.is_empty() { PCWSTR::null() } else { PCWSTR(expanded.as_ptr()) },
        pszExpandedControlText: PCWSTR(hide.as_ptr()),
        pszCollapsedControlText: PCWSTR(show.as_ptr()),
        ..Default::default()
    };
    let mut pressed = 0i32;
    let mut skip = windows::core::BOOL(0);
    if let Err(e) = unsafe { TaskDialogIndirect(&cfg, Some(&mut pressed), None, Some(&mut skip)) } {
        crate::log(&format!("update prompt: {e}"));
        return (UpdateChoice::Later, false);
    }
    let choice = match pressed {
        DOWNLOAD => UpdateChoice::Download,
        NOTES => UpdateChoice::Notes,
        _ => UpdateChoice::Later,
    };
    (choice, skip.as_bool())
}

thread_local! {
    /// The colour dialog's 16 custom-colour slots, kept for the session.
    static CUSTOM_COLORS: std::cell::RefCell<[COLORREF; 16]> = const { std::cell::RefCell::new([COLORREF(0x00FF_FFFF); 16]) };
}

/// The standard Windows colour picker, starting at `initial`. Keeps alpha.
pub fn choose_color(owner: HWND, initial: Rgba) -> Option<Rgba> {
    CUSTOM_COLORS.with(|cc| {
        let mut cc = cc.borrow_mut();
        let mut c = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: COLORREF(initial.r as u32 | (initial.g as u32) << 8 | (initial.b as u32) << 16),
            lpCustColors: cc.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        unsafe { ChooseColorW(&mut c) }.as_bool().then(|| {
            let v = c.rgbResult.0;
            Rgba::new((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8, if initial.a == 0 { 255 } else { initial.a })
        })
    })
}

pub fn info(owner: Option<HWND>, title: &str, body: &str) {
    unsafe {
        MessageBoxW(owner, &HSTRING::from(body), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION | MESSAGEBOX_STYLE(0x0001_0000)); // MB_SETFOREGROUND: Snap It usually has no window in front
    }
}
