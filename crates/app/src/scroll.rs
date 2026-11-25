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

/// Why a scrolling capture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// The content stopped moving: the end was reached.
    End,
    /// The mouse moved (the way to stop early).
    User,
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
    app::post(Command::Notify { title: "Scrolling capture…".into(), body: "Move the mouse to stop early.".into() });
    let main = app::main_hwnd().0 as isize;
    std::thread::spawn(move || {
        let result = run(r);
        let raw = Box::into_raw(Box::new(Done { result: result.map(|(p, stop, steps)| (p, r, stop, steps)) }));
        unsafe {
            if PostMessageW(Some(HWND(main as *mut _)), WM_SCROLL_DONE, WPARAM(0), LPARAM(raw as isize)).is_err() {
                drop(Box::from_raw(raw));
            }
        }
    });
}

pub fn on_done(d: Done) {
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
                Stop::User if steps == 0 => explain(
                    "Scrolling capture stopped because the mouse moved. Keep the mouse still while it scrolls; move it to stop early.\n\n\
                     Over Remote Desktop, the remote cursor can follow your local mouse, so keep that still too.",
                ),
                Stop::End if steps == 0 => explain(
                    "The selected area did not move when scrolled, so only one screen was captured.\n\n\
                     Select an area inside the part that scrolls, and make sure that window accepts mouse-wheel scrolling.",
                ),
                _ => {}
            }
        }
        Err(e) => explain(&format!("Scrolling capture failed: {e}")),
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

/// Returns the stitched image, why it stopped and how many scrolls were stitched.
fn run(r: Rect) -> Result<(Pixmap, Stop, usize), String> {
    let original = cursor();
    let (cx, cy) = r.center();
    std::thread::sleep(Duration::from_millis(250)); // let the overlay disappear
    unsafe {
        let _ = SetCursorPos(cx, cy);
    }
    // One wheel notch is roughly 50-150 px depending on the app and DPI. Scroll less in small
    // areas so consecutive frames still overlap enough to be lined up.
    let notches = (r.h / 300).clamp(1, 3);
    let mut st = Stitcher::new(snapit_capture::capture_rect(r, false)?);
    let mut steps = 0;
    let mut stop = Stop::Limit;
    'outer: for _ in 0..MAX_STEPS {
        let p = cursor();
        if (p.x - cx).abs() > 3 || (p.y - cy).abs() > 3 {
            stop = Stop::User;
            break;
        }
        wheel(notches);
        std::thread::sleep(Duration::from_millis(350)); // smooth-scroll animations
        // A frame caught mid-animation or mid-repaint may not line up (or may not have moved
        // yet), so look again once before deciding.
        let mut retried = false;
        loop {
            let frame = snapit_capture::capture_rect(r, false)?;
            match st.push(&frame) {
                Push::Added => break,
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
    if stop != Stop::User {
        unsafe {
            let _ = SetCursorPos(original.x, original.y);
        }
    }
    let image = st.finish().ok_or_else(|| "nothing was captured".to_string())?;
    Ok((image, stop, steps))
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

impl Stitcher {
    pub fn new(first: Pixmap) -> Stitcher {
        let (w, h) = (first.width() as usize, first.height() as usize);
        Stitcher { w, h, rows: first.data().to_vec(), prev: cell_hashes(&first) }
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
        if best.0 < MATCH {
            // No overlap found: the page jumped or is animating. Stop rather than produce a
            // broken image.
            return Push::NoMatch;
        }
        let dy = best.1;
        // Drop the previous frame's static footer rows, then add the newly revealed rows.
        let row = self.w * 4;
        let keep = self.height() - bottom;
        self.rows.truncate(keep * row);
        let start = top + mid - dy;
        self.rows.extend_from_slice(&next.data()[start * row..(top + mid) * row]);
        // Re-append the static footer from the latest frame.
        self.rows.extend_from_slice(&next.data()[(h - bottom) * row..h * row]);
        self.prev = b;
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

    #[test]
    fn a_frame_that_does_not_line_up_is_rejected() {
        let p = page(3000);
        let mut st = Stitcher::new(view(&p, 0, 6, 100, 0, 0));
        // Jumped much further than the frame height: no overlap.
        assert_eq!(st.push(&view(&p, 1500, 6, 100, 0, 0)), Push::NoMatch);
        assert_eq!(st.height(), 100, "a rejected frame adds nothing");
    }
}
