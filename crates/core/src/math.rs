//! 2D geometry primitives.
//!
//! Adapted from Satty (`src/math.rs`), MPL-2.0, Copyright the Satty authors.
//!
//! Every coordinate in this crate is in **image-pixel space**. View transforms
//! (zoom, pan, HiDPI scale) belong to the app shell and are applied only at the
//! render and input-translation boundary.

use std::{
    f32::consts::PI,
    fmt::Display,
    ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign},
};

use serde::{Deserialize, Serialize};

/// One 15° step, in radians. Used for shift-snapping lines and arrows.
const SNAP_STEP_RADIANS: f32 = PI / 12.0;

#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vec2D {
    pub x: f32,
    pub y: f32,
}

#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct Angle {
    pub radians: f32,
}

impl Angle {
    pub fn from_radians(radians: f32) -> Self {
        Self { radians }
    }

    pub fn from_degrees(degrees: f32) -> Self {
        Self {
            radians: degrees * PI / 180.0,
        }
    }

    pub fn cos(&self) -> f32 {
        self.radians.cos()
    }

    pub fn sin(&self) -> f32 {
        self.radians.sin()
    }
}

impl Mul<f32> for Angle {
    type Output = Angle;

    fn mul(self, rhs: f32) -> Self::Output {
        Angle::from_radians(self.radians * rhs)
    }
}

impl Vec2D {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub const fn zero() -> Self {
        Self::ZERO
    }

    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v }
    }

    pub fn norm(&self) -> f32 {
        self.norm2().sqrt()
    }

    pub fn norm2(&self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// Angle of the vector, where 0 is the positive x-axis and PI/2 the
    /// positive y-axis.
    pub fn angle(&self) -> Angle {
        Angle::from_radians(self.y.atan2(self.x))
    }

    pub fn from_angle(angle: Angle) -> Vec2D {
        Vec2D::new(angle.cos(), angle.sin())
    }

    /// Rotate around the origin.
    pub fn rotated(&self, angle: Angle) -> Vec2D {
        let (s, c) = (angle.sin(), angle.cos());
        Vec2D::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// The same vector snapped to the nearest multiple of 15°, preserving
    /// length. Used while holding Shift.
    pub fn snapped_vector_15deg(&self) -> Vec2D {
        if self.is_zero() {
            return *self;
        }
        let length = self.norm();
        let snapped = (self.angle().radians / SNAP_STEP_RADIANS).round() * SNAP_STEP_RADIANS;
        Vec2D::from_angle(Angle::from_radians(snapped)) * length
    }

    /// Snap to a square (equal absolute components), preserving each sign. Used
    /// while holding Shift with rectangle/ellipse tools.
    pub fn snapped_square(&self) -> Vec2D {
        let extent = self.x.abs().max(self.y.abs());
        Vec2D::new(
            extent * if self.x < 0.0 { -1.0 } else { 1.0 },
            extent * if self.y < 0.0 { -1.0 } else { 1.0 },
        )
    }

    pub fn is_zero(&self) -> bool {
        self.x.abs() < f32::EPSILON && self.y.abs() < f32::EPSILON
    }

    pub fn distance_to(&self, other: &Vec2D) -> f32 {
        (*self - *other).norm()
    }

    pub fn round(&self) -> Vec2D {
        Vec2D::new(self.x.round(), self.y.round())
    }

    pub fn clamp(&self, min: Vec2D, max: Vec2D) -> Vec2D {
        Vec2D::new(self.x.clamp(min.x, max.x), self.y.clamp(min.y, max.y))
    }
}

impl Add for Vec2D {
    type Output = Vec2D;
    fn add(self, rhs: Self) -> Self::Output {
        Vec2D::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2D {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs
    }
}

impl Sub for Vec2D {
    type Output = Vec2D;
    fn sub(self, rhs: Self) -> Self::Output {
        Vec2D::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2D {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2D {
    type Output = Vec2D;
    fn mul(self, rhs: f32) -> Self::Output {
        Vec2D::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2D {
    type Output = Vec2D;
    fn neg(self) -> Self::Output {
        Vec2D::new(-self.x, -self.y)
    }
}

impl Display for Vec2D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({},{})", self.x, self.y)
    }
}

impl From<(f32, f32)> for Vec2D {
    fn from((x, y): (f32, f32)) -> Self {
        Vec2D::new(x, y)
    }
}

/// An axis-aligned rectangle stored as origin + size. `size` is always
/// non-negative for rectangles produced by [`Rect::from_corners`] and
/// [`Rect::normalized`].
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub pos: Vec2D,
    pub size: Vec2D,
}

impl Rect {
    pub const fn new(pos: Vec2D, size: Vec2D) -> Self {
        Self { pos, size }
    }

    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::new(Vec2D::new(x, y), Vec2D::new(w, h))
    }

    /// Build a normalized rect spanning two arbitrary corners.
    pub fn from_corners(a: Vec2D, b: Vec2D) -> Self {
        Self::new(a, b - a).normalized()
    }

    /// Flip negative extents so `size` is non-negative and `pos` is the
    /// top-left corner.
    pub fn normalized(&self) -> Rect {
        let (x, w) = if self.size.x < 0.0 {
            (self.pos.x + self.size.x, -self.size.x)
        } else {
            (self.pos.x, self.size.x)
        };
        let (y, h) = if self.size.y < 0.0 {
            (self.pos.y + self.size.y, -self.size.y)
        } else {
            (self.pos.y, self.size.y)
        };
        Rect::from_xywh(x, y, w, h)
    }

    pub fn left(&self) -> f32 {
        self.pos.x
    }
    pub fn top(&self) -> f32 {
        self.pos.y
    }
    pub fn right(&self) -> f32 {
        self.pos.x + self.size.x
    }
    pub fn bottom(&self) -> f32 {
        self.pos.y + self.size.y
    }
    pub fn width(&self) -> f32 {
        self.size.x
    }
    pub fn height(&self) -> f32 {
        self.size.y
    }
    pub fn center(&self) -> Vec2D {
        self.pos + self.size * 0.5
    }
    pub fn top_left(&self) -> Vec2D {
        self.pos
    }
    pub fn top_right(&self) -> Vec2D {
        Vec2D::new(self.right(), self.top())
    }
    pub fn bottom_left(&self) -> Vec2D {
        Vec2D::new(self.left(), self.bottom())
    }
    pub fn bottom_right(&self) -> Vec2D {
        self.pos + self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size.x <= 0.0 || self.size.y <= 0.0
    }

    pub fn area(&self) -> f32 {
        (self.size.x * self.size.y).max(0.0)
    }

    pub fn contains(&self, p: Vec2D) -> bool {
        p.x >= self.left() && p.x <= self.right() && p.y >= self.top() && p.y <= self.bottom()
    }

    pub fn translated(&self, delta: Vec2D) -> Rect {
        Rect::new(self.pos + delta, self.size)
    }

    /// Grow (or, with a negative value, shrink) the rect on all sides.
    pub fn expanded(&self, amount: f32) -> Rect {
        Rect::new(
            self.pos - Vec2D::splat(amount),
            self.size + Vec2D::splat(amount * 2.0),
        )
    }

    /// Clamp this rect so it lies fully inside `bounds`. Returns an empty rect
    /// when the two do not overlap.
    pub fn clamped_to(&self, bounds: Rect) -> Rect {
        let me = self.normalized();
        let b = bounds.normalized();
        let left = me.left().max(b.left());
        let top = me.top().max(b.top());
        let right = me.right().min(b.right());
        let bottom = me.bottom().min(b.bottom());
        Rect::from_xywh(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
    }

    pub fn intersects(&self, other: Rect) -> bool {
        !self.clamped_to(other).is_empty()
    }

    pub fn rounded(&self) -> Rect {
        Rect::new(self.pos.round(), self.size.round())
    }

    /// The four corners, clockwise from the top-left.
    pub fn corners(&self) -> [Vec2D; 4] {
        [
            self.top_left(),
            self.top_right(),
            self.bottom_right(),
            self.bottom_left(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "{a} != {b}");
    }

    #[test]
    fn vec_arithmetic() {
        let a = Vec2D::new(3.0, 4.0);
        approx(a.norm(), 5.0);
        approx(a.norm2(), 25.0);
        assert_eq!(a + Vec2D::new(1.0, 1.0), Vec2D::new(4.0, 5.0));
        assert_eq!(a - a, Vec2D::ZERO);
        assert_eq!(a * 2.0, Vec2D::new(6.0, 8.0));
        assert_eq!(-a, Vec2D::new(-3.0, -4.0));
        approx(a.distance_to(&Vec2D::ZERO), 5.0);
    }

    #[test]
    fn snap_15deg_preserves_length_and_lands_on_multiples() {
        for degrees in [0.0f32, 7.0, 14.0, 44.0, 91.0, 179.0, -37.0, -100.0] {
            let v = Vec2D::from_angle(Angle::from_degrees(degrees)) * 10.0;
            let snapped = v.snapped_vector_15deg();
            approx(snapped.norm(), 10.0);
            let snapped_degrees = snapped.angle().radians * 180.0 / PI;
            let remainder = (snapped_degrees / 15.0).round() * 15.0 - snapped_degrees;
            approx(remainder, 0.0);
        }
    }

    #[test]
    fn snap_15deg_picks_the_nearest_step() {
        let v = Vec2D::from_angle(Angle::from_degrees(44.0)) * 5.0;
        let snapped = v.snapped_vector_15deg().angle().radians * 180.0 / PI;
        approx(snapped, 45.0);
    }

    #[test]
    fn snap_square_keeps_signs() {
        assert_eq!(
            Vec2D::new(10.0, -3.0).snapped_square(),
            Vec2D::new(10.0, -10.0)
        );
        assert_eq!(
            Vec2D::new(-2.0, 8.0).snapped_square(),
            Vec2D::new(-8.0, 8.0)
        );
    }

    #[test]
    fn zero_vector_snaps_to_itself() {
        assert!(Vec2D::ZERO.snapped_vector_15deg().is_zero());
    }

    #[test]
    fn rect_normalizes_negative_extents() {
        let r = Rect::from_xywh(10.0, 10.0, -4.0, -6.0).normalized();
        assert_eq!(r, Rect::from_xywh(6.0, 4.0, 4.0, 6.0));
    }

    #[test]
    fn rect_from_corners_is_order_independent() {
        let a = Rect::from_corners(Vec2D::new(5.0, 9.0), Vec2D::new(1.0, 2.0));
        let b = Rect::from_corners(Vec2D::new(1.0, 2.0), Vec2D::new(5.0, 9.0));
        assert_eq!(a, b);
        assert_eq!(a, Rect::from_xywh(1.0, 2.0, 4.0, 7.0));
    }

    #[test]
    fn clamp_to_bounds_clips_and_detects_disjoint() {
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let inside = Rect::from_xywh(-10.0, 50.0, 40.0, 200.0).clamped_to(bounds);
        assert_eq!(inside, Rect::from_xywh(0.0, 50.0, 30.0, 50.0));

        let disjoint = Rect::from_xywh(200.0, 200.0, 10.0, 10.0).clamped_to(bounds);
        assert!(disjoint.is_empty());
        assert!(!Rect::from_xywh(200.0, 200.0, 10.0, 10.0).intersects(bounds));
    }

    #[test]
    fn contains_and_center() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 20.0);
        assert!(r.contains(Vec2D::new(5.0, 5.0)));
        assert!(!r.contains(Vec2D::new(11.0, 5.0)));
        assert_eq!(r.center(), Vec2D::new(5.0, 10.0));
    }

    #[test]
    fn rotation_by_90_degrees() {
        let v = Vec2D::new(1.0, 0.0).rotated(Angle::from_degrees(90.0));
        approx(v.x, 0.0);
        approx(v.y, 1.0);
    }
}
