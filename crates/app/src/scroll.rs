//! Scrolling capture (opt-in; off by default).
//!
//! The cursor is parked over the selected area and mouse-wheel notches are sent
//! with `SendInput`. After each scroll the area is captured and stitched onto the
//! result. It stops when the content stops moving, a size limit is reached, or
//! the user moves the mouse. That needs no keyboard polling and no hooks.

use std::time::Duration;

use snapit_gfx::tiny_skia::{IntSize, Pixmap};
use snapit_gfx::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, SetCursorPos, WM_APP};

use crate::actions::Action;
use crate::app::{self, Capture, Command};

pub const WM_SCROLL_DONE: u32 = WM_APP + 6;
const MAX_STEPS: usize = 80;
const MAX_HEIGHT: usize = 30_000;

pub struct Done {
    pub result: Result<(Pixmap, Rect), String>,
}

pub fn start(r: Rect) {
    if !app::settings().scrolling_capture {
        return;
    }
    app::post(Command::Notify { title: "Scrolling capture…".into(), body: "Move the mouse to stop early.".into() });
    let main = app::main_hwnd().0 as isize;
    std::thread::spawn(move || {
        let result = run(r);
        let raw = Box::into_raw(Box::new(Done { result: result.map(|p| (p, r)) }));
        unsafe {
            if PostMessageW(Some(HWND(main as *mut _)), WM_SCROLL_DONE, WPARAM(0), LPARAM(raw as isize)).is_err() {
                drop(Box::from_raw(raw));
            }
        }
    });
}

pub fn on_done(d: Done) {
    match d.result {
        Ok((image, r)) => {
            let tasks = app::settings().tasks_for(Action::CaptureScrolling);
            crate::tasks::run(Capture { image, kind: Action::CaptureScrolling, title: "Scrolling".into(), screen_rect: Some(r) }, tasks);
        }
        Err(e) => app::post(Command::Notify { title: "Scrolling capture stopped".into(), body: e }),
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

fn run(r: Rect) -> Result<Pixmap, String> {
    let original = cursor();
    let (cx, cy) = r.center();
    std::thread::sleep(Duration::from_millis(250)); // let the overlay disappear
    unsafe {
        let _ = SetCursorPos(cx, cy);
    }
    let mut st = Stitcher::new(snapit_capture::capture_rect(r, false)?);
    let mut stopped_by_user = false;
    for _ in 0..MAX_STEPS {
        let p = cursor();
        if (p.x - cx).abs() > 3 || (p.y - cy).abs() > 3 {
            stopped_by_user = true;
            break;
        }
        wheel(3);
        std::thread::sleep(Duration::from_millis(350)); // smooth-scroll animations
        let frame = snapit_capture::capture_rect(r, false)?;
        if !st.push(&frame) || st.height() >= MAX_HEIGHT {
            break;
        }
    }
    if !stopped_by_user {
        unsafe {
            let _ = SetCursorPos(original.x, original.y);
        }
    }
    st.finish().ok_or_else(|| "nothing was captured".into())
}

/// Stitches successive frames of a scrolling area into one tall image.
pub struct Stitcher {
    w: usize,
    h: usize,
    /// Rows of the result so far (BGRA), ending with the latest static footer.
    rows: Vec<u8>,
    prev: Pixmap,
}

fn row_hashes(pm: &Pixmap) -> Vec<u64> {
    let w = pm.width() as usize * 4;
    pm.data()
        .chunks_exact(w)
        .map(|row| {
            // FNV-1a over the row, ignoring alpha.
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for px in row.as_chunks::<4>().0 {
                for &b in &px[..3] {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x100_0000_01b3);
                }
            }
            h
        })
        .collect()
}

impl Stitcher {
    pub fn new(first: Pixmap) -> Stitcher {
        let (w, h) = (first.width() as usize, first.height() as usize);
        Stitcher { w, h, rows: first.data().to_vec(), prev: first }
    }

    pub fn height(&self) -> usize {
        self.rows.len() / (self.w * 4)
    }

    /// Adds a frame. Returns false when the content did not move (end reached).
    pub fn push(&mut self, next: &Pixmap) -> bool {
        if next.width() as usize != self.w || next.height() as usize != self.h {
            return false;
        }
        let a = row_hashes(&self.prev);
        let b = row_hashes(next);
        let h = self.h;
        // Rows identical at the same position are static (sticky header/footer).
        let top = (0..h).take_while(|&y| a[y] == b[y]).count();
        if top == h {
            return false;
        }
        let bottom = (0..h).rev().take_while(|&y| a[y] == b[y]).count();
        let mid = h - top - bottom;
        // Find the smallest shift dy where next's middle matches prev's middle moved up by dy.
        let mut best = None;
        for dy in 1..mid {
            let n = mid - dy;
            if n < 8.min(mid / 2).max(1) {
                break;
            }
            let matches = (0..n).filter(|&i| b[top + i] == a[top + dy + i]).count();
            if matches as f32 >= n as f32 * 0.97 {
                best = Some(dy);
                break;
            }
        }
        let Some(dy) = best else {
            // No overlap found: the page jumped. Stop rather than produce a broken image.
            return false;
        };
        // Drop the previous frame's static footer rows, then add the newly revealed rows.
        let row = self.w * 4;
        let keep = self.height() - bottom;
        self.rows.truncate(keep * row);
        let start = top + mid - dy;
        self.rows.extend_from_slice(&next.data()[start * row..(top + mid) * row]);
        // Re-append the static footer from the latest frame.
        self.rows.extend_from_slice(&next.data()[(h - bottom) * row..h * row]);
        self.prev = next.clone();
        true
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
            assert!(st.push(&view(&p, off, w, h, hd, ft)));
        }
        assert!(!st.push(&view(&p, 120, w, h, hd, ft)), "no movement ends the capture");
        let out = st.finish().unwrap();
        // header + content rows 10..(120+92) + footer
        assert_eq!(out.height() as usize, hd + (120 + h - ft - hd) + ft);
        let row = |y: usize| &out.data()[y * w * 4..y * w * 4 + 4];
        assert_eq!(row(0), &[200, 10, 10, 255]);
        assert_eq!(row(hd + 50), &p[hd + 50]);
        assert_eq!(row(hd + 150), &p[hd + 150]);
        assert_eq!(row(out.height() as usize - 1), &[10, 200, 10, 255]);
    }
}
