//! Scrolling capture (opt-in; off by default).
//!
//! The cursor is parked over the selected area (and put back there before every
//! scroll, so a nudged mouse doesn't matter) and mouse-wheel notches are sent with
//! `SendInput`. After each scroll the area is captured and stitched onto the
//! result. It stops when the content stops moving, a size limit is reached, or the
//! user presses Esc: Esc is registered with `RegisterHotKey` on the capture thread
//! only while it runs (no keyboard polling, no hooks). If another app holds Esc, a
//! large deliberate mouse move stops it instead.
//!
//! If nothing moves, it tries scrolling up (chats open at the newest message), then
//! UI Automation's Scroll pattern on the element under the pointer (apps that ignore
//! injected wheel input), then other spots in the selection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use snapit_gfx::tiny_skia::{IntSize, Pixmap};
use snapit_gfx::Rect;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationScrollPattern, ScrollAmount_NoAmount,
    ScrollAmount_SmallDecrement, ScrollAmount_SmallIncrement, UIA_ScrollPatternId, UIA_ScrollPatternNoScroll,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0, INPUT_MOUSE, MOD_NOREPEAT, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    VK_ESCAPE,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK,
    RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, PeekMessageW, PostMessageW, SetCursorPos, WindowFromPoint, GA_ROOT,
    MSG, PM_REMOVE, RI_MOUSE_RIGHT_BUTTON_DOWN, WM_APP, WM_HOTKEY,
};

use crate::actions::Action;
use crate::app::{self, Capture};

pub const WM_SCROLL_DONE: u32 = WM_APP + 6;
const MAX_STEPS: usize = 80;
const MAX_HEIGHT: usize = 30_000;

/// Why a scrolling capture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// The content stopped moving: the end was reached.
    End,
    /// The user pressed Esc or right-clicked (or, without Esc, moved the mouse well away).
    User,
    /// Another window came to the front (Alt+Tab, a pop-up, the app closed): scrolling on
    /// would scroll whatever is under the pointer now.
    Interrupted,
    /// A scroll could not be lined up with the previous frame.
    Lost,
    /// The step or height limit was reached.
    Limit,
}

pub struct Done {
    result: Result<(Pixmap, Rect, Stop, usize), String>,
}

pub fn start(r: Rect) {
    if !app::settings().scrolling_capture {
        return;
    }
    // No "starting" notification: a toast slides in over the bottom-right of the screen,
    // inside many selections, and would be stitched into the capture. If it ends early, the
    // dialog afterwards explains why.
    let main = app::main_hwnd().0 as isize;
    STOP_REQUESTED.store(false, Ordering::Relaxed);
    watch_right_click(app::main_hwnd(), true);
    std::thread::spawn(move || {
        // UI Automation scrolling needs COM on this thread (the process keeps the MTA alive).
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let result = run(r);
        if com {
            unsafe { CoUninitialize() };
        }
        let raw = Box::into_raw(Box::new(Done { result: result.map(|(p, stop, steps)| (p, r, stop, steps)) }));
        unsafe {
            if PostMessageW(Some(HWND(main as *mut _)), WM_SCROLL_DONE, WPARAM(0), LPARAM(raw as isize)).is_err() {
                drop(Box::from_raw(raw));
            }
        }
    });
}

/// Debug builds: runs a scrolling capture of screen rect `r` straight away (no overlay, no
/// tasks), logging every attempt and frame comparison, and saves the result to `out`.
#[cfg(debug_assertions)]
pub fn diagnose(r: Rect, out: &std::path::Path) -> Result<String, String> {
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let res = run(r);
    if com {
        unsafe { CoUninitialize() };
    }
    let (image, stop, steps) = res?;
    snapit_gfx::io::save(&image, out, snapit_gfx::io::Format::Png, 100)?;
    Ok(format!("stopped: {stop:?} after {steps} scroll(s); {} x {} saved to {}", image.width(), image.height(), out.display()))
}

pub fn on_done(d: Done) {
    watch_right_click(app::main_hwnd(), false);
    // These go in a dialog, not a tray balloon: Windows often hides balloons, and without
    // them a capture that ended early looks like the feature did nothing.
    let explain = |body: &str| crate::ui::dialogs::info(None, "Snap It: scrolling capture", body);
    match d.result {
        Ok((image, r, stop, steps)) => {
            let s = app::settings();
            let mut tasks = s.tasks_for(Action::CaptureScrolling);
            // A tall stitched image is hard to judge from a "Saved" balloon (if Windows even
            // shows it), so open it in the editor unless this workflow sets its own tasks.
            if !s.workflows.iter().any(|w| w.action == Action::CaptureScrolling.name() && w.tasks.is_some()) {
                tasks.open_editor = true;
            }
            crate::tasks::run(Capture { image, kind: Action::CaptureScrolling, title: "Scrolling".into(), screen_rect: Some(r) }, tasks);
            match stop {
                Stop::Lost => explain(&format!(
                    "Scrolling capture stopped after {steps} scroll(s): the content changed in a way that could not be \
                     lined up with the previous part (an animation, a video, or a page that jumped).\n\nWhat was captured up to there was kept."
                )),
                Stop::Interrupted => explain(&format!(
                    "Scrolling capture stopped after {steps} scroll(s) because another window came to the front.\n\n\
                     What was captured up to there was kept."
                )),
                Stop::End if steps == 0 => explain(
                    "The selected area did not move when scrolled down or up (Snap It tried several spots in it), so only \
                     one screen was captured.\n\nSelect an area inside the part that scrolls, and make sure that window \
                     accepts mouse-wheel scrolling.",
                ),
                _ => {}
            }
        }
        Err(e) => explain(&format!("Scrolling capture failed: {e}")),
    }
}

/// Set when the user asks a running scrolling capture to stop with a right-click.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Watches for a right-click anywhere while a scrolling capture runs, via Raw Input (mouse
/// only, registered for the main window just for the capture's duration: no hooks, no
/// polling, nothing blocked or changed). The clicks arrive as `WM_INPUT` on the main window,
/// which hands them to [`on_raw_input`].
fn watch_right_click(main: HWND, on: bool) {
    let dev = RAWINPUTDEVICE {
        usUsagePage: 0x01, // generic desktop
        usUsage: 0x02,     // mouse
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { main } else { HWND::default() },
    };
    if let Err(e) = unsafe { RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32) } {
        crate::log(&format!("scroll: right-click watch {}: {e}", if on { "unavailable" } else { "removal failed" }));
    }
}

/// `WM_INPUT` on the main window: a right-click stops a running scrolling capture.
pub fn on_raw_input(lp: LPARAM) {
    let mut ri = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lp.0 as *mut _),
            RID_INPUT,
            Some(&mut ri as *mut RAWINPUT as *mut _),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == 0 || n == u32::MAX || ri.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let flags = unsafe { ri.data.mouse.Anonymous.Anonymous.usButtonFlags } as u32;
    if flags & RI_MOUSE_RIGHT_BUTTON_DOWN != 0 {
        STOP_REQUESTED.store(true, Ordering::Relaxed);
    }
}

/// A scrolling capture stops after this long whatever happens (a failsafe).
const MAX_DURATION: Duration = Duration::from_secs(180);

/// Hotkey id for Esc while a scrolling capture runs (registered on the capture thread).
const ESC_HOTKEY: i32 = 0x5C01;
/// Without Esc (another app holds it), moving the mouse this far from its spot stops instead.
const STOP_DISTANCE: i32 = 200;

/// Esc as a stop key for as long as this lives. `RegisterHotKey` with no window posts
/// `WM_HOTKEY` to this thread's queue, which [`EscKey::pressed`] checks between scrolls.
struct EscKey {
    registered: bool,
}

impl EscKey {
    fn register() -> EscKey {
        let registered = unsafe { RegisterHotKey(None, ESC_HOTKEY, MOD_NOREPEAT, VK_ESCAPE.0 as u32) }.is_ok();
        if !registered {
            crate::log("scroll: Esc is taken by another app; a large mouse move stops instead");
        }
        EscKey { registered }
    }

    fn pressed(&self) -> bool {
        let mut msg = MSG::default();
        self.registered && unsafe { PeekMessageW(&mut msg, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE) }.as_bool()
    }
}

impl Drop for EscKey {
    fn drop(&mut self) {
        if self.registered {
            unsafe {
                let _ = UnregisterHotKey(None, ESC_HOTKEY);
            }
        }
    }
}

fn wheel(notches: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: (-120 * notches) as u32, dwFlags: MOUSEEVENTF_WHEEL, time: 0, dwExtraInfo: 0 } },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn cursor() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

/// Most points tried when a scroll moves nothing (the pointer may be over a toolbar, a
/// fixed sidebar or another window rather than the part that scrolls).
const MAX_POINTS: usize = 6;

/// Points to park the pointer at, best first. A hand-drawn selection may also cover a
/// browser's tabs and address bar, a sidebar or other windows, so its centre is not always
/// over the content that scrolls: prefer points over the window that covers most of the
/// selection, nearest the centre first.
fn scroll_points(r: Rect) -> Vec<(i32, i32)> {
    let pts: Vec<(i32, i32)> =
        (0..5).flat_map(|j| (0..5).map(move |i| (r.x + r.w * (1 + 2 * i) / 10, r.y + r.h * (1 + 2 * j) / 10))).collect();
    let roots: Vec<isize> = pts
        .iter()
        .map(|&(x, y)| unsafe {
            let h = WindowFromPoint(POINT { x, y });
            if h.is_invalid() {
                0
            } else {
                GetAncestor(h, GA_ROOT).0 as isize
            }
        })
        .collect();
    order_points(&pts, &roots, r.center())
}

/// Orders sample points: those over the most common top-level window first, then the rest,
/// each group nearest `center` first. The exact centre always leads its group.
fn order_points(pts: &[(i32, i32)], roots: &[isize], center: (i32, i32)) -> Vec<(i32, i32)> {
    let mut counts: Vec<(isize, usize)> = Vec::new();
    for &w in roots.iter().filter(|&&w| w != 0) {
        match counts.iter_mut().find(|(x, _)| *x == w) {
            Some(c) => c.1 += 1,
            None => counts.push((w, 1)),
        }
    }
    let main = counts.iter().max_by_key(|(_, n)| *n).map(|(w, _)| *w);
    let dist = |&(x, y): &(i32, i32)| (x - center.0).pow(2) + (y - center.1).pow(2);
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    idx.sort_by_key(|&i| (Some(roots[i]) != main, dist(&pts[i])));
    let mut out: Vec<(i32, i32)> = idx.into_iter().map(|i| pts[i]).collect();
    out.dedup();
    out
}

/// How a scrolling capture moves the content.
enum Scroller {
    /// Mouse-wheel notches with `SendInput` (most apps).
    Wheel,
    /// UI Automation's Scroll pattern, for apps that ignore injected wheel input (some WinUI /
    /// packaged apps, such as WhatsApp). The same opt-in setting covers both.
    Uia(UiaScroller),
}

struct UiaScroller {
    pattern: IUIAutomationScrollPattern,
}

/// Milliseconds UI Automation may wait on an app before giving up (a hung app must not stall
/// the capture).
const UIA_TIMEOUT_MS: u32 = 1500;

impl UiaScroller {
    /// The vertically scrollable element under `(x, y)`, or its nearest scrollable ancestor.
    fn at(x: i32, y: i32) -> Option<UiaScroller> {
        unsafe {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            if let Ok(u2) = uia.cast::<IUIAutomation2>() {
                let _ = u2.SetConnectionTimeout(UIA_TIMEOUT_MS);
                let _ = u2.SetTransactionTimeout(UIA_TIMEOUT_MS);
            }
            let walker = uia.ControlViewWalker().ok()?;
            let mut el = uia.ElementFromPoint(POINT { x, y }).ok()?;
            for _ in 0..30 {
                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId) {
                    if p.CurrentVerticallyScrollable().is_ok_and(|b| b.as_bool()) {
                        return Some(UiaScroller { pattern: p });
                    }
                }
                el = walker.GetParentElement(&el).ok()?;
            }
            None
        }
    }

    /// Scrolls about half the visible height down (or up).
    fn step(&self, up: bool) {
        unsafe {
            let p = &self.pattern;
            let cur = p.CurrentVerticalScrollPercent().unwrap_or(UIA_ScrollPatternNoScroll);
            let view = p.CurrentVerticalViewSize().unwrap_or(0.0);
            crate::log(&format!("scroll: UI Automation at {cur:.2}% (view {view:.2}%), scrolling {}", if up { "up" } else { "down" }));
            if cur >= 0.0 && view > 0.0 && view < 100.0 {
                // ScrollPercent spans the scrollable range (content minus view); ViewSize is
                // the view as a share of the content. Half a view, in ScrollPercent units:
                let d = 50.0 * view / (100.0 - view);
                let target = (cur + if up { -d } else { d }).clamp(0.0, 100.0);
                let _ = p.SetScrollPercent(UIA_ScrollPatternNoScroll, target);
            } else {
                let amount = if up { ScrollAmount_SmallDecrement } else { ScrollAmount_SmallIncrement };
                for _ in 0..3 {
                    let _ = p.Scroll(ScrollAmount_NoAmount, amount);
                }
            }
        }
    }
}

/// One way to try scrolling: a spot to park the pointer at, a method and a direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Attempt {
    at: (i32, i32),
    uia: bool,
    up: bool,
}

/// The order to try things in until something moves: at each spot the wheel down, then up
/// (a chat opens at its newest message); at the best spot also UI Automation down and up.
fn attempts(points: &[(i32, i32)]) -> Vec<Attempt> {
    let mut out = Vec::new();
    for (i, &at) in points.iter().enumerate() {
        out.push(Attempt { at, uia: false, up: false });
        out.push(Attempt { at, uia: false, up: true });
        if i == 0 {
            out.push(Attempt { at, uia: true, up: false });
            out.push(Attempt { at, uia: true, up: true });
        }
    }
    out
}

/// Switches to the next attempt from `plan[*next..]` (`None` when all have been tried),
/// parking the pointer there. An attempt whose spot has nothing UI Automation can scroll
/// is skipped.
fn begin(plan: &[Attempt], next: &mut usize, cur: Option<Attempt>) -> Option<(Attempt, Scroller)> {
    while let Some(&a) = plan.get(*next) {
        *next += 1;
        if cur.is_none_or(|c| c.at != a.at) {
            unsafe {
                let _ = SetCursorPos(a.at.0, a.at.1);
            }
            // Moving the pointer can change hover effects; let them settle before the first frame.
            std::thread::sleep(Duration::from_millis(150));
        }
        if !a.uia {
            return Some((a, Scroller::Wheel));
        }
        if let Some(s) = UiaScroller::at(a.at.0, a.at.1) {
            return Some((a, Scroller::Uia(s)));
        }
        crate::log(&format!("scroll: nothing scrollable through UI Automation at {:?}", a.at));
    }
    None
}

/// Returns the stitched image, why it stopped and how many scrolls were stitched.
fn run(r: Rect) -> Result<(Pixmap, Stop, usize), String> {
    let original = cursor();
    let esc = EscKey::register();
    let started = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(250)); // let the overlay disappear
    // The window in front once the overlay has gone: if another one comes to the front, stop.
    let front = unsafe { GetForegroundWindow() };
    let points: Vec<(i32, i32)> = scroll_points(r).into_iter().take(MAX_POINTS).collect();
    let plan = attempts(if points.is_empty() { &[(0, 0)] } else { &points });
    let mut next_attempt = 0;
    // One wheel notch is roughly 50-150 px depending on the app and DPI. Scroll less in small
    // areas so consecutive frames still overlap enough to be lined up.
    let notches = (r.h / 300).clamp(1, 3);
    // Scrolling up: frames are flipped upside down, so the stitcher sees ordinary downward
    // scrolling; the result is flipped back.
    let grab = |up: bool| snapit_capture::capture_rect(r, false).map(|f| if up { flip(&f) } else { f });

    let (mut attempt, mut scroller) = begin(&plan, &mut next_attempt, None).ok_or("nothing to scroll")?;
    crate::log(&format!("scroll: attempt {attempt:?} ({})", if matches!(scroller, Scroller::Uia(_)) { "UI Automation" } else { "wheel" }));
    let mut st = Stitcher::new(grab(attempt.up)?);
    let mut steps = 0;
    let mut stop = Stop::Limit;
    'outer: for _ in 0..MAX_STEPS {
        // Esc stops. Mouse movement doesn't (unless Esc is unavailable and the move is large):
        // the pointer is put back on its spot so the wheel keeps reaching the right place.
        let p = cursor();
        let far = (p.x - attempt.at.0).abs() > STOP_DISTANCE || (p.y - attempt.at.1).abs() > STOP_DISTANCE;
        if esc.pressed() || STOP_REQUESTED.load(Ordering::Relaxed) || (!esc.registered && far) {
            stop = Stop::User;
            break;
        }
        // Failsafes: never keep scrolling something else, never run forever.
        if !front.is_invalid() && unsafe { GetForegroundWindow() } != front {
            stop = Stop::Interrupted;
            break;
        }
        if started.elapsed() > MAX_DURATION {
            break;
        }
        unsafe {
            let _ = SetCursorPos(attempt.at.0, attempt.at.1);
        }
        match &scroller {
            Scroller::Wheel => wheel(if attempt.up { -notches } else { notches }),
            Scroller::Uia(u) => u.step(attempt.up),
        }
        std::thread::sleep(Duration::from_millis(350)); // smooth-scroll animations
        // A frame caught mid-animation or mid-repaint may not line up (or may not have moved
        // yet), so look again once before deciding.
        let mut retried = false;
        loop {
            let frame = grab(attempt.up)?;
            // A right-click during the wait also opened the app's context menu: don't keep
            // a frame that may show it.
            if STOP_REQUESTED.load(Ordering::Relaxed) || esc.pressed() {
                stop = Stop::User;
                break 'outer;
            }
            let pushed = st.push(&frame);
            crate::log(&format!("scroll: step {steps}{}: {pushed:?}", if retried { " (second look)" } else { "" }));
            match pushed {
                Push::Added => break,
                Push::NoMovement if retried && steps == 0 => {
                    // Nothing has scrolled yet: try the next spot / method / direction.
                    let Some((a, s)) = begin(&plan, &mut next_attempt, Some(attempt)) else {
                        stop = Stop::End;
                        break 'outer;
                    };
                    (attempt, scroller) = (a, s);
                    crate::log(&format!("scroll: attempt {attempt:?} ({})", if matches!(scroller, Scroller::Uia(_)) { "UI Automation" } else { "wheel" }));
                    st = Stitcher::new(grab(attempt.up)?);
                    continue 'outer;
                }
                failed if retried => {
                    stop = if failed == Push::NoMovement { Stop::End } else { Stop::Lost };
                    break 'outer;
                }
                _ => {
                    retried = true;
                    std::thread::sleep(Duration::from_millis(400));
                }
            }
        }
        steps += 1;
        if st.height() >= MAX_HEIGHT {
            break;
        }
    }
    unsafe {
        let _ = SetCursorPos(original.x, original.y);
    }
    let image = st.finish().ok_or_else(|| "nothing was captured".to_string())?;
    Ok((if attempt.up { flip(&image) } else { image }, stop, steps))
}

/// The image upside down (rows reversed).
fn flip(p: &Pixmap) -> Pixmap {
    let row = p.width() as usize * 4;
    let mut data = Vec::with_capacity(p.data().len());
    for r in p.data().chunks_exact(row).rev() {
        data.extend_from_slice(r);
    }
    Pixmap::from_vec(data, IntSize::from_wh(p.width(), p.height()).expect("same size")).expect("same size")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    Added,
    /// The content did not move (the end was reached, or the scroll had no effect).
    NoMovement,
    /// The content moved but the new frame could not be lined up with the previous one.
    NoMatch,
}

/// Stitches successive frames of a scrolling area into one tall image.
pub struct Stitcher {
    w: usize,
    h: usize,
    /// Rows of the result so far (BGRA), ending with the latest static footer.
    rows: Vec<u8>,
    prev: Vec<Cells>,
    /// The previous frame's pixels, for [`tolerant_shift`].
    prev_px: Vec<u8>,
    /// Rows just above the footer that came from the bottom edge of the latest frame. That
    /// edge is where floating overlays sit (a browser's link-preview bubble, a chat button),
    /// so the next frame, where those page rows have scrolled up and out from under the
    /// overlay, replaces them. Only the last frame's are kept.
    provisional: usize,
}

/// Minimum rows held back at the bottom of each frame (see [`Stitcher::provisional`]).
fn default_hold(h: usize) -> usize {
    (h / 12).clamp(24, 96).min(h / 3)
}

/// Each row is hashed in this many vertical strips, so one strip that changes on its own
/// (a scrollbar thumb, a caret, a hover highlight) does not spoil the whole row.
const STRIPS: usize = 16;
type Cells = [u64; STRIPS];

/// How well the best strips must agree for a shift to be accepted.
const MATCH: f32 = 0.95;
/// Share of strips the score is taken from. The rest may be anything that does not move with
/// the page: a scrollbar, the edge of a fixed sidebar, a column under a hover highlight.
const BEST_STRIPS: f32 = 0.7;

/// How well `next` rows `top..top + n` match `prev` rows moved up by `dy`: the mean, over the
/// best-agreeing strips, of the share of rows that agree in that strip.
fn shift_score(prev: &[Cells], next: &[Cells], strips: &[usize], top: usize, dy: usize, n: usize) -> f32 {
    let mut agree: Vec<usize> = strips
        .iter()
        .map(|&s| (0..n).filter(|&i| next[top + i][s] == prev[top + dy + i][s]).count())
        .collect();
    agree.sort_unstable_by(|x, y| y.cmp(x));
    let k = ((strips.len() as f32 * BEST_STRIPS).ceil() as usize).max(1);
    agree[..k].iter().sum::<usize>() as f32 / (k * n) as f32
}

fn cell_hashes(pm: &Pixmap) -> Vec<Cells> {
    let w = pm.width() as usize;
    pm.data()
        .chunks_exact(w * 4)
        .map(|row| {
            let mut cells = [0u64; STRIPS];
            for (s, cell) in cells.iter_mut().enumerate() {
                // FNV-1a over the strip, ignoring alpha. Narrow images leave some strips empty.
                let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                for px in row[s * w / STRIPS * 4..(s + 1) * w / STRIPS * 4].as_chunks::<4>().0 {
                    for &b in &px[..3] {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100_0000_01b3);
                    }
                }
                *cell = h;
            }
            cells
        })
        .collect()
}

fn differing(a: &Cells, b: &Cells) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// Brightness of a BGRA pixel (0..=255).
fn luma(px: &[u8]) -> i32 {
    (px[0] as i32 * 29 + px[1] as i32 * 150 + px[2] as i32 * 77) >> 8
}

/// Width of the horizontal runs [`tolerant_shift`] averages.
const RUN: usize = 8;

/// Average brightness of `RUN`-pixel horizontal runs in rows `y0..y1` of a BGRA frame `w` px
/// wide, as (runs, runs per row). Rows stay separate, so shifts are exact to the pixel.
fn run_luma(px: &[u8], w: usize, y0: usize, y1: usize) -> (Vec<i32>, usize) {
    let rw = w / RUN;
    let mut out = Vec::with_capacity(rw * (y1 - y0));
    for y in y0..y1 {
        let row = &px[y * w * 4..(y + 1) * w * 4];
        for rx in 0..rw {
            let sum: i32 = (0..RUN).map(|dx| luma(&row[(rx * RUN + dx) * 4..(rx * RUN + dx) * 4 + 3])).sum();
            out.push(sum / RUN as i32);
        }
    }
    (out, rw)
}

/// Brightness difference (of run averages) treated as "the same". A chat wallpaper's faint
/// doodles differ from each other by less; text against its bubble by far more.
const RUN_TOL: i32 = 18;

/// The shift `dy` at which `next` rows `top..top + mid` line up with `prev` rows moved up by
/// `dy`, comparing brightness with a tolerance instead of exact strips, so content moving
/// over a fixed, low-contrast background (a chat wallpaper) can still be lined up. `None`
/// unless one shift is clearly best: blank or repetitive content (where many shifts fit) is
/// never guessed.
fn tolerant_shift(prev: &[u8], next: &[u8], w: usize, top: usize, mid: usize, min_overlap: usize) -> Option<usize> {
    let (a, rw) = run_luma(prev, w, top, top + mid);
    let (b, _) = run_luma(next, w, top, top + mid);
    if rw == 0 || mid < 8 {
        return None;
    }
    // Columns that stay put (a chat list or sidebar inside the selection) say nothing about
    // the shift and would only add mismatches: compare the moving columns only.
    let cols: Vec<usize> = (0..rw)
        .filter(|&c| (0..mid).filter(|&y| (a[y * rw + c] - b[y * rw + c]).abs() <= RUN_TOL).count() * 100 < mid * 98)
        .collect();
    if cols.is_empty() {
        return None;
    }
    /// How one shift compares: moving parts that line up (`good`) and that don't (`bad`).
    struct Fit {
        dy: usize,
        good: usize,
        bad: usize,
    }
    impl Fit {
        fn rate(&self) -> f32 {
            self.bad as f32 / (self.good + self.bad).max(1) as f32
        }
    }
    let mut fits: Vec<Fit> = Vec::new();
    for dy in 1..mid {
        let n = mid - dy;
        if n < min_overlap {
            break;
        }
        // Every 2nd row of `next` is plenty and halves the work. Parts unchanged at the same
        // place on screen (a notification popping up over the window, a badge in a side list,
        // the fixed wallpaper between messages) say nothing about the shift: skip them.
        let mut fit = Fit { dy, good: 0, bad: 0 };
        for y in (0..n).step_by(2) {
            let (rn, rp) = (&b[y * rw..(y + 1) * rw], &a[(y + dy) * rw..(y + dy + 1) * rw]);
            // ...on either side of the comparison: where the overlay sat in the previous frame,
            // the new frame shows what it was hiding.
            let (prev_here, next_there) = (&a[y * rw..(y + 1) * rw], &b[(y + dy) * rw..(y + dy + 1) * rw]);
            for &c in &cols {
                if (rn[c] - prev_here[c]).abs() <= RUN_TOL || (rp[c] - next_there[c]).abs() <= RUN_TOL {
                    continue;
                }
                if (rn[c] - rp[c]).abs() > RUN_TOL {
                    fit.bad += 1;
                } else {
                    fit.good += 1;
                }
            }
        }
        fits.push(fit);
    }
    // The true shift lines up the most moving content. (A wrong shift can look clean when
    // hardly anything is left to compare, so the share alone is not enough.) A few percent may
    // differ even there: hover buttons that follow the parked pointer, an animated image.
    let plausible = |f: &&Fit| f.rate() <= 0.15;
    let best = fits.iter().filter(|f| f.rate() <= 0.08).max_by_key(|f| f.good)?;
    // The best plausible shift elsewhere (not just a row or two off): repetitive content can
    // line up at two different shifts, and then neither is trusted.
    let rival = fits.iter().filter(plausible).filter(|f| f.dy.abs_diff(best.dy) > 8).map(|f| f.good).max().unwrap_or(0);
    // Enough evidence: at least 2% of the compared area moved and lined up.
    let evidence = best.good * 50 >= (mid / 2).max(1) * cols.len();
    crate::log(&format!(
        "scroll: tolerant match: best dy {} ({} parts agree, {:.1}% differ), runner-up {} parts",
        best.dy,
        best.good,
        best.rate() * 100.0,
        rival
    ));
    (evidence && (rival as f32) <= best.good as f32 * 0.6).then_some(best.dy)
}

impl Stitcher {
    pub fn new(first: Pixmap) -> Stitcher {
        let (w, h) = (first.width() as usize, first.height() as usize);
        Stitcher {
            w,
            h,
            rows: first.data().to_vec(),
            prev: cell_hashes(&first),
            prev_px: first.data().to_vec(),
            provisional: default_hold(h),
        }
    }

    pub fn height(&self) -> usize {
        self.rows.len() / (self.w * 4)
    }

    /// Adds a frame if it can be lined up with the previous one.
    pub fn push(&mut self, next: &Pixmap) -> Push {
        if next.width() as usize != self.w || next.height() as usize != self.h {
            return Push::NoMatch;
        }
        let a = &self.prev;
        let b = cell_hashes(next);
        let h = self.h;
        // Rows unchanged at the same position are static (sticky header/footer). One strip may
        // differ: the scrollbar beside a sticky header moves.
        let still = |y: usize| differing(&a[y], &b[y]) <= 1;
        let top = (0..h).take_while(|&y| still(y)).count();
        if top == h {
            return Push::NoMovement;
        }
        let bottom = (top..h).rev().take_while(|&y| still(y)).count();
        let mid = h - top - bottom;
        // Strips that stay put in the middle (a fixed sidebar, blank margins) say nothing about
        // the shift; compare only the others.
        let strips: Vec<usize> = (0..STRIPS)
            .filter(|&s| (top..top + mid).filter(|&y| a[y][s] == b[y][s]).count() * 100 < mid * 98)
            .collect();
        if strips.is_empty() {
            // Only a few scattered rows changed (a blinking caret, say): nothing scrolled.
            return Push::NoMovement;
        }
        // Find the shift dy at which next's middle best matches prev's middle moved up by dy.
        // Require a real overlap so a few blank rows cannot "match" a huge jump.
        let min_overlap = (mid / 6).max(8).min(mid.saturating_sub(1)).max(1);
        let mut best = (0.0f32, 0usize);
        for dy in 1..mid {
            let n = mid - dy;
            if n < min_overlap {
                break;
            }
            let score = shift_score(a, &b, &strips, top, dy, n);
            if score > best.0 {
                best = (score, dy);
                if score >= 0.999 {
                    break;
                }
            }
        }
        crate::log(&format!("scroll: frame: static top {top}, bottom {bottom}, moving {mid}, {} of {STRIPS} strips move, best exact match dy {} ({:.1}%)", strips.len(), best.1, best.0 * 100.0));
        let dy = if best.0 >= MATCH {
            best.1
        } else {
            // The exact strips disagree everywhere. That happens when the content moves over a
            // background that stays put (a chat wallpaper): try a brightness comparison that
            // tolerates the faint background. If that finds nothing clear either, the page
            // jumped or is animating: stop rather than produce a broken image.
            match tolerant_shift(&self.prev_px, next.data(), self.w, top, mid, min_overlap) {
                Some(dy) => dy,
                None => {
                    // If only a small part of the frame changed, nothing scrolled: something in
                    // view is animating (a GIF, a hover button over a message) while the content
                    // is at its end. Saying so lets the caller try the other direction.
                    let changed: usize = (0..h).map(|y| differing(&a[y], &b[y])).sum();
                    return if changed * 100 < h * STRIPS * 10 { Push::NoMovement } else { Push::NoMatch };
                }
            }
        };
        let row = self.w * 4;
        let start = top + mid - dy;
        // The previous frame's provisional rows are now `retake` rows above the newly
        // revealed ones in this frame, clear of anything floating at the bottom edge.
        // (At least almost a whole step: the first frame's hold was a guess made before the
        // step size was known. Any rows this frame still shows can be taken from it.)
        let retake = self.provisional.max(dy.saturating_sub(8)).min(start - top).min(self.height().saturating_sub(bottom));
        // Drop the static footer and the provisional rows, then add them back from this frame
        // together with the newly revealed rows.
        let keep = self.height() - bottom - retake;
        self.rows.truncate(keep * row);
        self.rows.extend_from_slice(&next.data()[(start - retake) * row..(top + mid) * row]);
        // Re-append the static footer from the latest frame.
        self.rows.extend_from_slice(&next.data()[(h - bottom) * row..h * row]);
        // Hold back this frame's bottom edge in turn: at least the default band, more when
        // part of each row there stays put on screen while the page scrolls (an overlay).
        let pinned = (top..top + mid).rev().take_while(|&y| strips.iter().any(|&s| a[y][s] == b[y][s])).count();
        // Holding back almost a whole step costs nothing (those rows come back from the next
        // frame) and clears anything floating at the bottom that is shorter than one step,
        // such as a notification that pops up over the window.
        self.provisional = default_hold(h).max(pinned + 4).max(dy.saturating_sub(8)).min(mid / 2);
        self.prev = b;
        self.prev_px.clear();
        self.prev_px.extend_from_slice(next.data());
        Push::Added
    }

    pub fn finish(self) -> Option<Pixmap> {
        let h = self.height() as u32;
        Pixmap::from_vec(self.rows, IntSize::from_wh(self.w as u32, h)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall "page" where every row has a distinct colour.
    fn page(h: usize) -> Vec<[u8; 4]> {
        (0..h).map(|y| [(y % 251) as u8, (y * 7 % 253) as u8, (y / 251) as u8, 255]).collect()
    }

    /// A viewport of the page at `offset`, with an optional sticky header/footer.
    fn view(p: &[[u8; 4]], offset: usize, w: usize, h: usize, header: usize, footer: usize) -> Pixmap {
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let c = if y < header {
                [200, 10, 10, 255]
            } else if y >= h - footer {
                [10, 200, 10, 255]
            } else {
                p[offset + y]
            };
            for _ in 0..w {
                data.extend_from_slice(&c);
            }
        }
        Pixmap::from_vec(data, IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap()
    }

    #[test]
    fn stitches_with_sticky_header_and_footer() {
        let p = page(1000);
        let (w, h, hd, ft) = (6, 100, 10, 8);
        let mut st = Stitcher::new(view(&p, 0, w, h, hd, ft));
        for off in [30, 60, 90, 120] {
            assert_eq!(st.push(&view(&p, off, w, h, hd, ft)), Push::Added);
        }
        assert_eq!(st.push(&view(&p, 120, w, h, hd, ft)), Push::NoMovement, "no movement ends the capture");
        let out = st.finish().unwrap();
        // header + content rows 10..(120+92) + footer
        assert_eq!(out.height() as usize, hd + (120 + h - ft - hd) + ft);
        let row = |y: usize| &out.data()[y * w * 4..y * w * 4 + 4];
        assert_eq!(row(0), &[200, 10, 10, 255]);
        assert_eq!(row(hd + 50), &p[hd + 50]);
        assert_eq!(row(hd + 150), &p[hd + 150]);
        assert_eq!(row(out.height() as usize - 1), &[10, 200, 10, 255]);
    }

    /// A wider viewport of the page: a fixed sidebar on the left, the page in the middle and
    /// a scrollbar on the right whose thumb moves with the offset. `blink` paints a small
    /// caret-like spot that is not part of the page.
    fn app_view(p: &[[u8; 4]], offset: usize, h: usize, page_len: usize, blink: bool) -> Pixmap {
        let (side, body, bar) = (20, 80, 4);
        let w = side + body + bar;
        let thumb_h = h * h / page_len;
        let thumb_y = offset * h / page_len;
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let c = if x < side {
                    [40, 40, (y % 7 * 30) as u8, 255] // sidebar: fixed, not blank
                } else if x >= side + body {
                    if (thumb_y..thumb_y + thumb_h).contains(&y) { [90, 90, 90, 255] } else { [230, 230, 230, 255] }
                } else if blink && (50..62).contains(&y) && (side + 10..side + 12).contains(&x) {
                    [0, 0, 0, 255]
                } else {
                    let r = p[offset + y];
                    [r[0], r[1].wrapping_add(x as u8), r[2], 255] // vary across the row
                };
                data.extend_from_slice(&c);
            }
        }
        Pixmap::from_vec(data, IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap()
    }

    #[test]
    fn tolerates_a_moving_scrollbar_a_sidebar_and_a_caret() {
        let len = 2000;
        let p = page(len);
        let h = 300;
        let mut st = Stitcher::new(app_view(&p, 0, h, len, false));
        for (i, off) in [80, 160, 240, 320, 400].into_iter().enumerate() {
            assert_eq!(st.push(&app_view(&p, off, h, len, i % 2 == 0)), Push::Added, "offset {off}");
        }
        let out = st.finish().unwrap();
        assert_eq!(out.height() as usize, 400 + h);
        // The page column of the result is the page itself, row for row.
        let w = out.width() as usize;
        for y in [0usize, 123, 399, 650] {
            let px = &out.data()[(y * w + 20) * 4..(y * w + 20) * 4 + 4];
            assert_eq!(px, &[p[y][0], p[y][1].wrapping_add(20), p[y][2], 255], "row {y}");
        }
    }

    /// A viewport with a floating box in the bottom-left corner (like a browser's link
    /// preview), drawn on top of the page. `tag` changes the box's text from frame to frame.
    fn overlay_view(p: &[[u8; 4]], offset: usize, w: usize, h: usize, tag: u8) -> Pixmap {
        let (bw, bh) = (w / 4, 14);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let c = if y >= h - bh && x < bw {
                    [250, 250, tag.wrapping_add(x as u8), 255]
                } else {
                    let r = p[offset + y];
                    [r[0], r[1].wrapping_add(x as u8), r[2], 255]
                };
                data.extend_from_slice(&c);
            }
        }
        Pixmap::from_vec(data, IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap()
    }

    #[test]
    fn a_floating_overlay_at_the_bottom_is_not_repeated() {
        let p = page(2000);
        let (w, h, last) = (64, 200, 400);
        for changing in [false, true] {
            let tag = |i: u8| if changing { i * 40 } else { 0 };
            let mut st = Stitcher::new(overlay_view(&p, 0, w, h, tag(0)));
            for (i, off) in [80, 160, 240, 320, last].into_iter().enumerate() {
                assert_eq!(st.push(&overlay_view(&p, off, w, h, tag(i as u8 + 1))), Push::Added, "offset {off}");
            }
            let out = st.finish().unwrap();
            assert_eq!(out.height() as usize, last + h);
            // Every row is the page itself under the box, except the last frame's own box.
            for (y, want) in p.iter().enumerate().take(last + h - 14) {
                let px = &out.data()[y * w * 4..y * w * 4 + 4];
                assert_eq!(px, want, "row {y} (box text changing: {changing})");
            }
        }
    }

    #[test]
    fn scrolling_up_stitches_through_flipped_frames() {
        // A chat that opens at its newest message (offset 600) and is scrolled up.
        let p = page(1000);
        let (w, h, hd, ft) = (6, 100, 10, 8);
        let mut st = Stitcher::new(flip(&view(&p, 600, w, h, hd, ft)));
        for off in [570, 540, 510] {
            assert_eq!(st.push(&flip(&view(&p, off, w, h, hd, ft))), Push::Added, "offset {off}");
        }
        let out = flip(&st.finish().unwrap());
        let oh = out.height() as usize;
        // header + content rows (510+hd)..(600+h-ft) + footer, in page order.
        assert_eq!(oh, hd + (600 + h - ft) - (510 + hd) + ft);
        let row = |y: usize| &out.data()[y * w * 4..y * w * 4 + 4];
        assert_eq!(row(0), &[200, 10, 10, 255], "header stays on top");
        assert_eq!(row(hd), &p[510 + hd], "oldest content first");
        assert_eq!(row(oh - ft - 1), &p[600 + h - ft - 1], "newest content last");
        assert_eq!(row(oh - 1), &[10, 200, 10, 255], "footer stays at the bottom");
    }

    /// A chat: message bubbles (dark, with bright "text") on alternating sides scroll over a
    /// fixed wallpaper with a faint pattern that depends only on the screen position.
    fn bubble(py: usize, x: usize) -> Option<u8> {
        let side_left = (py / 40).is_multiple_of(2);
        let inside = py % 40 < 28 && if side_left { (4..36).contains(&x) } else { (28..60).contains(&x) };
        inside.then(|| if (x * 7 + py * 13) % 11 < 4 { 230 } else { 60 })
    }

    fn chat_view(offset: usize, w: usize, h: usize) -> Pixmap {
        chat_with_list(offset, w, h, 0)
    }

    /// `chat_view` with a fixed, high-contrast list `list` px wide on the left (like the chat
    /// list beside a WhatsApp conversation) that doesn't scroll with the messages.
    fn chat_with_list(offset: usize, w: usize, h: usize, list: usize) -> Pixmap {
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let v = if x < list {
                    if (x + y * 3) % 7 < 2 { 240 } else { 40 }
                } else {
                    let cx = x - list;
                    bubble(offset + y, cx).unwrap_or(20 + ((x * 31 + y * 17) % 9) as u8)
                };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Pixmap::from_vec(data, IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap()
    }

    #[test]
    fn a_fixed_side_list_does_not_spoil_the_wallpaper_match() {
        let (list, w, h) = (16, 80, 160);
        let mut st = Stitcher::new(chat_with_list(0, w, h, list));
        for off in [50, 100, 150] {
            assert_eq!(st.push(&chat_with_list(off, w, h, list)), Push::Added, "offset {off}");
        }
        let out = st.finish().unwrap();
        for y in 0..150 + h {
            for x in list..w {
                if let Some(v) = bubble(y, x - list) {
                    assert_eq!(out.data()[(y * w + x) * 4], v, "row {y}, column {x}");
                }
            }
        }
    }

    #[test]
    fn messages_over_a_fixed_wallpaper_line_up() {
        let (w, h) = (64, 160);
        let mut st = Stitcher::new(chat_view(0, w, h));
        for off in [50, 100, 150, 200] {
            assert_eq!(st.push(&chat_view(off, w, h)), Push::Added, "offset {off}");
        }
        let out = st.finish().unwrap();
        assert_eq!(out.height() as usize, 200 + h);
        // Every message pixel is where it belongs in the conversation.
        for y in 0..200 + h {
            for x in 0..w {
                if let Some(v) = bubble(y, x) {
                    let i = (y * w + x) * 4;
                    assert_eq!(out.data()[i], v, "row {y}, column {x}");
                }
            }
        }
    }

    #[test]
    fn a_notification_over_the_chat_is_neither_matched_nor_repeated() {
        let (w, h) = (64, 160);
        let toast = |x: usize, y: usize| (40..64).contains(&x) && (h - 36..h - 4).contains(&y);
        let framed = |off: usize| {
            let mut p = chat_view(off, w, h);
            for y in 0..h {
                for x in 0..w {
                    if toast(x, y) {
                        let i = (y * w + x) * 4;
                        let v = if (x + y) % 5 < 2 { 250 } else { 120 };
                        p.data_mut()[i..i + 3].copy_from_slice(&[v, 40, 40]);
                    }
                }
            }
            p
        };
        let mut st = Stitcher::new(framed(0));
        for off in [50, 100, 150] {
            assert_eq!(st.push(&framed(off)), Push::Added, "offset {off}");
        }
        let out = st.finish().unwrap();
        let oh = out.height() as usize;
        assert_eq!(oh, 150 + h);
        // Above the last screen: only the conversation, no notification remnants.
        for y in 0..oh - h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                if let Some(v) = bubble(y, x) {
                    assert_eq!(out.data()[i], v, "row {y}, column {x}");
                }
                assert_ne!(out.data()[i + 1..i + 3], [40, 40], "notification stitched in at row {y}");
            }
        }
    }

    #[test]
    fn a_small_animation_at_the_end_counts_as_no_movement() {
        // The chat is at its end (nothing scrolls) but a small image in view animates: that
        // must read as "no movement" (so the other direction is tried), not as a failure.
        let (w, h) = (64, 160);
        let base = chat_view(400, w, h);
        let mut animated = base.clone();
        for y in 60..90 {
            for x in 6..16 {
                let i = (y * w + x) * 4;
                animated.data_mut()[i..i + 3].copy_from_slice(&[(x * 37 + y * 11) as u8, 90, 200]);
            }
        }
        let mut st = Stitcher::new(base);
        assert_eq!(st.push(&animated), Push::NoMovement);
    }

    #[test]
    fn tolerant_matching_refuses_repeating_content() {
        // Identical message blocks every 40 rows: shifts of 40, 80… fit equally well.
        let (w, h) = (64, 160);
        let frame = |off: usize| {
            let mut data = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let py = (off + y) % 40;
                    let v = if (4..60).contains(&x) && py < 28 && (x * 7 + py * 13) % 11 < 4 { 230 } else { 30 };
                    data.extend_from_slice(&[v, v, v, 255]);
                }
            }
            data
        };
        // Moved by 50, which lines up at 10, 50 and 90 alike: refuse to guess.
        assert_eq!(tolerant_shift(&frame(0), &frame(50), w, 0, h, 26), None);
    }

    #[test]
    fn the_shorter_last_scroll_at_the_end_of_a_chat_lines_up() {
        // The last scroll hits the end of the conversation and moves only 30 rows.
        let (w, h) = (64, 160);
        let mut st = Stitcher::new(chat_view(0, w, h));
        for off in [50, 100, 130] {
            assert_eq!(st.push(&chat_view(off, w, h)), Push::Added, "offset {off}");
        }
        assert_eq!(st.push(&chat_view(130, w, h)), Push::NoMovement, "the end");
        let out = st.finish().unwrap();
        assert_eq!(out.height() as usize, 130 + h);
        for y in 0..130 + h {
            for x in 0..w {
                if let Some(v) = bubble(y, x) {
                    assert_eq!(out.data()[(y * w + x) * 4], v, "row {y}, column {x}");
                }
            }
        }
    }

    #[test]
    fn tolerant_matching_refuses_featureless_content() {
        // A plain area with only the faint wallpaper: any shift "fits", so none is chosen.
        let (w, h) = (64, 160);
        let flat = |_: usize| {
            let mut data = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let v = 20 + ((x * 31 + y * 17) % 9) as u8;
                    data.extend_from_slice(&[v, v, v, 255]);
                }
            }
            data
        };
        assert_eq!(tolerant_shift(&flat(0), &flat(1), w, 0, h, 26), None);
    }

    #[test]
    fn attempts_try_wheel_both_ways_then_ui_automation_then_other_spots() {
        let plan = attempts(&[(1, 1), (2, 2)]);
        let a = |at, uia, up| Attempt { at, uia, up };
        assert_eq!(
            plan,
            vec![
                a((1, 1), false, false),
                a((1, 1), false, true),
                a((1, 1), true, false),
                a((1, 1), true, true),
                a((2, 2), false, false),
                a((2, 2), false, true),
            ]
        );
    }

    #[test]
    fn scroll_points_prefer_the_window_covering_most_of_the_selection() {
        // A 3-wide row of samples: the centre is over window 7 (say, a browser toolbar's
        // own window or another app), the other four are over window 9.
        let pts = [(0, 0), (10, 0), (20, 0), (30, 0), (40, 0)];
        let roots = [9, 9, 7, 9, 9];
        let out = order_points(&pts, &roots, (20, 0));
        assert_eq!(&out[..2], &[(10, 0), (30, 0)], "nearest points over the main window first");
        assert_eq!(out.last(), Some(&(20, 0)), "the off-window centre is tried last");
        // All over one window: the centre leads.
        assert_eq!(order_points(&pts, &[5; 5], (20, 0))[0], (20, 0));
        // Points over no window (0) never decide which window is "main".
        assert_eq!(order_points(&pts, &[0, 0, 0, 4, 0], (20, 0))[0], (30, 0));
    }

    #[test]
    fn a_frame_that_does_not_line_up_is_rejected() {
        let p = page(3000);
        let mut st = Stitcher::new(view(&p, 0, 6, 100, 0, 0));
        // Jumped much further than the frame height: no overlap.
        assert_eq!(st.push(&view(&p, 1500, 6, 100, 0, 0)), Push::NoMatch);
        assert_eq!(st.height(), 100, "a rejected frame adds nothing");
    }
}
