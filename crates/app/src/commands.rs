//! Runs queued [`Command`]s on the UI thread.

use snapit_capture as cap;
use snapit_gfx::Rect;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetTimer};

use crate::actions::Action;
use crate::app::{self, Capture, Command, Finish};

pub const TIMER_DELAY: usize = 10;

pub fn dispatch(cmd: Command) {
    match cmd {
        Command::Run(a) => run_action(a, true),
        Command::RunNow(a) => run_action(a, false),
        Command::Finish(c, how) => crate::tasks::finish(c, how),
        Command::OpenEditor { image, path } => crate::ui::editor::open(image, path),
        Command::OpenEditorWith { image, items } => crate::ui::editor::open_with(image, None, items),
        Command::EditorInsert { id, image } => crate::ui::editor::insert(id, image),
        Command::Pin { image, at } => crate::ui::pin::open(image, at),
        Command::StartScrolling(r) => crate::scroll::start(r),
        Command::StartRecording(kind, r) => crate::record_ui::start(kind, r),
        Command::StopRecording(cancel) => crate::record_ui::stop(cancel),
        Command::Ocr(img) => crate::tools::ocr(img),
        Command::ScanQr(img) => crate::tools::scan_qr(img),
        Command::Notify { title, body } => crate::tray::notify(app::main_hwnd(), &title, &body),
        Command::OpenLastCapture => open_last(),
        Command::ReloadSettings => {
            let failed = crate::reregister_hotkeys();
            if !failed.is_empty() {
                notify("Some hotkeys could not be registered", &failed.join(", "));
            }
        }
        Command::OpenWelcome => crate::ui::welcome::open(),
        Command::OverlayClosed => app::with(|s| s.overlay_open = false),
    }
}

fn notify(title: &str, body: &str) {
    app::post(Command::Notify { title: title.into(), body: body.into() });
}

fn open_last() {
    let (path, img) = app::with(|s| (s.last_saved.clone(), s.last_image.clone()));
    if let Some(p) = path.filter(|p| p.exists()) {
        match snapit_gfx::io::load(&p) {
            Ok(image) => return app::post(Command::OpenEditor { image, path: Some(p) }),
            Err(e) => crate::log(&e),
        }
    }
    if let Some(img) = img {
        app::post(Command::OpenEditor { image: (*img).clone(), path: None });
    }
}

fn cursor() -> (i32, i32) {
    let mut p = Default::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn grab(r: Rect, kind: Action, title: String) {
    let s = app::settings();
    let Some(r) = r.intersect(cap::virtual_screen()) else {
        return notify("Nothing to capture", "That area is off-screen.");
    };
    match cap::capture_rect(r, s.capture_cursor) {
        Ok(image) => app::post(Command::Finish(Capture { image, kind, title, screen_rect: Some(r) }, Finish::Tasks)),
        Err(e) => notify("Capture failed", &e),
    }
}

fn is_delayable(a: Action) -> bool {
    matches!(
        a,
        Action::CaptureActiveWindow | Action::CaptureMonitor | Action::CaptureFullscreen | Action::CaptureLastRegion | Action::CaptureCustomRegion
    )
}

pub fn run_action(a: Action, allow_delay: bool) {
    let s = app::settings();
    if a.uses_overlay() && app::with(|st| st.overlay_open) {
        return; // one overlay at a time
    }
    if allow_delay && is_delayable(a) && s.capture_delay_ms > 0 {
        app::with(|st| st.pending_delayed = Some(a));
        unsafe {
            SetTimer(Some(app::main_hwnd()), TIMER_DELAY, s.capture_delay_ms, None);
        }
        return;
    }
    use crate::ui::overlay::{self, Mode};
    match a {
        Action::CaptureFullscreen => grab(cap::virtual_screen(), a, "Screen".into()),
        Action::CaptureMonitor => {
            let (x, y) = cursor();
            let mons = cap::monitors();
            if let Some(m) = cap::monitor_at(&mons, x, y) {
                grab(m.rect, a, "Monitor".into());
            }
        }
        Action::CaptureActiveWindow => match cap::foreground_window() {
            Some((hwnd, r)) => grab(r, a, crate::ui::window_title(hwnd)),
            None => notify("No active window", "Click a window first, then try again."),
        },
        Action::CaptureLastRegion => match s.last_region {
            Some(r) => grab(r, a, "Region".into()),
            None => overlay::open(Mode::Region),
        },
        Action::CaptureCustomRegion => match s.custom_region {
            Some(r) => grab(r, a, "Region".into()),
            None => overlay::open(Mode::SetCustomRegion),
        },
        Action::CaptureRegion => overlay::open(Mode::Region),
        Action::CaptureWindow => overlay::open(Mode::Window),
        Action::CaptureScrolling => {
            // A tray balloon is easy to miss (and Windows often hides it), so ask directly.
            if !s.scrolling_capture {
                let ok = crate::ui::dialogs::confirm(
                    "Scrolling capture is off.\n\nIt scrolls the area you select by sending mouse-wheel input to that window. \
                     Turn it on now? (You can change this later in Settings > Capture.)",
                );
                if !ok {
                    return;
                }
                app::update_settings(|s| s.scrolling_capture = true);
            }
            overlay::open(Mode::Scrolling)
        }
        Action::RecordMp4 => crate::record_ui::toggle(crate::record_ui::Kind::Mp4),
        Action::RecordGif => crate::record_ui::toggle(crate::record_ui::Kind::Gif),
        Action::Ocr => overlay::open(Mode::Ocr),
        Action::ScanQr => overlay::open(Mode::ScanQr),
        Action::ColorPicker => overlay::open(Mode::ColorPicker),
        Action::Ruler => overlay::open(Mode::Ruler),
        Action::MakeQr => crate::ui::qr_dialog::open(),
        Action::OpenEditor => crate::ui::editor::open_blank(),
        Action::OpenHistory => crate::ui::history::open(),
        Action::OpenFolder => crate::shell::open_folder(&s.save_dir()),
        Action::OpenSettings => crate::ui::settings_window::open(),
        Action::ReportBug => crate::bugreport::report(),
        Action::Exit => crate::request_exit(),
    }
}
