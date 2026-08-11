//! The committed-annotation trait.
//!
//! A [`Drawable`] is an immutable record of one finished annotation. It knows
//! how to paint itself and where it lives, but nothing about tools, input or
//! the renderer. Tools produce drawables; the [`crate::scene::Scene`] owns them
//! and replays them on every frame and on export.

use std::fmt::Debug;

use crate::math::{Rect, Vec2D};
use crate::painter::Painter;

pub trait Drawable: DrawableClone + Debug {
    /// Paint this annotation. Called on every frame and again at export, so it
    /// must be free of side effects.
    fn draw(&self, painter: &mut dyn Painter);

    /// Bounding box in image space, used for hit-testing and for deciding
    /// whether an annotation survives a crop. `None` means "unbounded" and is
    /// only appropriate for full-canvas effects.
    fn bounds(&self) -> Option<Rect>;

    /// A stable name for logging and tests.
    fn kind(&self) -> &'static str;

    /// Move the annotation. Used by post-paint editing and by crop, which
    /// rebases every annotation onto the new origin.
    fn translate(&mut self, delta: Vec2D);

    /// The sequence number this annotation displays, if it shows one.
    ///
    /// Only numbered markers do. The scene uses it to work out what the next
    /// marker should be called; deriving that from a *count* instead produces
    /// duplicates the moment one is deleted from the middle.
    fn sequence_number(&self) -> Option<u16> {
        None
    }

    /// Whether the point hits this annotation, for post-paint selection.
    /// Defaults to a bounding-box test, which is right for most shapes.
    fn hit_test(&self, point: Vec2D) -> bool {
        self.bounds()
            .is_some_and(|b| b.expanded(HIT_TOLERANCE).contains(point))
    }
}

/// Slack in image pixels around an annotation's bounds when hit-testing, so
/// thin strokes remain clickable.
pub const HIT_TOLERANCE: f32 = 6.0;

/// Object-safe `Clone` for `Box<dyn Drawable>`.
///
/// Needed because tools hand a copy of their in-progress shape to the scene on
/// commit, and because crop clones the annotation list while rebasing it.
pub trait DrawableClone {
    fn clone_box(&self) -> Box<dyn Drawable>;
}

impl<T> DrawableClone for T
where
    T: 'static + Drawable + Clone,
{
    fn clone_box(&self) -> Box<dyn Drawable> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Drawable> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Bounding box helper for drawables built from a point cloud, widened by half
/// the stroke width so the stroke itself is included.
pub fn stroke_bounds(points: &[Vec2D], stroke_width: f32) -> Option<Rect> {
    let mut iter = points.iter().copied();
    let first = iter.next()?;
    let (mut min, mut max) = (first, first);
    for p in iter {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Some(Rect::new(min, max - min).expanded(stroke_width / 2.0))
}

/// Distance from `point` to the segment `a`–`b`. Used by line-like drawables
/// for precise hit-testing.
pub fn distance_to_segment(point: Vec2D, a: Vec2D, b: Vec2D) -> f32 {
    let ab = b - a;
    let len2 = ab.norm2();
    if len2 <= f32::EPSILON {
        return point.distance_to(&a);
    }
    let t = (((point - a).x * ab.x + (point - a).y * ab.y) / len2).clamp(0.0, 1.0);
    point.distance_to(&(a + ab * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_bounds_include_half_the_stroke_width() {
        let b = stroke_bounds(&[Vec2D::new(0.0, 0.0), Vec2D::new(10.0, 10.0)], 4.0).unwrap();
        assert_eq!(b, Rect::from_xywh(-2.0, -2.0, 14.0, 14.0));
    }

    #[test]
    fn stroke_bounds_of_nothing_is_none() {
        assert!(stroke_bounds(&[], 1.0).is_none());
    }

    #[test]
    fn distance_to_segment_handles_ends_and_middle() {
        let a = Vec2D::new(0.0, 0.0);
        let b = Vec2D::new(10.0, 0.0);
        assert!((distance_to_segment(Vec2D::new(5.0, 3.0), a, b) - 3.0).abs() < 1e-4);
        // Beyond an endpoint clamps to that endpoint.
        assert!((distance_to_segment(Vec2D::new(-4.0, 0.0), a, b) - 4.0).abs() < 1e-4);
        assert!((distance_to_segment(Vec2D::new(14.0, 0.0), a, b) - 4.0).abs() < 1e-4);
    }

    #[test]
    fn distance_to_a_degenerate_segment_is_distance_to_the_point() {
        let a = Vec2D::new(3.0, 4.0);
        assert!((distance_to_segment(Vec2D::ZERO, a, a) - 5.0).abs() < 1e-4);
    }
}
