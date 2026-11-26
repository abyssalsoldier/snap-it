use serde::{Deserialize, Serialize};

/// Integer pixel rectangle (x, y = top-left).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    /// Normalised rect spanning two corner points (inclusive of both pixels' area).
    pub fn from_points(ax: i32, ay: i32, bx: i32, by: i32) -> Self {
        let x = ax.min(bx);
        let y = ay.min(by);
        Self { x, y, w: (ax - bx).abs(), h: (ay - by).abs() }
    }
    pub fn from_ltrb(l: i32, t: i32, r: i32, b: i32) -> Self {
        Self { x: l, y: t, w: r - l, h: b - t }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    /// Is `o` entirely inside this rectangle?
    pub fn contains_rect(&self, o: Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
    pub fn area(&self) -> i64 {
        self.w.max(0) as i64 * self.h.max(0) as i64
    }
    pub fn intersect(&self, o: Rect) -> Option<Rect> {
        let l = self.x.max(o.x);
        let t = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > l && b > t).then(|| Rect::from_ltrb(l, t, r, b))
    }
    pub fn union(&self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_ltrb(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn inflate(&self, d: i32) -> Rect {
        Rect { x: self.x - d, y: self.y - d, w: self.w + 2 * d, h: self.h + 2 * d }
    }
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    pub fn to_skia(&self) -> Option<tiny_skia::Rect> {
        tiny_skia::Rect::from_xywh(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

impl PointF {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: PointF) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn offset(self, dx: f32, dy: f32) -> PointF {
        PointF::new(self.x + dx, self.y + dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = Rect::from_points(10, 10, 0, 0);
        assert_eq!(a, Rect::new(0, 0, 10, 10));
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 15));
        assert!(a.intersect(Rect::new(20, 20, 1, 1)).is_none());
        assert!(a.contains(0, 0) && !a.contains(10, 10));
    }
}
