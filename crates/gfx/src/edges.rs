//! Finding the visual box (panel, card, button, text field…) under a point from
//! the pixels alone. Used to snap region selection inside windows that expose no
//! child windows or UI Automation elements.
//!
//! A box side is a straight line of colour changes. From the point we scan
//! outwards in each direction for the first column or row where nearly every
//! pixel along the current span differs from its neighbour, then re-scan using
//! the box found so far as the span until it settles. Isolated glyph strokes
//! never cover enough of a span to count as a side.

use crate::Rect;
use tiny_skia::Pixmap;

/// Summed RGB difference between neighbours that counts as an edge.
const EDGE: u32 = 24;
/// Fraction of the span that must be edge pixels: strict for the small first
/// probe, looser once the span is a whole box side (borders may be broken by
/// overlapping content or rounded corners).
const SEED_FRAC: f32 = 0.9;
const SIDE_FRAC: f32 = 0.6;
/// Half-height/width of the first probe around the point.
const SEED: i32 = 14;
const MIN_SIDE: i32 = 10;
/// How far past a side the neighbouring sides must run for it to be a divider.
const DIVIDER: i32 = 12;
/// Samples taken along a long line before checking it pixel by pixel.
const PROBES: i32 = 16;

struct Img<'a> {
    d: &'a [u8],
    w: i32,
}

impl Img<'_> {
    #[inline]
    fn diff(&self, ax: i32, ay: i32, bx: i32, by: i32) -> bool {
        let a = ((ay * self.w + ax) * 4) as usize;
        let b = ((by * self.w + bx) * 4) as usize;
        let d = |i: usize| (self.d[a + i] as i32 - self.d[b + i] as i32).unsigned_abs();
        d(0) + d(1) + d(2) > EDGE
    }

    /// Do at least `frac` of the positions in [a, b) pass `hit`?
    ///
    /// Most lines scanned are not edges, so a long span first checks a few evenly
    /// spaced samples (with slack) and only lines that pass get the exact count.
    /// That keeps a scan across a 4K window to a few samples per line.
    #[inline]
    fn line(a: i32, b: i32, frac: f32, hit: impl Fn(i32) -> bool) -> bool {
        let n = b - a;
        if n <= 0 {
            return false;
        }
        if n > 4 * PROBES {
            let allowed = ((1.0 - frac) * PROBES as f32) as i32 + 2;
            let mut miss = 0;
            for i in 0..PROBES {
                if !hit(a + (2 * i + 1) * n / (2 * PROBES)) {
                    miss += 1;
                    if miss > allowed {
                        return false;
                    }
                }
            }
        }
        let allowed = ((1.0 - frac) * n as f32) as i32;
        let mut miss = 0;
        for p in a..b {
            if !hit(p) {
                miss += 1;
                if miss > allowed {
                    return false;
                }
            }
        }
        true
    }

    /// Is there an edge between columns x-1 and x over rows [y0, y1)?
    fn col_edge(&self, x: i32, y0: i32, y1: i32, frac: f32) -> bool {
        Self::line(y0, y1, frac, |y| self.diff(x - 1, y, x, y))
    }

    /// Is there an edge between rows y-1 and y over columns [x0, x1)?
    fn row_edge(&self, y: i32, x0: i32, x1: i32, frac: f32) -> bool {
        Self::line(x0, x1, frac, |x| self.diff(x, y - 1, x, y))
    }
}

/// The innermost box around (x, y) within `bounds`, or `None` when no box
/// smaller than `bounds` is found. Divider lines count as sides.
pub fn box_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Option<Rect> {
    find(pm, x, y, bounds, false)
}

/// Like [`box_at`], but steps past dividers: a side line counts only where the
/// box's neighbouring sides end at it. If both neighbouring sides carry on past
/// it, it splits a larger box (a separator in a card, a list row) and the scan
/// continues outwards. This is the less eager box, used as the default target.
pub fn outer_box_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Option<Rect> {
    find(pm, x, y, bounds, true)
}

fn find(pm: &Pixmap, x: i32, y: i32, bounds: Rect, skip_dividers: bool) -> Option<Rect> {
    let bounds = bounds.intersect(Rect::new(0, 0, pm.width() as i32, pm.height() as i32))?;
    if !bounds.contains(x, y) {
        return None;
    }
    let img = Img { d: pm.data(), w: pm.width() as i32 };
    let (bl, bt, br, bb) = (bounds.x, bounds.y, bounds.right(), bounds.bottom());

    // Scans return box edges as [l, r) × [t, b), starting at `from` and moving outwards.
    let left = |from: i32, y0, y1, f| (bl + 1..=from).rev().find(|&c| img.col_edge(c, y0, y1, f)).unwrap_or(bl);
    let right = |from: i32, y0, y1, f| (from..br).find(|&c| img.col_edge(c, y0, y1, f)).unwrap_or(br);
    let top = |from: i32, x0, x1, f| (bt + 1..=from).rev().find(|&r| img.row_edge(r, x0, x1, f)).unwrap_or(bt);
    let bottom = |from: i32, x0, x1, f| (from..bb).find(|&r| img.row_edge(r, x0, x1, f)).unwrap_or(bb);

    // Does the line at `at` continue `DIVIDER` px beyond `side` (towards `dir`)?
    let col_runs = |at: i32, side: i32, dir: i32| {
        let (a, b) = if dir < 0 { ((side - DIVIDER).max(bt), side) } else { (side, (side + DIVIDER).min(bb)) };
        at > bl && at < br && b - a >= DIVIDER / 2 && img.col_edge(at, a, b, SIDE_FRAC)
    };
    let row_runs = |at: i32, side: i32, dir: i32| {
        let (a, b) = if dir < 0 { ((side - DIVIDER).max(bl), side) } else { (side, (side + DIVIDER).min(br)) };
        at > bt && at < bb && b - a >= DIVIDER / 2 && img.row_edge(at, a, b, SIDE_FRAC)
    };

    let (sy0, sy1) = ((y - SEED).max(bt), (y + SEED + 1).min(bb));
    let (sx0, sx1) = ((x - SEED).max(bl), (x + SEED + 1).min(br));
    let mut b = (left(x, sy0, sy1, SEED_FRAC), top(y, sx0, sx1, SEED_FRAC), right(x + 1, sy0, sy1, SEED_FRAC), bottom(y + 1, sx0, sx1, SEED_FRAC));
    for _ in 0..6 {
        let (l, t, r, bo) = b;
        let mut l2 = left(x, t, bo, SIDE_FRAC);
        let mut r2 = right(x + 1, t, bo, SIDE_FRAC);
        let mut t2 = top(y, l2, r2, SIDE_FRAC);
        let mut b2 = bottom(y + 1, l2, r2, SIDE_FRAC);
        if skip_dividers {
            // A side is a divider when both neighbouring sides run on past it.
            while l2 > bl && row_runs(t2, l2, -1) && row_runs(b2, l2, -1) {
                l2 = left(l2 - 1, t2, b2, SIDE_FRAC);
            }
            while r2 < br && row_runs(t2, r2, 1) && row_runs(b2, r2, 1) {
                r2 = right(r2 + 1, t2, b2, SIDE_FRAC);
            }
            while t2 > bt && col_runs(l2, t2, -1) && col_runs(r2, t2, -1) {
                t2 = top(t2 - 1, l2, r2, SIDE_FRAC);
            }
            while b2 < bb && col_runs(l2, b2, 1) && col_runs(r2, b2, 1) {
                b2 = bottom(b2 + 1, l2, r2, SIDE_FRAC);
            }
        }
        let next = (l2, t2, r2, b2);
        if next == (l, t, r, bo) {
            break;
        }
        b = next;
    }
    let r = Rect::from_ltrb(b.0, b.1, b.2, b.3);
    (r.w >= MIN_SIDE && r.h >= MIN_SIDE && r != bounds && r.contains(x, y)).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Paint, Transform};

    fn canvas() -> Pixmap {
        let mut pm = Pixmap::new(200, 150).unwrap();
        pm.fill(Color::WHITE);
        let mut p = Paint::default();
        // A grey panel with a darker button inside it.
        p.set_color_rgba8(220, 220, 220, 255);
        pm.fill_rect(tiny_skia::Rect::from_xywh(20.0, 20.0, 160.0, 110.0).unwrap(), &p, Transform::identity(), None);
        p.set_color_rgba8(60, 110, 200, 255);
        pm.fill_rect(tiny_skia::Rect::from_xywh(40.0, 40.0, 50.0, 20.0).unwrap(), &p, Transform::identity(), None);
        // "Text": short strokes that must not be taken for box sides.
        p.set_color_rgba8(30, 30, 30, 255);
        for i in 0..6 {
            pm.fill_rect(tiny_skia::Rect::from_xywh(100.0 + i as f32 * 8.0, 90.0, 2.0, 9.0).unwrap(), &p, Transform::identity(), None);
        }
        pm
    }

    #[test]
    fn finds_innermost_box() {
        let pm = canvas();
        let all = Rect::new(0, 0, 200, 150);
        assert_eq!(box_at(&pm, 60, 50, all), Some(Rect::new(40, 40, 50, 20)));
        assert_eq!(box_at(&pm, 150, 50, all), Some(Rect::new(20, 20, 160, 110)));
    }

    #[test]
    fn text_does_not_split_a_panel() {
        let pm = canvas();
        assert_eq!(box_at(&pm, 105, 94, Rect::new(0, 0, 200, 150)), Some(Rect::new(20, 20, 160, 110)));
    }

    #[test]
    fn outer_box_steps_past_a_divider() {
        // A bordered card split by a horizontal divider.
        let mut pm = Pixmap::new(200, 150).unwrap();
        pm.fill(Color::WHITE);
        let mut p = Paint::default();
        p.set_color_rgba8(120, 120, 120, 255);
        let mut fill = |x: f32, y: f32, w: f32, h: f32| pm.fill_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap(), &p, Transform::identity(), None);
        fill(20.0, 20.0, 160.0, 1.0);
        fill(20.0, 129.0, 160.0, 1.0);
        fill(20.0, 20.0, 1.0, 110.0);
        fill(179.0, 20.0, 1.0, 110.0);
        fill(20.0, 70.0, 160.0, 1.0); // divider
        let all = Rect::new(0, 0, 200, 150);
        // Innermost: the half under the cursor (inside the border lines).
        assert_eq!(box_at(&pm, 100, 100, all), Some(Rect::new(21, 71, 158, 58)));
        // Outer: the whole card.
        assert_eq!(outer_box_at(&pm, 100, 100, all), Some(Rect::new(21, 21, 158, 108)));
    }

    #[test]
    fn outer_box_keeps_a_plain_box() {
        let pm = canvas();
        let all = Rect::new(0, 0, 200, 150);
        assert_eq!(outer_box_at(&pm, 60, 50, all), Some(Rect::new(40, 40, 50, 20)));
        assert_eq!(outer_box_at(&pm, 150, 50, all), Some(Rect::new(20, 20, 160, 110)));
    }

    #[test]
    fn margin_beside_a_panel_is_a_strip() {
        // Bounded by the window edge on one side and the panel on the other, like a sidebar.
        let pm = canvas();
        assert_eq!(box_at(&pm, 5, 5, Rect::new(0, 0, 200, 150)), Some(Rect::new(0, 0, 20, 150)));
    }

    /// Worst case: a maximised 4K window with a big panel. Run with
    /// `cargo test --release -p snapit-gfx edges_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn edges_speed() {
        let mut pm = Pixmap::new(3840, 2160).unwrap();
        pm.fill(Color::WHITE);
        let mut p = Paint::default();
        p.set_color_rgba8(220, 220, 220, 255);
        pm.fill_rect(tiny_skia::Rect::from_xywh(300.0, 200.0, 3200.0, 1800.0).unwrap(), &p, Transform::identity(), None);
        p.set_color_rgba8(30, 30, 30, 255);
        for i in 0..300 {
            pm.fill_rect(tiny_skia::Rect::from_xywh(400.0 + (i % 60) as f32 * 50.0, 300.0 + (i / 60) as f32 * 300.0, 3.0, 14.0).unwrap(), &p, Transform::identity(), None);
        }
        let all = Rect::new(0, 0, 3840, 2160);
        let t = std::time::Instant::now();
        let n = 50;
        for i in 0..n {
            std::hint::black_box(box_at(&pm, 1000 + i * 7, 1000 + i * 3, all));
        }
        println!("box_at: {:.2} ms per call", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }

    #[test]
    fn blank_area_gives_nothing() {
        let mut pm = Pixmap::new(120, 80).unwrap();
        pm.fill(Color::WHITE);
        assert_eq!(box_at(&pm, 60, 40, Rect::new(0, 0, 120, 80)), None);
    }
}
