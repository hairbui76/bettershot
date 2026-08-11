//! The image-space ⇄ screen-space transform.
//!
//! This is the boundary the whole architecture depends on: annotations live in
//! image-pixel coordinates and know nothing about zoom, pan or HiDPI, so every
//! conversion happens here and nowhere else. Getting it wrong is the classic
//! bug in this kind of program (annotations that drift when you zoom, or land
//! offset on export), which is why it is a small, pure, heavily tested struct
//! rather than arithmetic scattered through the paint code.
//!
//! The transform is `screen = image * zoom + origin`, where `origin` is the
//! screen position of image pixel (0, 0).

use bettershot_core::math::{Rect, Vec2D};

/// Zoom limits. Below the minimum the image is unusably small; above the
/// maximum egui's tessellator starts producing enormous meshes.
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 32.0;
/// Multiplier per zoom step (mouse wheel notch or keyboard).
pub const ZOOM_STEP: f32 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Screen pixels per image pixel.
    zoom: f32,
    /// Screen position of image pixel (0, 0).
    origin: Vec2D,
    /// Size of the image being displayed.
    image_size: Vec2D,
    /// The screen rectangle the image is drawn into.
    viewport: Rect,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            origin: Vec2D::ZERO,
            image_size: Vec2D::ZERO,
            viewport: Rect::default(),
        }
    }
}

impl View {
    pub fn new(image_size: Vec2D, viewport: Rect) -> Self {
        let mut view = Self {
            zoom: 1.0,
            origin: Vec2D::ZERO,
            image_size,
            viewport,
        };
        view.fit();
        view
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn image_size(&self) -> Vec2D {
        self.image_size
    }

    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    /// Update the drawing area, keeping the view centred on whatever it was
    /// looking at. Called on every window resize.
    pub fn set_viewport(&mut self, viewport: Rect) {
        if self.viewport.size.is_zero() || viewport.size.is_zero() {
            self.viewport = viewport;
            self.fit();
            return;
        }
        let previous_centre = self.screen_to_image(self.viewport.center());
        self.viewport = viewport;
        self.center_on(previous_centre);
    }

    /// Swap the image, e.g. after a crop, and re-fit.
    pub fn set_image_size(&mut self, image_size: Vec2D) {
        self.image_size = image_size;
        self.fit();
    }

    // --- conversions ------------------------------------------------------

    pub fn image_to_screen(&self, p: Vec2D) -> Vec2D {
        p * self.zoom + self.origin
    }

    pub fn screen_to_image(&self, p: Vec2D) -> Vec2D {
        if self.zoom.abs() < f32::EPSILON {
            return Vec2D::ZERO;
        }
        (p - self.origin) * (1.0 / self.zoom)
    }

    pub fn image_rect_to_screen(&self, r: Rect) -> Rect {
        Rect::new(self.image_to_screen(r.pos), r.size * self.zoom)
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "part of the view API, covered by tests")
    )]
    pub fn screen_rect_to_image(&self, r: Rect) -> Rect {
        Rect::new(self.screen_to_image(r.pos), r.size * (1.0 / self.zoom))
    }

    /// Scale an image-space length (a stroke width) into screen pixels.
    pub fn scale_length(&self, length: f32) -> f32 {
        length * self.zoom
    }

    /// Where the whole image lands on screen.
    pub fn image_screen_rect(&self) -> Rect {
        self.image_rect_to_screen(Rect::new(Vec2D::ZERO, self.image_size))
    }

    // --- navigation -------------------------------------------------------

    /// Scale the image to fit the viewport and centre it. Never enlarges past
    /// 1:1, so a small screenshot is shown at native size rather than blown up
    /// and blurry.
    pub fn fit(&mut self) {
        if self.image_size.x <= 0.0
            || self.image_size.y <= 0.0
            || self.viewport.size.x <= 0.0
            || self.viewport.size.y <= 0.0
        {
            self.zoom = 1.0;
            self.origin = self.viewport.pos;
            return;
        }
        let scale = (self.viewport.size.x / self.image_size.x)
            .min(self.viewport.size.y / self.image_size.y)
            .min(1.0);
        self.zoom = scale.clamp(MIN_ZOOM, MAX_ZOOM);
        self.center();
    }

    /// Show the image at exactly one screen pixel per image pixel.
    pub fn zoom_to_actual_size(&mut self) {
        let centre = self.screen_to_image(self.viewport.center());
        self.zoom = 1.0;
        self.center_on(centre);
    }

    /// Centre the image in the viewport.
    pub fn center(&mut self) {
        let scaled = self.image_size * self.zoom;
        self.origin = self.viewport.pos + (self.viewport.size - scaled) * 0.5;
    }

    /// Put the given image point at the centre of the viewport.
    pub fn center_on(&mut self, image_point: Vec2D) {
        self.origin = self.viewport.center() - image_point * self.zoom;
    }

    pub fn pan(&mut self, screen_delta: Vec2D) {
        self.origin += screen_delta;
    }

    /// Zoom by `factor`, keeping the image point currently under `anchor`
    /// (a screen position, normally the pointer) pinned in place. This is what
    /// makes wheel-zoom feel right.
    pub fn zoom_by(&mut self, factor: f32, anchor: Vec2D) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - self.zoom).abs() < f32::EPSILON {
            return;
        }
        // Solve image_to_screen(anchor_image) == anchor for the new origin.
        let anchor_image = self.screen_to_image(anchor);
        self.zoom = new_zoom;
        self.origin = anchor - anchor_image * new_zoom;
    }

    /// Zoom one notch in or out, anchored at `anchor`.
    pub fn zoom_step(&mut self, steps: f32, anchor: Vec2D) {
        self.zoom_by(ZOOM_STEP.powf(steps), anchor);
    }

    /// Stop the image being dragged entirely out of sight. A margin of the
    /// image always stays within the viewport.
    pub fn clamp_to_viewport(&mut self) {
        const KEEP_VISIBLE: f32 = 32.0;
        let image = self.image_screen_rect();
        let v = self.viewport;
        if image.size.x <= 0.0 || image.size.y <= 0.0 || v.size.x <= 0.0 {
            return;
        }
        let min_x = v.left() + KEEP_VISIBLE - image.width();
        let max_x = v.right() - KEEP_VISIBLE;
        let min_y = v.top() + KEEP_VISIBLE - image.height();
        let max_y = v.bottom() - KEEP_VISIBLE;
        self.origin.x = self.origin.x.clamp(min_x.min(max_x), max_x.max(min_x));
        self.origin.y = self.origin.y.clamp(min_y.min(max_y), max_y.max(min_y));
    }

    /// Whether the image is entirely visible at the current zoom.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "part of the view API, covered by tests")
    )]
    pub fn fits_in_viewport(&self) -> bool {
        let image = self.image_screen_rect();
        image.width() <= self.viewport.width() + 0.5
            && image.height() <= self.viewport.height() + 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View::new(
            Vec2D::new(1000.0, 500.0),
            Rect::from_xywh(0.0, 0.0, 500.0, 500.0),
        )
    }

    fn approx(a: Vec2D, b: Vec2D) {
        assert!(
            (a.x - b.x).abs() < 1e-2 && (a.y - b.y).abs() < 1e-2,
            "{a} != {b}"
        );
    }

    #[test]
    fn conversions_round_trip() {
        let v = view();
        for p in [
            Vec2D::ZERO,
            Vec2D::new(123.0, 456.0),
            Vec2D::new(-40.0, 900.0),
        ] {
            approx(v.screen_to_image(v.image_to_screen(p)), p);
        }
    }

    #[test]
    fn fitting_scales_down_and_centres() {
        let v = view();
        // A 1000x500 image in a 500x500 viewport fits at 0.5.
        assert!((v.zoom() - 0.5).abs() < 1e-4, "zoom was {}", v.zoom());
        let on_screen = v.image_screen_rect();
        assert!((on_screen.width() - 500.0).abs() < 1e-2);
        assert!((on_screen.height() - 250.0).abs() < 1e-2);
        // Centred vertically in the leftover space.
        assert!((on_screen.center().y - 250.0).abs() < 1e-2);
        assert!((on_screen.center().x - 250.0).abs() < 1e-2);
    }

    #[test]
    fn fitting_never_enlarges_a_small_image() {
        let v = View::new(
            Vec2D::new(100.0, 100.0),
            Rect::from_xywh(0.0, 0.0, 1000.0, 1000.0),
        );
        assert_eq!(v.zoom(), 1.0, "a small image should stay at 1:1");
        approx(v.image_screen_rect().center(), Vec2D::new(500.0, 500.0));
    }

    #[test]
    fn zooming_keeps_the_anchor_point_pinned() {
        let mut v = view();
        let anchor = Vec2D::new(300.0, 200.0);
        let before = v.screen_to_image(anchor);
        v.zoom_by(2.5, anchor);
        let after = v.screen_to_image(anchor);
        approx(before, after);
    }

    #[test]
    fn repeated_zoom_steps_stay_anchored() {
        let mut v = view();
        let anchor = Vec2D::new(137.0, 401.0);
        let before = v.screen_to_image(anchor);
        for _ in 0..20 {
            v.zoom_step(1.0, anchor);
        }
        for _ in 0..25 {
            v.zoom_step(-1.0, anchor);
        }
        approx(v.screen_to_image(anchor), before);
    }

    #[test]
    fn zoom_is_clamped_at_both_ends() {
        let mut v = view();
        for _ in 0..500 {
            v.zoom_step(1.0, Vec2D::new(250.0, 250.0));
        }
        assert!(v.zoom() <= MAX_ZOOM + 1e-4, "zoom ran away to {}", v.zoom());

        for _ in 0..2000 {
            v.zoom_step(-1.0, Vec2D::new(250.0, 250.0));
        }
        assert!(
            v.zoom() >= MIN_ZOOM - 1e-6,
            "zoom collapsed to {}",
            v.zoom()
        );
    }

    #[test]
    fn a_nonsense_zoom_factor_is_ignored() {
        let mut v = view();
        let before = v;
        v.zoom_by(0.0, Vec2D::ZERO);
        v.zoom_by(-1.0, Vec2D::ZERO);
        v.zoom_by(f32::NAN, Vec2D::ZERO);
        v.zoom_by(f32::INFINITY, Vec2D::ZERO);
        assert_eq!(v.zoom(), before.zoom());
    }

    #[test]
    fn panning_moves_the_image_by_exactly_the_screen_delta() {
        let mut v = view();
        let before = v.image_to_screen(Vec2D::new(10.0, 10.0));
        v.pan(Vec2D::new(25.0, -15.0));
        approx(
            v.image_to_screen(Vec2D::new(10.0, 10.0)),
            before + Vec2D::new(25.0, -15.0),
        );
    }

    #[test]
    fn stroke_widths_scale_with_zoom() {
        let mut v = view();
        v.zoom_by(4.0 / v.zoom(), Vec2D::ZERO);
        assert!((v.scale_length(5.0) - 20.0).abs() < 1e-3);
    }

    #[test]
    fn actual_size_shows_one_screen_pixel_per_image_pixel() {
        let mut v = view();
        v.zoom_to_actual_size();
        assert_eq!(v.zoom(), 1.0);
        let r = v.image_screen_rect();
        assert!((r.width() - 1000.0).abs() < 1e-2);
    }

    #[test]
    fn resizing_the_viewport_keeps_the_same_image_point_centred() {
        let mut v = view();
        v.zoom_by(3.0, Vec2D::new(250.0, 250.0));
        let centred_before = v.screen_to_image(v.viewport().center());

        v.set_viewport(Rect::from_xywh(0.0, 0.0, 800.0, 600.0));
        let centred_after = v.screen_to_image(v.viewport().center());
        approx(centred_before, centred_after);
        assert_eq!(v.zoom(), 3.0 * 0.5, "resize must not change zoom");
    }

    #[test]
    fn a_degenerate_viewport_does_not_produce_nan() {
        let mut v = View::new(Vec2D::new(100.0, 100.0), Rect::default());
        v.fit();
        assert!(v.zoom().is_finite() && v.zoom() > 0.0);
        assert!(v.image_to_screen(Vec2D::ZERO).x.is_finite());

        let mut v = View::new(Vec2D::ZERO, Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
        v.fit();
        assert!(v.zoom().is_finite());
        assert!(v.screen_to_image(Vec2D::new(5.0, 5.0)).x.is_finite());
    }

    #[test]
    fn clamping_keeps_part_of_the_image_on_screen() {
        let mut v = view();
        v.pan(Vec2D::new(100_000.0, 100_000.0));
        v.clamp_to_viewport();
        let image = v.image_screen_rect();
        assert!(
            image.intersects(v.viewport()),
            "image was lost off-screen: {image:?}"
        );

        v.pan(Vec2D::new(-100_000.0, -100_000.0));
        v.clamp_to_viewport();
        assert!(v.image_screen_rect().intersects(v.viewport()));
    }

    #[test]
    fn a_rect_survives_the_round_trip() {
        let v = view();
        let r = Rect::from_xywh(10.0, 20.0, 100.0, 50.0);
        let back = v.screen_rect_to_image(v.image_rect_to_screen(r));
        approx(back.pos, r.pos);
        approx(back.size, r.size);
    }

    #[test]
    fn fits_in_viewport_reports_honestly() {
        let mut v = view();
        assert!(v.fits_in_viewport(), "the fitted view should fit");
        v.zoom_by(8.0, Vec2D::new(250.0, 250.0));
        assert!(!v.fits_in_viewport());
    }

    #[test]
    fn changing_the_image_size_refits() {
        let mut v = view();
        v.set_image_size(Vec2D::new(250.0, 250.0));
        assert_eq!(v.image_size(), Vec2D::new(250.0, 250.0));
        assert_eq!(v.zoom(), 1.0, "the smaller image now fits at 1:1");
        approx(v.image_screen_rect().center(), Vec2D::new(250.0, 250.0));
    }
}
