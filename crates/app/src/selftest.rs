//! `snapit --selftest <dir>` (debug builds only).
//!
//! Drives the real windows **hidden**, with synthetic mouse/keyboard messages, a
//! fake desktop image instead of a screen capture, and all data redirected into
//! `<dir>`. Nothing is shown, the clipboard is never written, and nothing is
//! captured from the real screen. Each window's rendered frame is saved as a PNG
//! so the UI can be reviewed, and every step reports OK/FAIL.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Transform};
use snapit_gfx::{text, Rect, Rgba};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{self, Capture, Command};
use crate::settings::{Settings, Tasks};
use crate::ui;

struct Report {
    dir: PathBuf,
    failures: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            println!("  OK    {name}");
        } else {
            self.failures += 1;
            println!("  FAIL  {name}  {detail}");
        }
    }
    fn dump(&mut self, h: HWND, name: &str) {
        let path = self.dir.join(format!("{name}.png"));
        let ok = unsafe { SendMessageW(h, ui::WM_SNAPIT_DUMP, Some(WPARAM(0)), Some(LPARAM(Box::into_raw(Box::new(path.clone())) as isize))) }.0 == 1;
        self.check(&format!("render {name}.png"), ok && path.exists(), "no frame");
    }
}

fn find(class: &str) -> Option<HWND> {
    unsafe { FindWindowW(&HSTRING::from(class), None) }.ok().filter(|h| !h.is_invalid())
}

fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))) }.0
}

fn xy(x: i32, y: i32) -> isize {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
}

fn drag(h: HWND, a: (i32, i32), b: (i32, i32)) {
    send(h, WM_MOUSEMOVE, 0, xy(a.0, a.1));
    send(h, WM_LBUTTONDOWN, 1, xy(a.0, a.1));
    for i in 1..=4 {
        let (x, y) = (a.0 + (b.0 - a.0) * i / 4, a.1 + (b.1 - a.1) * i / 4);
        send(h, WM_MOUSEMOVE, 1, xy(x, y));
    }
    send(h, WM_LBUTTONUP, 0, xy(b.0, b.1));
}

fn click(h: HWND, x: i32, y: i32) {
    drag(h, (x, y), (x, y));
}

fn key(h: HWND, vk: u8) {
    send(h, WM_KEYDOWN, vk as usize, 0);
    send(h, WM_KEYUP, vk as usize, 0);
}

fn type_text(h: HWND, s: &str) {
    for c in s.encode_utf16() {
        send(h, WM_CHAR, c as usize, 0);
    }
}

fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut msg = MSG::default();
    loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                crate::drain();
            }
        }
        crate::drain();
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn pump_until(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump(30);
        if done() {
            return true;
        }
    }
    done()
}

/// A plausible fake desktop: wallpaper, two app windows with text, and a QR code.
fn fake_desktop(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    let grad = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(w as f32, h as f32),
        vec![GradientStop::new(0.0, Rgba::rgb(30, 64, 120).color()), GradientStop::new(1.0, Rgba::rgb(120, 60, 140).color())],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    pm.fill_rect(snapit_gfx::tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap(), &Paint { shader: grad, ..Default::default() }, Transform::identity(), None);
    let window = |pm: &mut Pixmap, x: f32, y: f32, ww: f32, wh: f32, title: &str, lines: &[&str]| {
        let id = Transform::identity();
        if let Some(p) = rounded_rect(x, y, ww, wh, 8.0) {
            pm.fill_path(&p, &Rgba::rgb(250, 250, 252).paint(), FillRule::Winding, id, None);
        }
        if let Some(p) = rounded_rect(x, y, ww, 34.0, 8.0) {
            pm.fill_path(&p, &Rgba::rgb(229, 231, 235).paint(), FillRule::Winding, id, None);
        }
        text::draw(&mut pm.as_mut(), title, x + 14.0, y + 8.0, 15.0, true, Rgba::rgb(31, 41, 55), None);
        for (i, l) in lines.iter().enumerate() {
            text::draw(&mut pm.as_mut(), l, x + 18.0, y + 52.0 + i as f32 * 28.0, 17.0, false, Rgba::rgb(55, 65, 81), None);
        }
    };
    window(&mut pm, 140.0, 90.0, 820.0, 560.0, "Quarterly report.docx", &[
        "Invoice #20481 - due 15 December",
        "Revenue grew 18% compared to last quarter.",
        "The new onboarding flow cut support tickets in half.",
        "Next steps: ship the mobile beta and hire two engineers.",
        "",
        "Action items",
        "  1. Review the design mockups",
        "  2. Update the release notes",
        "  3. Schedule the retrospective",
    ]);
    window(&mut pm, 1010.0, 160.0, 460.0, 420.0, "Share", &["Scan to open the demo:"]);
    if let Some(qr) = crate::tools::make_qr("https://example.com/demo", 5) {
        snapit_gfx::blit(&mut pm.as_mut(), &qr, 1100, 250);
    }
    pm
}

pub fn run(dir: &Path) -> i32 {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("create selftest dir");
    // Safe: single-threaded at this point, before any worker threads exist.
    std::env::set_var("SNAPIT_HOME", dir);
    ui::set_headless(true);
    let mut r = Report { dir: dir.to_path_buf(), failures: 0 };
    println!("Snap It self-test -> {}", dir.display());

    let mut s = Settings { tasks: Tasks { copy: false, save: true, open_editor: false, pin: false, notify: false, sound: false }, ..Settings::default() };
    s.region.detect_windows = false; // the fake desktop has no real windows behind it
    s.save().expect("write test settings");

    let hwnd = crate::create_main_window().expect("main window");
    app::init(app::State {
        settings: std::rc::Rc::new(Settings::load()),
        main_hwnd: hwnd,
        overlay_open: false,
        last_saved: None,
        last_image: None,
        bindings: Vec::new(),
        pending_delayed: None,
        pending_click: None,
    });
    r.check("settings load from sandbox", !app::settings().tasks.sound, "");

    let desk = fake_desktop(1600, 900);
    let _ = snapit_gfx::io::save(&desk, &dir.join("00_fake_desktop.png"), snapit_gfx::io::Format::Png, 100);

    // --- Region overlay + quick annotate ---
    crate::ui::overlay::set_test_shot(desk.clone());
    crate::ui::overlay::open(crate::ui::overlay::Mode::Region);
    let Some(ov) = find("SnapItOverlay") else {
        r.check("overlay opens", false, "window not found");
        return 1;
    };
    r.check("overlay opens", true, "");
    send(ov, WM_PAINT, 0, 0);
    send(ov, WM_MOUSEMOVE, 0, xy(205, 156));
    r.dump(ov, "01_overlay_selecting");
    send(ov, WM_LBUTTONDOWN, 1, xy(120, 70));
    send(ov, WM_MOUSEMOVE, 1, xy(700, 420));
    send(ov, WM_MOUSEMOVE, 1, xy(990, 680));
    r.dump(ov, "02_overlay_dragging");
    send(ov, WM_LBUTTONUP, 0, xy(990, 680));

    key(ov, b'A');
    drag(ov, (230, 470), (420, 380));
    key(ov, b'R');
    drag(ov, (150, 128), (470, 172));
    key(ov, b'T');
    click(ov, 600, 470);
    type_text(ov, "Ship it!");
    send(ov, WM_CHAR, 0x0D, 0);
    key(ov, b'N');
    click(ov, 135, 410);
    click(ov, 135, 440);
    key(ov, b'H');
    drag(ov, (155, 168), (520, 190));
    key(ov, b'X');
    drag(ov, (430, 128), (700, 170));
    key(ov, b'K');
    drag(ov, (720, 520), (900, 580));
    type_text(ov, "Looks great");
    send(ov, WM_CHAR, 0x0D, 0);
    key(ov, b'G');
    drag(ov, (600, 280), (940, 300));
    key(ov, b'M');
    drag(ov, (720, 220), (840, 340));
    key(ov, b'V');
    send(ov, WM_MOUSEMOVE, 0, xy(420, 705)); // hover a toolbar button to show its tooltip
    r.dump(ov, "03_overlay_annotated");

    // Undo/redo through the toolbar keyboard path (Ctrl isn't held, so use Delete on a selection).
    click(ov, 470, 150); // select the rectangle outline edge region
    key(ov, 0x2E);
    r.dump(ov, "04_overlay_after_delete");

    // Open the result in the editor with annotations still editable.
    key(ov, b'E');
    pump(100);
    r.check("overlay closes on finish", find("SnapItOverlay").is_none(), "still open");

    // --- Editor ---
    let ed = find("SnapItEditor");
    r.check("editor opens with the annotated region", ed.is_some(), "");
    if let Some(ed) = ed {
        unsafe {
            let _ = SetWindowPos(ed, None, 0, 0, 1300, 820, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        send(ed, WM_PAINT, 0, 0);
        r.dump(ed, "05_editor");
        // Draw something new and save.
        key(ed, b'A');
        send(ed, WM_MOUSEMOVE, 0, xy(500, 400));
        send(ed, WM_PAINT, 0, 0);
        let saved = send(ed, crate::ui::editor::WM_EDITOR_TEST_SAVE, 0, 0) == 1;
        let files = std::fs::read_dir(dir.join("Screenshots")).map(|d| d.count()).unwrap_or(0);
        r.check("editor saves to the screenshots folder", saved && files >= 1, &format!("{files} files"));
        send(ed, WM_CLOSE, 0, 0);
        pump(50);
        r.check("editor closes after saving", find("SnapItEditor").is_none(), "");
    }

    // --- Region capture straight through the task chain (Enter) ---
    crate::ui::overlay::set_test_shot(desk.clone());
    crate::ui::overlay::open(crate::ui::overlay::Mode::Region);
    if let Some(ov) = find("SnapItOverlay") {
        send(ov, WM_PAINT, 0, 0);
        drag(ov, (1000, 150), (1480, 590));
        key(ov, 0x0D);
        let hist = crate::paths::history_file();
        let ok = pump_until(5000, || crate::history::load(&hist, 10).iter().any(|e| e.kind == "capture_region"));
        r.check("Enter runs the task chain (saved + history)", ok, "no history entry");
        let last = app::settings().last_region;
        r.check("last region is remembered", last == Some(Rect::new(1000, 150, 481, 441)), &format!("{last:?}"));
    }

    // --- Instant capture path (as fullscreen/window captures use) ---
    app::post(Command::Finish(Capture { image: desk.clone(), kind: crate::actions::Action::CaptureFullscreen, title: "Screen".into(), screen_rect: None }, app::Finish::Tasks));
    let ok = pump_until(8000, || crate::history::load(&crate::paths::history_file(), 10).iter().any(|e| e.kind == "capture_fullscreen"));
    r.check("instant capture saves in the background", ok, "");

    // --- QR + OCR on regions of the fake desktop (no clipboard involved) ---
    let qr = snapit_gfx::crop(&desk, Rect::new(1080, 230, 230, 230)).unwrap();
    r.check("QR scan of a screen region", crate::tools::decode_qr(&qr).as_deref() == Some("https://example.com/demo"), "");

    // --- History ---
    crate::ui::history::open();
    if let Some(h) = find("SnapItHistory") {
        unsafe {
            let _ = SetWindowPos(h, None, 0, 0, 1000, 640, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        pump(1500); // thumbnails load on a worker thread
        send(h, WM_PAINT, 0, 0);
        r.dump(h, "06_history");
        send(h, WM_CLOSE, 0, 0);
    } else {
        r.check("history opens", false, "");
    }

    // --- Pin ---
    let crop = snapit_gfx::crop(&desk, Rect::new(140, 90, 520, 300)).unwrap();
    crate::ui::pin::open(crop, Some(Rect::new(100, 100, 520, 300)));
    if let Some(p) = find("SnapItPin") {
        send(p, WM_PAINT, 0, 0);
        r.dump(p, "07_pin");
        send(p, WM_MOUSEWHEEL, (120u32 << 16) as usize, xy(300, 200));
        send(p, WM_CLOSE, 0, 0);
    } else {
        r.check("pin opens", false, "");
    }

    // --- QR maker ---
    crate::ui::qr_dialog::open();
    if let Some(q) = find("SnapItQr") {
        send(q, WM_SHOWWINDOW, 1, 0);
        send(q, WM_PAINT, 0, 0);
        r.dump(q, "08_qr_maker");
        send(q, WM_CLOSE, 0, 0);
    }

    // --- Settings: build every control, then Save without changes ---
    let before = (*app::settings()).clone();
    crate::ui::settings_window::open();
    if let Some(sw) = find("SnapItSettings") {
        send(sw, WM_SHOWWINDOW, 1, 0);
        send(sw, WM_COMMAND, crate::ui::settings_window::ID_SAVE as usize, 0);
        pump(50);
        let after = Settings::load();
        let (a, b) = (toml::to_string_pretty(&before).unwrap_or_default(), toml::to_string_pretty(&after).unwrap_or_default());
        let diff: Vec<String> = a.lines().zip(b.lines()).filter(|(x, y)| x != y).map(|(x, y)| format!("{x}  ->  {y}")).collect();
        r.check("settings window round-trips every value", after == before, &diff.join(" | "));
        r.check("settings window closes on Save", find("SnapItSettings").is_none(), "");
    } else {
        r.check("settings opens", false, "");
    }

    let n = std::fs::read_dir(dir.join("Screenshots")).map(|d| d.count()).unwrap_or(0);
    println!("{} screenshots saved, {} failure(s)", n, r.failures);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    (r.failures > 0) as i32
}
