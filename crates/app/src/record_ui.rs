//! Recording control: a small REC pill (timer, Stop, Cancel) and a red frame drawn
//! *outside* the recorded area so it never appears in the video.

use std::cell::RefCell;

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, PathBuilder, Transform};
use snapit_gfx::{text, Rect, Rgba};
use snapit_record::{Output, Recorder, Stats};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, LoadCursorW, PostMessageW, SetCursor, SetTimer, HTCLIENT,
    IDC_HAND, WM_APP, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_SETCURSOR, WM_TIMER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::app::{self, Click, Command};
use crate::ui::{backbuffer, Backbuffer, Handler, Win};

pub const WM_RECORD_DONE: u32 = WM_APP + 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mp4,
    Gif,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Kind::Mp4 => "mp4",
            Kind::Gif => "gif",
        }
    }
    fn history_kind(self) -> &'static str {
        match self {
            Kind::Mp4 => "record_mp4",
            Kind::Gif => "record_gif",
        }
    }
}

struct Active {
    recorder: Recorder,
    kind: Kind,
    windows: Vec<HWND>,
}

pub struct Done {
    pub result: Result<Stats, String>,
    pub kind: Kind,
    pub cancelled: bool,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

pub fn is_recording() -> bool {
    ACTIVE.with(|a| a.borrow().is_some())
}

/// Hotkey / menu entry point: stops a running recording, or starts choosing a region.
pub fn toggle(kind: Kind) {
    if is_recording() {
        stop(false);
    } else {
        crate::ui::overlay::open(crate::ui::overlay::Mode::Record(kind));
    }
}

fn notify(title: &str, body: &str) {
    app::post(Command::Notify { title: title.into(), body: body.into() });
}

pub fn start(kind: Kind, region: Rect) {
    if is_recording() {
        return;
    }
    let s = app::settings();
    let mons = snapit_capture::monitors();
    let (cx, cy) = region.center();
    let Some(mon) = snapit_capture::monitor_at(&mons, cx, cy).cloned() else { return };
    let Some(region) = region.intersect(mon.rect) else { return };

    let name = crate::naming::expand(
        &s.filename_pattern,
        &crate::naming::Vars { stamp: crate::naming::Stamp::now(), width: region.w as u32, height: region.h as u32, title: "Recording" },
    );
    let path = crate::naming::unique_path(&s.save_dir(), &name, kind.ext());
    let r = &s.recording;
    let out = match kind {
        Kind::Mp4 => Output::Mp4 { path, fps: r.mp4_fps, bitrate_kbps: r.mp4_bitrate_kbps },
        Kind::Gif => Output::Gif { path, fps: r.gif_fps, max_seconds: r.gif_max_seconds },
    };
    let recorder = match Recorder::start(mon.handle, mon.rect, region, r.cursor, out) {
        Ok(rec) => rec,
        Err(e) => return notify("Could not start recording", &e),
    };

    let scale = mon.scale();
    let mut windows = Vec::new();
    // Red frame, 3 px outside the recorded rectangle.
    let t = (3.0 * scale).max(2.0) as i32;
    let o = region.inflate(t + 1);
    for edge in [
        Rect::new(o.x, o.y, o.w, t),
        Rect::new(o.x, o.bottom() - t, o.w, t),
        Rect::new(o.x, o.y, t, o.h),
        Rect::new(o.right() - t, o.y, t, o.h),
    ] {
        if let Ok(h) = crate::ui::create("SnapItRecFrame", WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, WS_POPUP, "", edge, None, Box::new(Solid(Rgba::rgb(239, 68, 68)))) {
            crate::ui::show(h, false);
            windows.push(h);
        }
    }
    // Control pill below the region, else above, else inside the bottom edge.
    let (pw, ph) = ((230.0 * scale) as i32, (40.0 * scale) as i32);
    let gap = (10.0 * scale) as i32;
    let px = (region.x + region.w / 2 - pw / 2).clamp(mon.work.x + 4, mon.work.right() - pw - 4);
    let py = if o.bottom() + gap + ph <= mon.work.bottom() {
        o.bottom() + gap
    } else if o.y - gap - ph >= mon.work.y {
        o.y - gap - ph
    } else {
        region.bottom() - ph - gap
    };
    let pill = Pill { scale, kind, bb: None, hover: None };
    if let Ok(h) = crate::ui::create("SnapItRecPill", WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, WS_POPUP, "Snap It recording", Rect::new(px, py, pw, ph), None, Box::new(pill)) {
        crate::ui::show(h, false);
        unsafe {
            SetTimer(Some(h), 1, 250, None);
        }
        windows.push(h);
    }
    ACTIVE.with(|a| *a.borrow_mut() = Some(Active { recorder, kind, windows }));
}

/// Stops (or cancels) the recording. Finalising happens off the UI thread.
pub fn stop(cancel: bool) {
    let Some(active) = ACTIVE.with(|a| a.borrow_mut().take()) else { return };
    for h in &active.windows {
        // Posted so we never destroy a window from inside its own handler.
        unsafe {
            let _ = PostMessageW(Some(*h), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    let kind = active.kind;
    let handle = active.recorder.stop();
    let main = app::main_hwnd().0 as isize;
    std::thread::spawn(move || {
        let result = handle.join().unwrap_or_else(|_| Err("recording thread crashed".into()));
        if cancel {
            if let Ok(st) = &result {
                let _ = std::fs::remove_file(&st.path);
            }
        }
        let raw = Box::into_raw(Box::new(Done { result, kind, cancelled: cancel }));
        unsafe {
            if PostMessageW(Some(HWND(main as *mut _)), WM_RECORD_DONE, WPARAM(0), LPARAM(raw as isize)).is_err() {
                drop(Box::from_raw(raw));
            }
        }
    });
}

pub fn on_done(d: Done) {
    if d.cancelled {
        return notify("Recording discarded", "Nothing was saved.");
    }
    match d.result {
        Ok(st) => {
            let e = crate::history::Entry::new(st.path.clone(), st.width, st.height, d.kind.history_kind());
            let _ = crate::history::append(&crate::paths::history_file(), &e);
            let size = std::fs::metadata(&st.path).map(|m| m.len()).unwrap_or(0);
            app::with(|s| s.pending_click = Some(Click::ShowFile(st.path.clone())));
            let name = st.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            notify(
                &format!("Recording saved ({:.1} s)", st.seconds),
                &format!("{name}\n{:.1} MB · click to show in folder", size as f64 / 1_048_576.0),
            );
        }
        Err(e) => notify("Recording failed", &e),
    }
}

struct Solid(Rgba);

impl Handler for Solid {
    fn message(&mut self, w: &mut Win, msg: u32, _wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_PAINT => {
                let mut rc = Default::default();
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(w.hwnd, &mut rc);
                }
                let mut bb = Backbuffer::new(rc.right.max(1) as u32, rc.bottom.max(1) as u32);
                if let Some(b) = bb.as_mut() {
                    b.pixmap().fill(self.0.color());
                }
                backbuffer::paint(w.hwnd, bb.as_ref());
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PillBtn {
    Stop,
    Cancel,
}

struct Pill {
    scale: f32,
    kind: Kind,
    bb: Option<Backbuffer>,
    hover: Option<PillBtn>,
}

impl Pill {
    fn buttons(&self, w: i32, h: i32) -> [(PillBtn, Rect); 2] {
        let b = h - (10.0 * self.scale) as i32;
        let y = (h - b) / 2;
        [(PillBtn::Stop, Rect::new(w - 2 * b - (14.0 * self.scale) as i32, y, b, b)), (PillBtn::Cancel, Rect::new(w - b - (8.0 * self.scale) as i32, y, b, b))]
    }
}

impl Handler for Pill {
    fn snapshot(&self) -> Option<snapit_gfx::tiny_skia::Pixmap> {
        self.bb.as_ref()?.to_pixmap()
    }

    fn message(&mut self, w: &mut Win, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let mut rc = Default::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(w.hwnd, &mut rc);
        }
        let (cw, ch) = (rc.right, rc.bottom);
        match msg {
            WM_TIMER => {
                if ACTIVE.with(|a| a.borrow().as_ref().is_some_and(|a| a.recorder.is_finished())) {
                    unsafe {
                        let _ = KillTimer(Some(w.hwnd), 1);
                    }
                    app::post(Command::StopRecording(false));
                }
                w.invalidate();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_PAINT => {
                Backbuffer::ensure(&mut self.bb, cw.max(1) as u32, ch.max(1) as u32);
                let s = self.scale;
                let (secs, frames) = ACTIVE.with(|a| a.borrow().as_ref().map(|a| (a.recorder.elapsed().as_secs(), a.recorder.frames())).unwrap_or((0, 0)));
                let buttons = self.buttons(cw, ch);
                if let Some(bb) = self.bb.as_mut() {
                    let mut pm = bb.pixmap();
                    let id = Transform::identity();
                    pm.fill(Rgba::rgb(24, 24, 27).color());
                    let blink = secs % 2 == 0 || frames == 0;
                    if let Some(dot) = PathBuilder::from_circle(16.0 * s, ch as f32 / 2.0, 6.0 * s) {
                        pm.fill_path(&dot, &Rgba::rgb(239, 68, 68).with_alpha(if blink { 255 } else { 120 }).paint(), FillRule::Winding, id, None);
                    }
                    let label = format!("{}  {:02}:{:02}", if self.kind == Kind::Gif { "GIF" } else { "REC" }, secs / 60, secs % 60);
                    let ts = 14.0 * s;
                    let l = text::layout(&label, ts, true);
                    text::draw(&mut pm, &label, 30.0 * s, (ch as f32 - l.height) / 2.0, ts, true, Rgba::WHITE, None);
                    for (b, r) in buttons {
                        let bg = if self.hover == Some(b) { Rgba::rgb(63, 63, 72) } else { Rgba::rgb(45, 45, 52) };
                        if let Some(p) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 6.0 * s) {
                            pm.fill_path(&p, &bg.paint(), FillRule::Winding, id, None);
                        }
                        let inset = r.w as f32 * 0.3;
                        match b {
                            PillBtn::Stop => {
                                if let Some(sq) = rounded_rect(r.x as f32 + inset, r.y as f32 + inset, r.w as f32 - inset * 2.0, r.h as f32 - inset * 2.0, 2.0) {
                                    pm.fill_path(&sq, &Rgba::rgb(239, 68, 68).paint(), FillRule::Winding, id, None);
                                }
                            }
                            PillBtn::Cancel => crate::ui::icons::draw(&mut pm, crate::ui::icons::Icon::Close, r.x as f32 + r.w as f32 * 0.15, r.y as f32 + r.h as f32 * 0.15, r.w as f32 * 0.7, Rgba::WHITE),
                        }
                    }
                }
                backbuffer::paint(w.hwnd, self.bb.as_ref());
                Some(LRESULT(0))
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                unsafe {
                    if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(c));
                    }
                }
                Some(LRESULT(1))
            }
            WM_MOUSEMOVE => {
                let (x, y) = crate::ui::lparam_xy(lp);
                let h = self.buttons(cw, ch).iter().find(|(_, r)| r.contains(x, y)).map(|(b, _)| *b);
                if h != self.hover {
                    self.hover = h;
                    w.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = crate::ui::lparam_xy(lp);
                if let Some((b, _)) = self.buttons(cw, ch).iter().find(|(_, r)| r.contains(x, y)) {
                    app::post(Command::StopRecording(*b == PillBtn::Cancel));
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}
