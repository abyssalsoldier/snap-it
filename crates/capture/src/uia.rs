//! UI Automation element snapshots for control-level snapping.
//!
//! Modern apps (Chromium/Electron, WPF, WinUI/UWP, Windows 11 shell) draw their
//! whole UI into one surface and have no child HWNDs, so `EnumChildWindows`
//! finds nothing to snap to. UI Automation exposes their element tree instead.
//!
//! The overlay never calls UIA while the mouse moves. One worker thread fetches
//! every element rectangle of a window in a single cached `FindAllBuildCache`
//! call, with connection/transaction timeouts so a hung app cannot stall it,
//! and the overlay hit-tests that list locally.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use snapit_gfx::Rect;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, TreeScope_Subtree, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId, UIA_TextControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Hard cap on elements kept per window (a big web page can expose tens of thousands).
const MAX_ELEMENTS: i32 = 20_000;
/// Milliseconds UIA may wait on a provider before giving up.
const TIMEOUT_MS: u32 = 1500;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Element {
    /// Screen coordinates.
    pub rect: Rect,
    /// A text label or a tiny part that usually sits inside the control you want
    /// (a button's caption, an icon glyph). Reachable, but not the default target.
    pub fine: bool,
}

/// Elements smaller than this on either side count as fine.
const FINE_SIDE: i32 = 12;

#[derive(Clone, Debug)]
pub enum Fetch {
    Pending,
    /// Elements, smallest first.
    Ready(Vec<Element>),
    Failed,
}

/// A background UIA fetcher, alive as long as the overlay that owns it.
pub struct Fetcher {
    tx: Option<Sender<isize>>,
    results: Arc<Mutex<HashMap<isize, Fetch>>>,
}

impl Fetcher {
    /// Starts the worker. When a window's elements arrive it posts `msg`
    /// (wparam = the window handle) to `notify` so the overlay can re-hit-test.
    pub fn new(notify: isize, msg: u32) -> Self {
        let (tx, rx) = channel::<isize>();
        let results: Arc<Mutex<HashMap<isize, Fetch>>> = Arc::default();
        let out = results.clone();
        let spawned = std::thread::Builder::new().name("snapit-uia".into()).spawn(move || unsafe {
            let inited = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let uia: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
            if let Some(u2) = uia.as_ref().and_then(|u| u.cast::<IUIAutomation2>().ok()) {
                let _ = u2.SetConnectionTimeout(TIMEOUT_MS);
                let _ = u2.SetTransactionTimeout(TIMEOUT_MS);
            }
            while let Ok(hwnd) = rx.recv() {
                let res = uia.as_ref().and_then(|u| elements_of(u, hwnd)).map_or(Fetch::Failed, Fetch::Ready);
                if let Ok(mut m) = out.lock() {
                    m.insert(hwnd, res);
                }
                let _ = PostMessageW(Some(HWND(notify as *mut _)), msg, WPARAM(hwnd as usize), LPARAM(0));
            }
            drop(uia);
            if inited {
                CoUninitialize();
            }
        });
        Self { tx: spawned.ok().map(|_| tx), results }
    }

    /// The elements of `hwnd`, queuing a fetch the first time it is asked for.
    pub fn get(&self, hwnd: isize) -> Fetch {
        let Ok(mut m) = self.results.lock() else { return Fetch::Failed };
        if let Some(f) = m.get(&hwnd) {
            return f.clone();
        }
        match &self.tx {
            Some(tx) if tx.send(hwnd).is_ok() => {
                m.insert(hwnd, Fetch::Pending);
                Fetch::Pending
            }
            _ => Fetch::Failed,
        }
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        // Closing the channel ends the worker after its current fetch; it is not joined
        // so a slow provider never delays closing the overlay.
        self.tx = None;
    }
}

unsafe fn elements_of(uia: &IUIAutomation, hwnd: isize) -> Option<Vec<Element>> {
    let root = uia.ElementFromHandle(HWND(hwnd as *mut _)).ok()?;
    let cache = uia.CreateCacheRequest().ok()?;
    cache.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
    cache.AddProperty(UIA_IsOffscreenPropertyId).ok()?;
    cache.AddProperty(UIA_ControlTypePropertyId).ok()?;
    let cond = uia.ControlViewCondition().ok()?;
    let all = root.FindAllBuildCache(TreeScope_Subtree, &cond, &cache).ok()?;
    let n = all.Length().ok()?.min(MAX_ELEMENTS);
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n {
        let Ok(e) = all.GetElement(i) else { continue };
        if e.CachedIsOffscreen().is_ok_and(|b| b.as_bool()) {
            continue;
        }
        let Ok(r) = e.CachedBoundingRectangle() else { continue };
        let r = Rect::from_ltrb(r.left, r.top, r.right, r.bottom);
        if r.w > 4 && r.h > 4 {
            let text = e.CachedControlType().is_ok_and(|t| t == UIA_TextControlTypeId);
            out.push(Element { rect: r, fine: text || r.w < FINE_SIDE || r.h < FINE_SIDE });
        }
    }
    out.sort_by_key(|e| e.rect.area());
    out.dedup();
    Some(out)
}
