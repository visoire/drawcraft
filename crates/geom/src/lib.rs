//! L0 geometry primitives for DrawCraft.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use serde::{Deserialize, Serialize};

/// A 2D point in document (page) or screen space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    #[inline]
    pub fn sanitized(self) -> Self {
        Self {
            x: if self.x.is_finite() { self.x } else { 0.0 },
            y: if self.y.is_finite() { self.y } else { 0.0 },
        }
    }

    #[inline]
    pub fn distance_sq(self, other: Self) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        dx * dx + dy * dy
    }

    #[inline]
    pub fn distance(self, other: Self) -> f64 {
        self.distance_sq(other).sqrt()
    }

    #[inline]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
}

/// A 2D displacement / direction vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn from_points(a: Point, b: Point) -> Self {
        Self {
            x: b.x - a.x,
            y: b.y - a.y,
        }
    }

    #[inline]
    pub fn len_sq(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    #[inline]
    pub fn len(self) -> f64 {
        self.len_sq().sqrt()
    }

    #[inline]
    pub fn normalized(self) -> Option<Self> {
        let l = self.len();
        if l.is_finite() && l > 1e-12 {
            Some(Self {
                x: self.x / l,
                y: self.y / l,
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    #[inline]
    pub const fn perp(self) -> Self {
        Self {
            x: -self.y,
            y: self.x,
        }
    }

    #[inline]
    pub fn angle_rad(self) -> f64 {
        self.y.atan2(self.x)
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }

    #[inline]
    pub fn scale(self, s: f64) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
        }
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

/// An axis-aligned floating-point bounding rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const EMPTY: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    #[inline]
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
            return Self::EMPTY;
        }
        Self {
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }

    #[inline]
    pub fn from_points(a: Point, b: Point) -> Self {
        if !(a.is_finite() && b.is_finite()) {
            return Self::EMPTY;
        }
        let min_x = a.x.min(b.x);
        let min_y = a.y.min(b.y);
        let max_x = a.x.max(b.x);
        let max_y = a.y.max(b.y);
        Self {
            x: min_x,
            y: min_y,
            w: (max_x - min_x).max(0.0),
            h: (max_y - min_y).max(0.0),
        }
    }

    pub fn from_point_iter(points: impl IntoIterator<Item = Point>) -> Self {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;
        for p in points {
            if !p.is_finite() {
                continue;
            }
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
            any = true;
        }
        if !any {
            return Self::EMPTY;
        }
        Self {
            x: min_x,
            y: min_y,
            w: (max_x - min_x).max(0.0),
            h: (max_y - min_y).max(0.0),
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    #[inline]
    pub fn right(self) -> f64 {
        self.x + self.w
    }

    #[inline]
    pub fn bottom(self) -> f64 {
        self.y + self.h
    }

    #[inline]
    pub fn union(self, other: Self) -> Self {
        if self.w <= 0.0 && self.h <= 0.0 && self.x == 0.0 && self.y == 0.0 {
            return other;
        }
        if other.w <= 0.0 && other.h <= 0.0 && other.x == 0.0 && other.y == 0.0 {
            return self;
        }
        let min_x = self.x.min(other.x);
        let min_y = self.y.min(other.y);
        let max_x = self.right().max(other.right());
        let max_y = self.bottom().max(other.bottom());
        Self {
            x: min_x,
            y: min_y,
            w: (max_x - min_x).max(0.0),
            h: (max_y - min_y).max(0.0),
        }
    }

    #[inline]
    pub fn inset(self, dx: f64, dy: f64) -> Self {
        if !(dx.is_finite() && dy.is_finite()) {
            return self;
        }
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - 2.0 * dx).max(0.0),
            h: (self.h - 2.0 * dy).max(0.0),
        }
    }

    #[inline]
    pub fn inflated(self, pad: f64) -> Self {
        if !pad.is_finite() {
            return self;
        }
        let p = pad.max(0.0);
        Self {
            x: self.x - p,
            y: self.y - p,
            w: self.w + 2.0 * p,
            h: self.h + 2.0 * p,
        }
    }

    #[inline]
    pub fn intersects(self, other: Self) -> bool {
        self.x <= other.right()
            && self.right() >= other.x
            && self.y <= other.bottom()
            && self.bottom() >= other.y
    }

    #[inline]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }
}

/// An integer pixel rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    #[inline]
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        if w <= 0 || h <= 0 {
            Self { x, y, w: 0, h: 0 }
        } else {
            Self { x, y, w, h }
        }
    }

    #[inline]
    pub fn from_bounds(min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> Self {
        if max_x <= min_x || max_y <= min_y {
            Self::EMPTY
        } else {
            Self {
                x: min_x,
                y: min_y,
                w: max_x.saturating_sub(min_x),
                h: max_y.saturating_sub(min_y),
            }
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[inline]
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    #[inline]
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    #[inline]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let min_x = self.x.min(other.x);
        let min_y = self.y.min(other.y);
        let max_x = self.right().max(other.right());
        let max_y = self.bottom().max(other.bottom());
        Self::from_bounds(min_x, min_y, max_x, max_y)
    }

    #[inline]
    pub fn intersection(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::EMPTY;
        }
        let min_x = self.x.max(other.x);
        let min_y = self.y.max(other.y);
        let max_x = self.right().min(other.right());
        let max_y = self.bottom().min(other.bottom());
        Self::from_bounds(min_x, min_y, max_x, max_y)
    }

    #[inline]
    pub fn contains(self, x: i32, y: i32) -> bool {
        !self.is_empty() && x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Isotropic 2D affine view transform mapping document page space to screen space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine2 {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

impl Default for Affine2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine2 {
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };

    #[inline]
    pub fn new(scale: f64, offset_x: f64, offset_y: f64) -> Self {
        let s = if scale.is_finite() && scale > 1e-6 {
            scale
        } else {
            1.0
        };
        let ox = if offset_x.is_finite() { offset_x } else { 0.0 };
        let oy = if offset_y.is_finite() { offset_y } else { 0.0 };
        Self {
            scale: s,
            offset_x: ox,
            offset_y: oy,
        }
    }

    #[inline]
    pub fn to_screen(self, p: Point) -> Point {
        Point {
            x: p.x * self.scale + self.offset_x,
            y: p.y * self.scale + self.offset_y,
        }
    }

    #[inline]
    pub fn to_page(self, p: Point) -> Point {
        let inv = if self.scale.is_finite() && self.scale > 1e-9 {
            1.0 / self.scale
        } else {
            1.0
        };
        Point {
            x: (p.x - self.offset_x) * inv,
            y: (p.y - self.offset_y) * inv,
        }
    }

    pub fn zoom_around(
        &mut self,
        screen_anchor: Point,
        factor: f64,
        min_scale: f64,
        max_scale: f64,
    ) {
        if !(screen_anchor.is_finite() && factor.is_finite() && factor > 0.0) {
            return;
        }
        let page_pt = self.to_page(screen_anchor);
        let lo = min_scale.max(0.05);
        let hi = max_scale.max(lo);
        let next_scale = (self.scale * factor).clamp(lo, hi);
        self.scale = next_scale;
        self.offset_x = screen_anchor.x - page_pt.x * next_scale;
        self.offset_y = screen_anchor.y - page_pt.y * next_scale;
    }
}

/// Shortest distance from `p` to segment `[a, b]` and the clamped projection parameter `t` in `0..=1`.
#[inline]
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> (f64, f64) {
    if !(p.is_finite() && a.is_finite() && b.is_finite()) {
        return (f64::INFINITY, 0.0);
    }
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let l2 = dx * dx + dy * dy;
    if !l2.is_finite() || l2 <= 1e-18 {
        return (p.distance(a), 0.0);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0);
    let proj = Point::new(a.x + dx * t, a.y + dy * t);
    (p.distance(proj), t)
}

#[inline]
fn knot_interp(a: Point, b: Point, ta: f64, tb: f64, t: f64) -> Point {
    let span = tb - ta;
    if !span.is_finite() || span.abs() <= 1e-12 {
        a
    } else {
        a.lerp(b, (t - ta) / span)
    }
}

/// Evaluates the Centripetal Catmull-Rom ($\alpha = 0.5$) spline segment between `p1` and `p2`
/// at `u` in `0.0..=1.0` using the Barry-Goldman pyramidal formulation.
pub fn catmull_rom_centripetal_point(p0: Point, p1: Point, p2: Point, p3: Point, u: f64) -> Point {
    let u = if u.is_finite() {
        u.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if !(p0.is_finite() && p1.is_finite() && p2.is_finite() && p3.is_finite()) {
        return p1.sanitized();
    }
    let dt01 = p0.distance(p1).sqrt().max(1e-4);
    let dt12 = p1.distance(p2).sqrt().max(1e-4);
    let dt23 = p2.distance(p3).sqrt().max(1e-4);
    let t0 = 0.0;
    let t1 = dt01;
    let t2 = t1 + dt12;
    let t3 = t2 + dt23;
    let t = t1 + u * (t2 - t1);

    let a1 = knot_interp(p0, p1, t0, t1, t);
    let a2 = knot_interp(p1, p2, t1, t2, t);
    let a3 = knot_interp(p2, p3, t2, t3, t);
    let b1 = knot_interp(a1, a2, t0, t2, t);
    let b2 = knot_interp(a2, a3, t1, t3, t);
    knot_interp(b1, b2, t1, t2, t)
}

/// Evaluates Centripetal Catmull-Rom with a configurable tension factor in `0.0..=1.0`
/// (`0.5` = exact centripetal Catmull-Rom; `0.0` = tighter toward chord; `1.0` = fuller curve).
pub fn catmull_rom_centripetal_with_tension(
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    u: f64,
    tension: f64,
) -> Point {
    let u = if u.is_finite() {
        u.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let c = catmull_rom_centripetal_point(p0, p1, p2, p3, u);
    let t = if tension.is_finite() {
        tension.clamp(0.0, 1.0)
    } else {
        0.5
    };
    if (t - 0.5).abs() < 1e-6 {
        return c;
    }
    let chord = p1.sanitized().lerp(p2.sanitized(), u);
    let blend = (0.5 + t).clamp(0.0, 1.5);
    chord.lerp(c, blend)
}

/// Iterative (non-recursive) Ramer-Douglas-Peucker polyline simplification returning kept indices.
pub fn rdp_indices(points: &[Point], epsilon: f64) -> Vec<usize> {
    let n = points.len();
    if n <= 2 || !epsilon.is_finite() || epsilon <= 0.0 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(64);
    stack.push((0, n - 1));

    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let a = points[start];
        let b = points[end];
        let mut max_dist = -1.0;
        let mut max_idx = start;

        for (i, &p) in points.iter().enumerate().take(end).skip(start + 1) {
            let (d, _) = distance_to_segment(p, a, b);
            if d.is_finite() && d > max_dist {
                max_dist = d;
                max_idx = i;
            }
        }

        if max_dist > epsilon && max_idx > start && max_idx < end {
            keep[max_idx] = true;
            if stack.len() < 4096 {
                stack.push((start, max_idx));
                stack.push((max_idx, end));
            }
        }
    }

    keep.iter()
        .enumerate()
        .filter_map(|(i, &k)| k.then_some(i))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance_handles_degenerate_and_endpoints() {
        let a = Point::new(10.0, 10.0);
        let b = Point::new(10.0, 10.0);
        let (d, t) = distance_to_segment(Point::new(13.0, 14.0), a, b);
        assert!((d - 5.0).abs() < 1e-9);
        assert_eq!(t, 0.0);

        let (d2, t2) = distance_to_segment(
            Point::new(5.0, 4.0),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
        );
        assert!((d2 - 4.0).abs() < 1e-9);
        assert!((t2 - 0.5).abs() < 1e-9);

        let (d3, t3) = distance_to_segment(
            Point::new(-3.0, 4.0),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
        );
        assert!((d3 - 5.0).abs() < 1e-9);
        assert_eq!(t3, 0.0);
    }

    #[test]
    fn catmull_rom_handles_coincident_and_nan_knots() {
        let p = Point::new(42.0, 24.0);
        let res = catmull_rom_centripetal_point(p, p, p, p, 0.5);
        assert!((res.x - 42.0).abs() < 1e-9 && (res.y - 24.0).abs() < 1e-9);

        let p0 = Point::new(0.0, 0.0);
        let p1 = Point::new(10.0, 0.0);
        let p2 = Point::new(20.0, 10.0);
        let p3 = Point::new(30.0, 10.0);
        let mid = catmull_rom_centripetal_point(p0, p1, p2, p3, 0.5);
        assert!(mid.is_finite());
        assert!((mid.x - 15.0).abs() < 1.5);
        assert!((mid.y - 5.0).abs() < 1.5);

        let start = catmull_rom_centripetal_point(p0, p1, p2, p3, 0.0);
        let end = catmull_rom_centripetal_point(p0, p1, p2, p3, 1.0);
        assert!((start.x - p1.x).abs() < 1e-9 && (start.y - p1.y).abs() < 1e-9);
        assert!((end.x - p2.x).abs() < 1e-9 && (end.y - p2.y).abs() < 1e-9);

        let nan_res = catmull_rom_centripetal_point(
            Point::new(f64::NAN, 0.0),
            Point::new(5.0, 6.0),
            p2,
            p3,
            f64::NAN,
        );
        assert!(nan_res.is_finite());
    }

    #[test]
    fn affine_zoom_round_trip_preserves_anchor() {
        let mut xf = Affine2::new(1.5, 120.0, 80.0);
        let anchor = Point::new(400.0, 300.0);
        let page_before = xf.to_page(anchor);
        xf.zoom_around(anchor, 2.0, 0.2, 10.0);
        let page_after = xf.to_page(anchor);
        assert!((page_before.x - page_after.x).abs() < 1e-9);
        assert!((page_before.y - page_after.y).abs() < 1e-9);
        assert!((xf.scale - 3.0).abs() < 1e-9);
    }

    #[test]
    fn rdp_simplifies_collinear_points() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.1),
            Point::new(10.0, 0.0),
            Point::new(20.0, 15.0),
            Point::new(30.0, 15.0),
        ];
        let kept = rdp_indices(&pts, 0.5);
        assert_eq!(kept, vec![0, 2, 3, 4]);
    }
}
