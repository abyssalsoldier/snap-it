//! The after-capture task chain: sound, copy, save, history, notify, editor, pin.
//! Encoding and writing the file happen on a worker thread so the UI never stalls.

use std::path::PathBuf;
use std::rc::Rc;

use snapit_gfx::io;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::{self, Capture, Command, Finish};
use crate::history::{self, Entry};
use crate::naming::{self, Stamp, Vars};
use crate::settings::Tasks;

/// Posted by the save worker; `lparam` is a `Box<SaveResult>`.
pub const WM_SAVED: u32 = WM_APP + 2;

pub struct SaveResult {
    pub result: Result<PathBuf, String>,
    pub width: u32,
    pub height: u32,
    pub kind: &'static str,
    pub copied: bool,
    pub notify: bool,
}

pub fn finish(cap: Capture, how: Finish) {
    let s = app::settings();
    let tasks = match how {
        Finish::Tasks => s.tasks_for(cap.kind),
        Finish::Copy => Tasks { copy: true, save: false, open_editor: false, pin: false, notify: false, sound: false },
        Finish::Save => Tasks { copy: false, save: true, open_editor: false, pin: false, notify: true, sound: false },
        Finish::Pin => Tasks { copy: false, save: false, open_editor: false, pin: true, notify: false, sound: false },
        Finish::Editor => Tasks { copy: false, save: false, open_editor: true, pin: false, notify: false, sound: false },
    };
    run(cap, tasks);
}

pub fn run(cap: Capture, tasks: Tasks) {
    let s = app::settings();
    if tasks.sound {
        crate::sound::shutter();
    }
    let mut copied = false;
    if tasks.copy {
        match crate::clipboard::set_image(&cap.image) {
            Ok(()) => copied = true,
            Err(e) => crate::log(&format!("clipboard: {e}")),
        }
    }
    let shared = Rc::new(cap.image.clone());
    app::with(|st| st.last_image = Some(shared));

    if tasks.save {
        let name = naming::expand(
            &s.filename_pattern,
            &Vars { stamp: Stamp::now(), width: cap.image.width(), height: cap.image.height(), title: &cap.title },
        );
        let dir = s.save_dir();
        let fmt = s.format;
        let q = s.jpeg_quality;
        let image = cap.image.clone();
        let kind = cap.kind.name();
        let notify = tasks.notify;
        let hwnd = app::main_hwnd().0 as isize;
        std::thread::spawn(move || {
            let path = naming::unique_path(&dir, &name, fmt.ext());
            let result = io::save(&image, &path, fmt, q).map(|_| path);
            let msg = Box::new(SaveResult { result, width: image.width(), height: image.height(), kind, copied, notify });
            unsafe {
                let raw = Box::into_raw(msg);
                let h = windows::Win32::Foundation::HWND(hwnd as *mut _);
                if PostMessageW(Some(h), WM_SAVED, WPARAM(0), LPARAM(raw as isize)).is_err() {
                    drop(Box::from_raw(raw));
                }
            }
        });
    } else if tasks.notify && copied {
        app::post(Command::Notify { title: "Copied to clipboard".into(), body: format!("{} × {}", cap.image.width(), cap.image.height()) });
    }

    if tasks.open_editor {
        app::post(Command::OpenEditor { image: cap.image.clone(), path: None });
    }
    if tasks.pin {
        app::post(Command::Pin { image: cap.image, at: cap.screen_rect });
    }
}

/// Runs on the UI thread when the save worker reports back.
pub fn on_saved(r: SaveResult) {
    let s = app::settings();
    match r.result {
        Ok(path) => {
            let e = Entry::new(path.clone(), r.width, r.height, r.kind);
            if let Err(err) = history::append(&crate::paths::history_file(), &e) {
                crate::log(&format!("history: {err}"));
            }
            maybe_trim_history(s.history_limit);
            app::with(|st| st.last_saved = Some(path.clone()));
            if r.notify {
                let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let title = if r.copied { "Saved and copied" } else { "Saved" };
                app::post(Command::Notify { title: title.into(), body: format!("{file}\nClick to open in the editor") });
            }
        }
        Err(e) => app::post(Command::Notify { title: "Could not save capture".into(), body: e }),
    }
}

fn maybe_trim_history(limit: usize) {
    let file = crate::paths::history_file();
    // Cheap check: only rewrite when the file has grown well past the limit.
    let lines = std::fs::read_to_string(&file).map(|t| t.lines().count()).unwrap_or(0);
    if lines > limit + 50 {
        let keep = history::load(&file, limit);
        let _ = history::rewrite(&file, &keep);
    }
}
