//! Process-wide state for the UI thread, and the command queue.
//!
//! Window procedures never do heavy work or open other windows directly. They
//! `post()` a [`Command`], and the main loop runs it after the current message
//! finishes. That keeps window procs free of re-entrancy surprises.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use snapit_gfx::{tiny_skia::Pixmap, Rect};
use windows::Win32::Foundation::HWND;

use crate::actions::Action;
use crate::settings::Settings;

/// A finished capture travelling through the task chain.
pub struct Capture {
    pub image: Pixmap,
    pub kind: Action,
    pub title: String,
    /// Where on screen it came from (used to place pinned images).
    pub screen_rect: Option<Rect>,
}

/// How the user finished a region capture from the overlay's quick-annotate bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Run the configured task chain (Enter).
    Tasks,
    Copy,
    Save,
    Pin,
    Editor,
}

pub enum Command {
    Run(Action),
    /// Run without applying the capture delay (the delay already elapsed).
    RunNow(Action),
    Finish(Capture, Finish),
    OpenEditor { image: Pixmap, path: Option<PathBuf> },
    /// Open the editor keeping quick-annotate items editable (item coords = image pixels).
    OpenEditorWith { image: Pixmap, items: Vec<snapit_annotate::Item> },
    /// Insert an image (picked with the overlay) into the editor with this id.
    EditorInsert { id: u64, image: Pixmap },
    Pin { image: Pixmap, at: Option<Rect> },
    StartScrolling(Rect),
    StartRecording(crate::record_ui::Kind, Rect),
    /// Stop the active recording (`true` = discard it).
    StopRecording(bool),
    Ocr(Pixmap),
    ScanQr(Pixmap),
    Notify { title: String, body: String },
    OpenLastCapture,
    ReloadSettings,
    /// Open the first-launch setup wizard.
    OpenWelcome,
    OverlayClosed,
}

/// Notification click actions.
pub enum Click {
    Url(String),
    ShowFile(PathBuf),
    /// Open the first-launch setup (it has new steps since this user last saw it).
    OpenWelcome,
    /// Open "Report a bug" (Snap It crashed last time).
    ReportBug,
}

pub struct State {
    pub settings: Rc<Settings>,
    pub main_hwnd: HWND,
    pub overlay_open: bool,
    pub last_saved: Option<PathBuf>,
    pub last_image: Option<Rc<Pixmap>>,
    pub bindings: Vec<(u16, crate::hotkeys::Chord)>,
    pub pending_delayed: Option<Action>,
    /// What clicking the latest notification does (instead of opening the last capture).
    pub pending_click: Option<Click>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Command>> = const { RefCell::new(VecDeque::new()) };
}

pub fn init(state: State) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
}

/// Short borrow of the state. Never open windows or post Win32 messages inside `f`.
pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("app state initialised")))
}

pub fn settings() -> Rc<Settings> {
    with(|s| s.settings.clone())
}

pub fn main_hwnd() -> HWND {
    with(|s| s.main_hwnd)
}

/// Applies a change to the settings and writes them to disk.
pub fn update_settings(f: impl FnOnce(&mut Settings)) {
    let mut next = (*settings()).clone();
    f(&mut next);
    if let Err(e) = next.save() {
        crate::log(&format!("could not save settings: {e}"));
    }
    with(|s| s.settings = Rc::new(next));
}

pub fn post(cmd: Command) {
    QUEUE.with(|q| q.borrow_mut().push_back(cmd));
}

pub fn next_command() -> Option<Command> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}
