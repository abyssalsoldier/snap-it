// Release builds are GUI-subsystem (no console); debug builds keep a console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod clipboard;
mod commands;
mod history;
mod hotkeys;
mod icon;
mod instance;
mod integration;
mod naming;
mod paths;
mod record_ui;
mod scroll;
#[cfg(debug_assertions)]
mod selftest;
mod settings;
mod shell;
mod sound;
mod tasks;
mod tools;
mod tray;
mod ui;

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;

use windows::core::{w, Result, HSTRING};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW,
    KillTimer, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SendMessageW,
    TranslateMessage, CW_USEDEFAULT, HICON, MSG, WINDOW_EX_STYLE, WM_COMMAND, WM_CONTEXTMENU,
    WM_COPYDATA, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_INPUT, WM_LBUTTONDBLCLK, WM_QUERYENDSESSION,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

use actions::Action;
use app::Command;

const MAIN_CLASS: &str = "SnapItMainWindow";
const COPYDATA_ARGS: usize = 0x534E_4150; // "SNAP"
const HOTKEY_BASE: u16 = 0x100;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn log(msg: &str) {
    #[cfg(debug_assertions)]
    eprintln!("[snapit] {msg}");
    #[cfg(not(debug_assertions))]
    let _ = msg;
}

pub fn request_exit() {
    let hwnd = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    if !hwnd.is_invalid() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

struct Icons(isize, isize);
static ICONS: OnceLock<Icons> = OnceLock::new();

fn icons() -> &'static Icons {
    ICONS.get_or_init(|| {
        let big = icon::hicon(32).map(|h| h.0 as isize).unwrap_or(0);
        let small = icon::hicon(16).map(|h| h.0 as isize).unwrap_or(0);
        Icons(big, small)
    })
}

pub fn app_icon() -> HICON {
    HICON(icons().0 as *mut _)
}

pub fn app_icon_small() -> HICON {
    HICON(icons().1 as *mut _)
}

/// Creates the hidden top-level window that owns the tray icon and receives WM_HOTKEY.
pub fn create_main_window() -> Result<HWND> {
    let hinstance = unsafe { GetModuleHandleW(None)? };
    let class_name = HSTRING::from(MAIN_CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
        hIcon: app_icon(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(windows::core::Error::from_win32());
    }
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), &class_name, w!("Snap It"), WS_OVERLAPPED, CW_USEDEFAULT, CW_USEDEFAULT, 0, 0, None, None, Some(hinstance.into()), None)?
    };
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    Ok(hwnd)
}

/// Unregisters every global hotkey until [`reregister_hotkeys`] (used while recording a
/// hotkey in Settings, so pressing it doesn't trigger it).
pub fn suspend_hotkeys() {
    let hwnd = app::main_hwnd();
    let old = app::with(|s| std::mem::take(&mut s.bindings));
    hotkeys::unregister_all(hwnd, &old);
}

/// (Re)registers hotkeys from the current workflows. Returns chords that failed.
pub fn reregister_hotkeys() -> Vec<String> {
    let hwnd = app::main_hwnd();
    let old = app::with(|s| std::mem::take(&mut s.bindings));
    hotkeys::unregister_all(hwnd, &old);
    let s = app::settings();
    let mut bindings = Vec::new();
    let mut bad = Vec::new();
    for (i, wf) in s.workflows.iter().enumerate() {
        match wf.resolve() {
            Some((chord, _)) => bindings.push((HOTKEY_BASE + i as u16, chord)),
            None => bad.push(format!("{} (not understood)", wf.hotkey)),
        }
    }
    let failed = hotkeys::register_all(hwnd, &bindings);
    bad.extend(failed.iter().map(|c| c.display()));
    bindings.retain(|(_, c)| !failed.contains(c));
    app::with(|st| st.bindings = bindings);
    bad
}

fn action_for_hotkey(id: u16) -> Option<Action> {
    let i = id.checked_sub(HOTKEY_BASE)? as usize;
    app::settings().workflows.get(i)?.resolve().map(|(_, a)| a)
}

/// Start in the tray without opening Settings (used by the Start-with-Windows shortcut).
pub const BACKGROUND_ARG: &str = "--background";

const USAGE: &str = "snapit [--capture <action>] [--edit <image>] [--pin <image>] [--background] [--welcome] [--quit]\n\
actions: region, window, active_window, monitor, fullscreen, last_region, custom_region, scrolling,\n\
         record_mp4, record_gif, ocr, scan_qr, color_picker, ruler, open_editor, open_history, open_settings";

/// Handles command-line arguments (from this launch or forwarded by a second launch).
fn handle_args(args: &[String]) {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--capture" | "-c" => {
                let Some(name) = it.next() else { break };
                let act = Action::from_name(name).or_else(|| Action::from_name(&format!("capture_{name}")));
                match act {
                    Some(act) => app::post(Command::Run(act)),
                    None => log(&format!("unknown action '{name}'\n{USAGE}")),
                }
            }
            "--edit" | "--pin" => {
                let Some(path) = it.next() else { break };
                let p = std::path::PathBuf::from(path);
                match snapit_gfx::io::load(&p) {
                    Ok(image) if a == "--edit" => app::post(Command::OpenEditor { image, path: Some(p) }),
                    Ok(image) => app::post(Command::Pin { image, at: None }),
                    Err(e) => app::post(Command::Notify { title: "Could not open image".into(), body: e }),
                }
            }
            "--welcome" => app::post(Command::OpenWelcome),
            BACKGROUND_ARG => {}
            "--help" | "-h" | "/?" => log(USAGE),
            "--version" | "-V" => log(concat!("Snap It ", env!("CARGO_PKG_VERSION"))),
            other => log(&format!("ignoring argument '{other}'")),
        }
    }
}

/// If Snap It is already running, hand our arguments to it and return true.
fn forward_to_running(args: &[String]) -> bool {
    let Ok(hwnd) = (unsafe { FindWindowW(&HSTRING::from(MAIN_CLASS), None) }) else { return false };
    let joined: Vec<u16> = args.join("\0").encode_utf16().collect();
    let cds = COPYDATASTRUCT { dwData: COPYDATA_ARGS, cbData: (joined.len() * 2) as u32, lpData: joined.as_ptr() as *mut _ };
    unsafe {
        SendMessageW(hwnd, WM_COPYDATA, Some(WPARAM(0)), Some(LPARAM(&cds as *const _ as isize)));
    }
    true
}

fn main() -> Result<()> {
    // Only load DLLs from System32 (blocks DLL planting next to the exe).
    unsafe {
        let _ = windows::Win32::System::LibraryLoader::SetDefaultDllDirectories(windows::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        // Release builds have no console, so show the version in a message box there.
        #[cfg(debug_assertions)]
        println!("Snap It {}", env!("CARGO_PKG_VERSION"));
        #[cfg(not(debug_assertions))]
        ui::dialogs::info(None, "Snap It", concat!("Snap It ", env!("CARGO_PKG_VERSION")));
        return Ok(());
    }

    #[cfg(debug_assertions)]
    if args.first().map(String::as_str) == Some("--selftest") {
        let dir = args.get(1).map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("snapit-selftest"));
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        snapit_record::ensure_mta();
        std::process::exit(selftest::run(&dir));
    }

    // Debug builds: `--scroll-diag x y w h out.png` runs a logged scrolling capture of that
    // screen rectangle (for diagnosing apps where it fails).
    #[cfg(debug_assertions)]
    if args.first().map(String::as_str) == Some("--scroll-diag") {
        let n: Vec<i32> = args.iter().skip(1).take(4).filter_map(|a| a.parse().ok()).collect();
        let out = args.get(5).map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("snapit-scroll-diag.png"));
        if n.len() != 4 {
            eprintln!("usage: snapit --scroll-diag x y w h [out.png]");
            std::process::exit(2);
        }
        snapit_record::ensure_mta();
        match scroll::diagnose(snapit_gfx::Rect::new(n[0], n[1], n[2], n[3]), &out) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed: {e}"),
        }
        return Ok(());
    }

    // `--quit` (used by the installers): signal a running copy, never start one.
    if args.iter().any(|a| a == "--quit") {
        instance::signal_quit();
        return Ok(());
    }

    // Single instance. A second launch with arguments forwards them; without arguments it
    // either reminds the user that Snap It is running or (newer/different copy) takes over.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\SnapIt.SingleInstance"))? };
    let mut updated_from = None;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !args.is_empty() {
            forward_to_running(&args);
            return Ok(());
        }
        let me = env!("CARGO_PKG_VERSION");
        let take_over = match instance::decide_now() {
            instance::SecondLaunch::Activate => false,
            instance::SecondLaunch::TakeOver { updated_from: v } => {
                updated_from = Some(v);
                true
            }
            instance::SecondLaunch::Ask { running } => {
                let which = match &running {
                    Some(r) => format!("Snap It {} is already running from:\n{}", r.version, r.exe),
                    None => "Another copy of Snap It is already running.".to_string(),
                };
                ui::dialogs::confirm(&format!("{which}\n\nClose it and run this copy (version {me}) instead?"))
            }
        };
        if !take_over {
            instance::signal_activate();
            return Ok(());
        }
        if !instance::take_over(mutex, 8000) {
            ui::dialogs::info(None, "Snap It", "The running copy of Snap It did not close. Exit it from its tray icon, then start Snap It again.");
            return Ok(());
        }
    }

    // Checked before anything saves settings: no settings file yet means first launch.
    let first_run = ui::welcome::is_first_run();

    // COM (apartment-threaded) for the shell link, file and folder dialogs.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    // Worker threads (OCR, recording) use the MTA; keep it alive for the whole process.
    snapit_record::ensure_mta();
    let hwnd = create_main_window()?;
    instance::watch(hwnd);
    instance::publish();

    app::init(app::State {
        settings: std::rc::Rc::new(settings::Settings::load()),
        main_hwnd: hwnd,
        overlay_open: false,
        last_saved: None,
        last_image: None,
        bindings: Vec::new(),
        pending_delayed: None,
        pending_click: None,
    });

    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);
    tray::add(hwnd, app_icon_small())?;

    let failed = reregister_hotkeys();
    if !failed.is_empty() {
        log(&format!("hotkeys unavailable: {}", failed.join(", ")));
        tray::notify(
            hwnd,
            "Some hotkeys are in use by another app",
            &format!("{}\nChange them in Settings, or close the other app.", failed.join(", ")),
        );
    }
    log("running; right-click the tray icon for the menu");
    if let Some(old) = updated_from {
        app::post(Command::Notify { title: format!("Snap It updated to {}", env!("CARGO_PKG_VERSION")), body: format!("The running copy ({old}) was closed and replaced.") });
    }
    // Started by the user (Start menu, desktop, installer): show a window, not just a tray icon.
    // Started at sign-in (`--background`) or to run a command: stay in the tray.
    // The first start, and the first start after an update that added setup steps (usually
    // from the installer's "Launch Snap It"), open setup instead of Settings. Started at
    // sign-in, a notification offers it instead of popping a window up.
    let setup_due = ui::welcome::is_due(first_run, &app::settings());
    if args.is_empty() {
        app::post(if setup_due { Command::OpenWelcome } else { Command::Run(Action::OpenSettings) });
    } else if setup_due && args.iter().any(|a| a == BACKGROUND_ARG) {
        app::with(|s| s.pending_click = Some(app::Click::OpenWelcome));
        app::post(Command::Notify { title: "Snap It has new features".into(), body: "Click here to set them up.".into() });
    }
    integration::upgrade_startup_link();

    if app::settings().integration.update_check {
        integration::check_for_updates(false);
    }
    handle_args(&args);
    drain();

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            drain();
        }
    }

    let bindings = app::with(|s| std::mem::take(&mut s.bindings));
    hotkeys::unregister_all(hwnd, &bindings);
    Ok(())
}

pub fn drain() {
    while let Some(cmd) = app::next_command() {
        commands::dispatch(cmd);
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if let Some(action) = action_for_hotkey(wparam.0 as u16) {
                app::post(Command::Run(action));
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(action) = Action::from_id((wparam.0 & 0xFFFF) as u16) {
                app::post(Command::Run(action));
            }
            LRESULT(0)
        }
        tray::WM_TRAY => {
            match lparam.0 as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    if let Err(e) = tray::show_menu(hwnd) {
                        log(&format!("tray menu failed: {e}"));
                    }
                }
                WM_LBUTTONDBLCLK => app::post(Command::Run(Action::CaptureRegion)),
                tray::NIN_BALLOONUSERCLICK => match app::with(|s| s.pending_click.take()) {
                    Some(app::Click::Url(url)) => shell::open_url(&url),
                    Some(app::Click::ShowFile(p)) => shell::show_in_folder(&p),
                    Some(app::Click::OpenWelcome) => app::post(Command::OpenWelcome),
                    None => app::post(Command::OpenLastCapture),
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == commands::TIMER_DELAY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), commands::TIMER_DELAY);
            }
            if let Some(a) = app::with(|s| s.pending_delayed.take()) {
                app::post(Command::RunNow(a));
            }
            LRESULT(0)
        }
        // Raw mouse input, registered only while a scrolling capture runs (right-click stops it).
        // DefWindowProc must still see it to release the input data.
        WM_INPUT => {
            scroll::on_raw_input(lparam);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        scroll::WM_SCROLL_DONE => {
            let r = unsafe { Box::from_raw(lparam.0 as *mut scroll::Done) };
            scroll::on_done(*r);
            LRESULT(0)
        }
        record_ui::WM_RECORD_DONE => {
            let r = unsafe { Box::from_raw(lparam.0 as *mut record_ui::Done) };
            record_ui::on_done(*r);
            LRESULT(0)
        }
        tools::WM_TOOL_RESULT => {
            let r = unsafe { Box::from_raw(lparam.0 as *mut tools::ToolResult) };
            tools::on_result(*r);
            LRESULT(0)
        }
        integration::WM_UPDATE_SKIP => {
            let tag = unsafe { Box::from_raw(lparam.0 as *mut String) };
            integration::on_skip_version(*tag);
            LRESULT(0)
        }
        integration::WM_UPDATE_RESULT => {
            let r = unsafe { Box::from_raw(lparam.0 as *mut integration::UpdateResult) };
            integration::on_update_result(*r);
            LRESULT(0)
        }
        tasks::WM_SAVED => {
            let r = unsafe { Box::from_raw(lparam.0 as *mut tasks::SaveResult) };
            tasks::on_saved(*r);
            LRESULT(0)
        }
        // Sign-out, shutdown, or an installer's Restart Manager closing apps that hold its
        // files: agree, then exit straight away. Ignoring WM_ENDSESSION made Windows wait for
        // its hung-app timeout (about 100 s in the MSI upgrade) before terminating Snap It.
        WM_QUERYENDSESSION => LRESULT(1),
        // Windows switched between light and dark ("ImmersiveColorSet"): re-theme every window.
        windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE => {
            if lparam.0 != 0 {
                let s = unsafe { windows::core::PCWSTR(lparam.0 as *const u16).to_string() }.unwrap_or_default();
                if s == "ImmersiveColorSet" {
                    ui::broadcast_theme();
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                request_exit();
            }
            LRESULT(0)
        }
        instance::WM_APP_QUIT => {
            request_exit();
            LRESULT(0)
        }
        instance::WM_APP_ACTIVATE => {
            // Someone started Snap It again while it was already running: show its window.
            app::post(Command::Run(Action::OpenSettings));
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            if cds.dwData == COPYDATA_ARGS && !cds.lpData.is_null() {
                let words = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
                let s = String::from_utf16_lossy(words);
                let args: Vec<String> = s.split('\0').map(str::to_owned).collect();
                handle_args(&args);
            }
            LRESULT(1)
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted, so put the tray icon back.
            let _ = tray::add(hwnd, app_icon_small());
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
