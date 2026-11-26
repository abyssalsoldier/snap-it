//! Finding the visual box (panel, card, button, text field…) under a point from
//! the pixels alone. Used to snap region selection inside windows that expose no
//! child windows or UI Automation elements.
//!
//! A box side is a straight line of colour changes. From the point we scan
//! outwards in each direction for the first column or row where nearly every
//! pixel along the current span differs from its neighbour, then re-scan using
//! the box found so far as the span until it settles. Isolated glyph strokes
//! never cover enough of a span to count as a side, and a line counts only if
//! one side of it is smooth along it, so detail inside pictures (icons, game
//! art, photos) does not either.
//!
//! [`boxes_at`] gives the whole set for snapping: finer parts, the default box,
//! then each enclosing box in turn (button, toolbar, panel…), so scrolling steps
//! through them in any app, not just ones that expose UI Automation.

use crate::Rect;
use tiny_skia::Pixmap;

/// Summed RGB difference between neighbours that counts as an edge.
const EDGE: u32 = 24;
/// Fraction of the span that must be edge pixels: strict for the small first
/// probe, looser once the span is a whole box side (borders may be broken by
/// overlapping content or rounded corners).
const SEED_FRAC: f32 = 0.9;
const SIDE_FRAC: f32 = 0.6;
/// Fraction of a side line along which one of its two sides must be smooth. Real
/// frames and padding are smooth almost all the way; a gradient or texture inside
/// a picture is not, even where it changes colour across a line most of the way.
const STRICT: f32 = 0.85;
/// The looser requirement of [`loose_box_at`].
const LOOSE: f32 = 0.75;
/// Half-height/width of the first probe around the point.
const SEED: i32 = 14;
const MIN_SIDE: i32 = 10;
/// Enclosing boxes [`boxes_at`] steps out through at most.
const MAX_LEVELS: usize = 4;
/// Enclosing-box scans [`boxes_at`] makes at most (frame lines it steps across
/// one by one included).
const MAX_SCANS: usize = 12;
/// Boxes whose sides are all this close count as the same box.
const NEAR: i32 = 3;
/// How far past a side the neighbouring sides must run for it to be a divider.
const DIVIDER: i32 = 12;
/// Samples taken along a long line before checking it pixel by pixel.
const PROBES: i32 = 16;

struct Img<'a> {
    /// Fraction of a side line along which one side must be smooth.
    smooth: f32,
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
    ///
    /// Besides nearly every row changing colour across the line, one side of it
    /// must be smooth along the line (a frame, a gutter, a panel's padding).
    /// Where both sides are busy it is detail inside a picture, such as a stroke
    /// in a game's ability icon, not the side of a box.
    fn col_edge(&self, x: i32, y0: i32, y1: i32, frac: f32) -> bool {
        let y0 = y0.max(1);
        Self::line(y0, y1, frac, |y| self.diff(x - 1, y, x, y))
            && (Self::line(y0, y1, self.smooth, |y| !self.diff(x - 1, y - 1, x - 1, y)) || Self::line(y0, y1, self.smooth, |y| !self.diff(x, y - 1, x, y)))
    }

    /// Is there an edge between rows y-1 and y over columns [x0, x1)?
    fn row_edge(&self, y: i32, x0: i32, x1: i32, frac: f32) -> bool {
        let x0 = x0.max(1);
        Self::line(x0, x1, frac, |x| self.diff(x, y - 1, x, y))
            && (Self::line(x0, x1, self.smooth, |x| !self.diff(x - 1, y - 1, x, y - 1)) || Self::line(x0, x1, self.smooth, |x| !self.diff(x - 1, y, x, y)))
    }
}

/// The innermost box around (x, y) within `bounds`, or `None` when no box
/// smaller than `bounds` is found. Divider lines count as sides.
pub fn box_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Option<Rect> {
    find(pm, x, y, bounds, false, STRICT)
}

/// Like [`box_at`], but steps past dividers: a side line counts only where the
/// box's neighbouring sides end at it. If both neighbouring sides carry on past
/// it, it splits a larger box (a separator in a card, a list row) and the scan
/// continues outwards. This is the less eager box, used as the default target.
pub fn outer_box_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Option<Rect> {
    find(pm, x, y, bounds, true, STRICT)
}

/// Like [`box_at`], but a side line only needs one side smooth along most of it
/// (not nearly all). Finds boxes whose frame is partly covered or crossed (small
/// slots in a bar with a heavier frame, an item under a badge), where the strict
/// scan steps out further.
/// Looser, so it can also stop at detail inside a picture: offer it as a finer
/// target, not the default.
pub fn loose_box_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Option<Rect> {
    find(pm, x, y, bounds, false, LOOSE)
}

/// The boxes around (x, y) for region snapping, from the smallest to the largest
/// below `bounds`, each flagged `fine` when it should not be the default target.
///
/// - The default is [`outer_box_at`]; [`box_at`] (a divided part) and
///   [`loose_box_at`] (a slot whose frame is crossed by its container's) sit
///   below it as finer steps.
/// - Above it, each next box is the one enclosing the previous: a button, then the
///   bar or panel holding it, then the panel holding that, until nothing smaller
///   than `bounds` encloses it.
///
/// Boxes within a few pixels of one already listed are left out, so each scroll
/// step visibly changes the selection.
pub fn boxes_at(pm: &Pixmap, x: i32, y: i32, bounds: Rect) -> Vec<(Rect, bool)> {
    let outer = outer_box_at(pm, x, y, bounds);
    let mut out: Vec<(Rect, bool)> = Vec::new();
    fn add(out: &mut Vec<(Rect, bool)>, r: Rect, fine: bool) {
        if !out.iter().any(|(o, _)| near(*o, r)) {
            out.push((r, fine));
        }
    }
    // Default first, so a finer box only stays when it differs from it.
    if let Some(o) = outer {
        add(&mut out, o, false);
    }
    for r in [box_at(pm, x, y, bounds), loose_box_at(pm, x, y, bounds)].into_iter().flatten() {
        add(&mut out, r, true);
    }
    // Step out from the largest box found at the point. A thick or bevelled frame
    // is several lines, so some steps only cross one of them; those are too small
    // to show and do not count as levels.
    let mut cur = out.iter().map(|(r, _)| *r).max_by_key(|r| r.area());
    let mut levels = 0;
    for _ in 0..MAX_SCANS {
        let Some(c) = cur else { break };
        // Most of `bounds` is the leftover around other boxes, not a container;
        // the window itself is the next step anyway.
        cur = enclosing_box(pm, c, bounds).filter(|e| e.area() * 4 < bounds.area() * 3);
        if let Some(e) = cur {
            let n = out.len();
            add(&mut out, e, false);
            levels += usize::from(out.len() > n);
            if levels == MAX_LEVELS {
                break;
            }
        }
    }
    out.sort_by_key(|(r, _)| r.area());
    out
}

/// The nearest box around `inner` (strictly larger) below `bounds`, stepping past
/// dividers: the panel or bar a button sits in, not the next button.
pub fn enclosing_box(pm: &Pixmap, inner: Rect, bounds: Rect) -> Option<Rect> {
    let from = (inner.x - 1, inner.y - 1, inner.right() + 1, inner.bottom() + 1);
    let r = scan(pm, from, None, bounds, true, STRICT)?;
    (r != inner && r.contains_rect(inner)).then_some(r)
}

/// Are all four sides close (the same box for the user, give or take a frame)?
/// Used to drop scroll steps that would barely change the selection.
/// "Close" is a few pixels, or a tenth of the smaller box's shorter side.
pub fn near(a: Rect, b: Rect) -> bool {
    let tol = NEAR.max(a.w.min(a.h).min(b.w).min(b.h) / 10);
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol && (a.right() - b.right()).abs() <= tol && (a.bottom() - b.bottom()).abs() <= tol
}

fn find(pm: &Pixmap, x: i32, y: i32, bounds: Rect, skip_dividers: bool, smooth: f32) -> Option<Rect> {
    let r = scan(pm, (x, y, x + 1, y + 1), Some((x, y)), bounds, skip_dividers, smooth)?;
    r.contains(x, y).then_some(r)
}

/// Scans outwards for box sides: left from column `from.0`, top from row `from.1`,
/// right from `from.2` and bottom from `from.3`. With a `seed` point the first
/// pass is a small strict probe around it; otherwise the first pass uses the
/// starting box's own extent as the span.
fn scan(pm: &Pixmap, from: (i32, i32, i32, i32), seed: Option<(i32, i32)>, bounds: Rect, skip_dividers: bool, smooth: f32) -> Option<Rect> {
    let bounds = bounds.intersect(Rect::new(0, 0, pm.width() as i32, pm.height() as i32))?;
    // A box flush with an edge of `bounds` starts its scan on that edge.
    let (fl, ft) = (from.0.max(bounds.x), from.1.max(bounds.y));
    let (fr, fb) = (from.2.min(bounds.right()), from.3.min(bounds.bottom()));
    if fl >= fr || ft >= fb || !bounds.contains(fl, ft) {
        return None;
    }
    let img = Img { smooth, d: pm.data(), w: pm.width() as i32 };
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

    let mut b = match seed {
        Some((x, y)) => {
            let (sy0, sy1) = ((y - SEED).max(bt), (y + SEED + 1).min(bb));
            let (l, r) = (left(fl, sy0, sy1, SEED_FRAC), right(fr, sy0, sy1, SEED_FRAC));
            // Probe for top and bottom only between the sides just found: past them
            // is the next box, and something overlapping it there (a notification
            // badge, a status dot) must not hide this box's top or bottom.
            let (sx0, sx1) = ((x - SEED).max(l), (x + SEED + 1).min(r));
            (l, top(ft, sx0, sx1, SEED_FRAC), r, bottom(fb, sx0, sx1, SEED_FRAC))
        }
        // Around a box: its sides are the span (minus the corners we start from).
        None => (left(fl, ft + 1, fb - 1, SIDE_FRAC), top(ft, fl + 1, fr - 1, SIDE_FRAC), right(fr, ft + 1, fb - 1, SIDE_FRAC), bottom(fb, fl + 1, fr - 1, SIDE_FRAC)),
    };
    for _ in 0..6 {
        let (l, t, r, bo) = b;
        let mut l2 = left(fl, t, bo, SIDE_FRAC);
        let mut r2 = right(fr, t, bo, SIDE_FRAC);
        let mut t2 = top(ft, l2, r2, SIDE_FRAC);
        let mut b2 = bottom(fb, l2, r2, SIDE_FRAC);
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
    (r.w >= MIN_SIDE && r.h >= MIN_SIDE && r != bounds).then_some(r)
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
    fn busy_picture_in_a_frame_is_one_box() {
        // A game-style icon: hard vertical streaks inside, on a smooth dark frame.
        let mut pm = Pixmap::new(120, 80).unwrap();
        pm.fill(Color::from_rgba8(20, 20, 20, 255));
        let mut p = Paint::default();
        for x in 30..90 {
            let v = if (x * 7) % 5 < 2 { 230 } else { 90 + ((x * 13) % 40) as u8 };
            for y in 20..60 {
                // Grainy, so neither side of a streak is smooth along it.
                let w = if (x + y) % 2 == 0 { 0 } else { 50 };
                p.set_color_rgba8(v - w, v / 2, 40 + w, 255);
                pm.fill_rect(tiny_skia::Rect::from_xywh(x as f32, y as f32, 1.0, 1.0).unwrap(), &p, Transform::identity(), None);
            }
        }
        let all = Rect::new(0, 0, 120, 80);
        for (x, y) in [(60, 40), (45, 35), (75, 45)] {
            assert_eq!(box_at(&pm, x, y, all), Some(Rect::new(30, 20, 60, 40)), "at {x},{y}");
            assert_eq!(outer_box_at(&pm, x, y, all), Some(Rect::new(30, 20, 60, 40)), "at {x},{y}");
        }
    }

    #[test]
    fn steps_out_from_a_button_to_its_toolbar_and_panel() {
        // A window: grey panel, holding a darker toolbar, holding three buttons.
        let mut pm = Pixmap::new(300, 200).unwrap();
        pm.fill(Color::WHITE);
        let mut p = Paint::default();
        let mut fill = |pm: &mut Pixmap, rgb: (u8, u8, u8), x: f32, y: f32, w: f32, h: f32| {
            p.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
            pm.fill_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap(), &p, Transform::identity(), None);
        };
        fill(&mut pm, (225, 225, 225), 20.0, 20.0, 260.0, 160.0); // panel
        fill(&mut pm, (170, 170, 175), 30.0, 30.0, 240.0, 40.0); // toolbar
        for i in 0..3 {
            fill(&mut pm, (60, 110, 200), 40.0 + i as f32 * 50.0, 36.0, 40.0, 28.0); // buttons
        }
        let all = Rect::new(0, 0, 300, 200);
        let chain: Vec<Rect> = boxes_at(&pm, 105, 50, all).into_iter().filter(|(_, fine)| !fine).map(|(r, _)| r).collect();
        assert_eq!(chain, vec![Rect::new(90, 36, 40, 28), Rect::new(30, 30, 240, 40), Rect::new(20, 20, 260, 160)]);
    }

    #[test]
    fn blank_area_gives_nothing() {
        let mut pm = Pixmap::new(120, 80).unwrap();
        pm.fill(Color::WHITE);
        assert_eq!(box_at(&pm, 60, 40, Rect::new(0, 0, 120, 80)), None);
    }
}
