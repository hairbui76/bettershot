//! Geometry helpers shared by every backend.
//!
//! Two conventions are fixed here and relied on everywhere else:
//!
//! * **Half-open containment.** A rect owns the pixels in `[left, right) x
//!   [top, bottom)`. [`bettershot_core::Rect::contains`] is *closed* on both
//!   ends, which makes two side-by-side monitors both claim the pixel column on
//!   their shared edge. Hit-testing therefore uses [`contains_half_open`], so
//!   every point on the virtual desktop belongs to at most one monitor.
//! * **Physical pixels.** Every rect that reaches this module is in physical
//!   device pixels. See the crate-level docs for why.

use bettershot_core::{Rect, Vec2D};

use crate::CaptureError;

/// Does `rect` contain `point` under half-open (`[left, right)`) semantics?
///
/// This is the rule you want for pixel grids: a 1920-wide monitor at x=0 owns
/// columns 0..=1919, and x=1920 belongs to whatever sits to its right.
pub fn contains_half_open(rect: Rect, point: Vec2D) -> bool {
    let r = rect.normalized();
    point.x >= r.left() && point.x < r.right() && point.y >= r.top() && point.y < r.bottom()
}

/// Smallest rect covering all of `rects`, or `None` when the iterator is empty.
///
/// Empty (zero-area) rects still contribute their position, which matters for
/// disabled-but-present monitors reported at a real origin with a zero size.
pub fn bounding_box(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    let mut iter = rects.into_iter().map(|r| r.normalized());
    let first = iter.next()?;
    let (mut left, mut top, mut right, mut bottom) =
        (first.left(), first.top(), first.right(), first.bottom());
    for r in iter {
        left = left.min(r.left());
        top = top.min(r.top());
        right = right.max(r.right());
        bottom = bottom.max(r.bottom());
    }
    Some(Rect::from_xywh(left, top, right - left, bottom - top))
}

/// Clip `requested` to `bounds`.
///
/// Returns [`CaptureError::EmptyRegion`] when the request is degenerate before
/// clipping or does not overlap `bounds` at all — the caller asked for
/// something that cannot produce pixels, and silently returning a 0x0 image
/// would be worse than an error.
pub fn clamp_region(requested: Rect, bounds: Rect) -> Result<Rect, CaptureError> {
    // `f32::max(NaN, 0.0)` is 0.0 and `f32::min(NaN, w)` is w, so a NaN
    // rectangle would silently become a full-desktop capture instead of an
    // error. Reject it before the clamping can hide it.
    if !requested.pos.x.is_finite()
        || !requested.pos.y.is_finite()
        || !requested.size.x.is_finite()
        || !requested.size.y.is_finite()
    {
        return Err(CaptureError::EmptyRegion);
    }

    let requested = requested.normalized();
    if requested.is_empty() {
        return Err(CaptureError::EmptyRegion);
    }
    let clipped = requested.clamped_to(bounds);
    if clipped.is_empty() {
        return Err(CaptureError::EmptyRegion);
    }
    Ok(clipped)
}

/// An integer, non-negative pixel rectangle: what a backend actually asks the
/// OS for, once the floating-point selection has been resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelRect {
    /// Left edge, may be negative on a multi-monitor virtual desktop.
    pub x: i32,
    /// Top edge, may be negative on a multi-monitor virtual desktop.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl PixelRect {
    /// Construct directly from integer components.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Snap a float rect to the pixel grid.
    ///
    /// Each *edge* is rounded independently and the size derived from the
    /// rounded edges, so a rect that is already integral survives untouched and
    /// adjacent rects stay adjacent (rounding position and size separately can
    /// leave a one-pixel seam).
    ///
    /// Fails with [`CaptureError::EmptyRegion`] when the result would have zero
    /// area, and with [`CaptureError::InvalidFrame`] for non-finite or
    /// absurdly large input.
    pub fn from_rect(rect: Rect) -> Result<Self, CaptureError> {
        let r = rect.normalized();
        let edges = [r.left(), r.top(), r.right(), r.bottom()];
        if edges.iter().any(|e| !e.is_finite()) {
            return Err(CaptureError::invalid_frame(format!(
                "region has non-finite coordinates: {r:?}"
            )));
        }
        let limit = i32::MAX as f32;
        if edges.iter().any(|e| e.abs() > limit) {
            return Err(CaptureError::invalid_frame(format!(
                "region does not fit in i32 pixel coordinates: {r:?}"
            )));
        }

        let left = r.left().round() as i32;
        let top = r.top().round() as i32;
        let right = r.right().round() as i32;
        let bottom = r.bottom().round() as i32;
        let width = (right - left).max(0) as u32;
        let height = (bottom - top).max(0) as u32;
        if width == 0 || height == 0 {
            return Err(CaptureError::EmptyRegion);
        }
        Ok(Self::new(left, top, width, height))
    }

    /// Back to the float rect space used by `bettershot-core`.
    pub fn to_rect(self) -> Rect {
        Rect::from_xywh(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
    }

    /// Exclusive right edge.
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    /// Exclusive bottom edge.
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    /// Number of pixels, as `u64` so 8K-by-8K virtual desktops cannot overflow.
    pub fn pixel_count(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Same rect, shifted so `origin` becomes `(0, 0)`.
    pub fn relative_to(self, origin: Vec2D) -> Self {
        Self::new(
            self.x - origin.x.round() as i32,
            self.y - origin.y.round() as i32,
            self.width,
            self.height,
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_non_finite_region_is_rejected_rather_than_becoming_the_whole_desktop() {
        // f32::max/min silently swallow NaN, so without an explicit guard a
        // NaN rectangle clamped to the desktop returns the entire desktop —
        // capturing far more than was asked for.
        let bounds = Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0);
        for bad in [
            Rect::from_xywh(f32::NAN, f32::NAN, 100.0, 100.0),
            Rect::from_xywh(0.0, 0.0, f32::NAN, 100.0),
            Rect::from_xywh(f32::INFINITY, 0.0, 100.0, 100.0),
            Rect::from_xywh(0.0, 0.0, f32::NEG_INFINITY, 100.0),
        ] {
            assert!(
                clamp_region(bad, bounds).is_err(),
                "{bad:?} should not clamp to anything"
            );
        }
        // A sane region still works.
        assert!(clamp_region(Rect::from_xywh(10.0, 10.0, 50.0, 50.0), bounds).is_ok());
    }

    use super::*;

    #[test]
    fn half_open_containment_excludes_the_far_edges() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert!(contains_half_open(r, Vec2D::new(0.0, 0.0)));
        assert!(contains_half_open(r, Vec2D::new(9.999, 9.999)));
        assert!(!contains_half_open(r, Vec2D::new(10.0, 5.0)));
        assert!(!contains_half_open(r, Vec2D::new(5.0, 10.0)));
        assert!(!contains_half_open(r, Vec2D::new(-0.001, 5.0)));
    }

    #[test]
    fn adjacent_rects_never_both_claim_the_shared_edge() {
        let left = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let right = Rect::from_xywh(100.0, 0.0, 100.0, 100.0);
        let seam = Vec2D::new(100.0, 50.0);
        assert!(!contains_half_open(left, seam));
        assert!(contains_half_open(right, seam));
        // The closed-interval version in core would claim both, which is
        // exactly the ambiguity this helper exists to remove.
        assert!(left.contains(seam) && right.contains(seam));
    }

    #[test]
    fn bounding_box_of_nothing_is_none() {
        assert!(bounding_box(std::iter::empty()).is_none());
    }

    #[test]
    fn bounding_box_spans_negative_origins() {
        let bb = bounding_box([
            Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0),
            Rect::from_xywh(-1280.0, -200.0, 1280.0, 1024.0),
        ])
        .unwrap();
        assert_eq!(bb, Rect::from_xywh(-1280.0, -200.0, 3200.0, 1280.0));
    }

    #[test]
    fn bounding_box_covers_gaps() {
        let bb = bounding_box([
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            Rect::from_xywh(500.0, 0.0, 100.0, 100.0),
        ])
        .unwrap();
        assert_eq!(bb, Rect::from_xywh(0.0, 0.0, 600.0, 100.0));
    }

    #[test]
    fn clamp_region_clips_to_bounds() {
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let got = clamp_region(Rect::from_xywh(-20.0, 40.0, 80.0, 400.0), bounds).unwrap();
        assert_eq!(got, Rect::from_xywh(0.0, 40.0, 60.0, 60.0));
    }

    #[test]
    fn clamp_region_rejects_degenerate_requests() {
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        assert!(matches!(
            clamp_region(Rect::from_xywh(10.0, 10.0, 0.0, 50.0), bounds),
            Err(CaptureError::EmptyRegion)
        ));
    }

    #[test]
    fn clamp_region_rejects_fully_outside_requests() {
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        assert!(matches!(
            clamp_region(Rect::from_xywh(200.0, 200.0, 10.0, 10.0), bounds),
            Err(CaptureError::EmptyRegion)
        ));
        // Touching the far edge only is still empty under clipping.
        assert!(matches!(
            clamp_region(Rect::from_xywh(100.0, 0.0, 10.0, 10.0), bounds),
            Err(CaptureError::EmptyRegion)
        ));
    }

    #[test]
    fn clamp_region_normalizes_backwards_drags() {
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let dragged_up_left = Rect::from_xywh(80.0, 80.0, -60.0, -60.0);
        assert_eq!(
            clamp_region(dragged_up_left, bounds).unwrap(),
            Rect::from_xywh(20.0, 20.0, 60.0, 60.0)
        );
    }

    #[test]
    fn pixel_rect_rounds_edges_not_extents() {
        // Edge-wise rounding keeps neighbours seamless: 0.4..10.4 and
        // 10.4..20.4 must become 0..10 and 10..20, not 0..10 and 10..10.
        let a = PixelRect::from_rect(Rect::from_xywh(0.4, 0.0, 10.0, 5.0)).unwrap();
        let b = PixelRect::from_rect(Rect::from_xywh(10.4, 0.0, 10.0, 5.0)).unwrap();
        assert_eq!(a, PixelRect::new(0, 0, 10, 5));
        assert_eq!(b, PixelRect::new(10, 0, 10, 5));
        assert_eq!(a.right(), b.x);
    }

    #[test]
    fn pixel_rect_keeps_negative_origins() {
        let r = PixelRect::from_rect(Rect::from_xywh(-1280.0, -200.0, 1280.0, 1024.0)).unwrap();
        assert_eq!(r, PixelRect::new(-1280, -200, 1280, 1024));
        assert_eq!(r.right(), 0);
        assert_eq!(r.bottom(), 824);
        assert_eq!(r.pixel_count(), 1280 * 1024);
    }

    #[test]
    fn pixel_rect_rejects_subpixel_and_nonfinite_rects() {
        assert!(matches!(
            PixelRect::from_rect(Rect::from_xywh(0.0, 0.0, 0.2, 10.0)),
            Err(CaptureError::EmptyRegion)
        ));
        assert!(matches!(
            PixelRect::from_rect(Rect::from_xywh(0.0, 0.0, f32::NAN, 10.0)),
            Err(CaptureError::InvalidFrame(_))
        ));
        assert!(matches!(
            PixelRect::from_rect(Rect::from_xywh(0.0, 0.0, f32::INFINITY, 10.0)),
            Err(CaptureError::InvalidFrame(_))
        ));
    }

    #[test]
    fn pixel_rect_round_trips_through_rect() {
        let r = PixelRect::new(-5, 7, 30, 40);
        assert_eq!(PixelRect::from_rect(r.to_rect()).unwrap(), r);
    }

    #[test]
    fn relative_to_shifts_into_frame_local_space() {
        let r = PixelRect::new(100, 50, 20, 20);
        assert_eq!(
            r.relative_to(Vec2D::new(-1280.0, -200.0)),
            PixelRect::new(1380, 250, 20, 20)
        );
    }
}
