//! [`CpuPainter`]: the [`Painter`] implementation that turns a scene into
//! pixels.
//!
//! # Why two images
//!
//! The painter holds a mutable *working* canvas and an immutable *base* image.
//! Solid geometry and text composite onto the working canvas; image effects
//! re-sample the base. Keeping them separate is what makes a blur mean "hide
//! what was originally here" rather than "smear whatever happens to be on
//! screen" — see the [`crate::effects`] module docs for the full argument.
//!
//! The two are ordinary borrows, so the caller decides whether the base is a
//! long-lived screenshot shared across many renders (the editor) or a
//! throwaway clone (the exporter).

use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::painter::{ImageEffect, Painter, TextDraw};
use bettershot_core::path::{Mesh, Path, Stroke};
use bettershot_core::style::Color;

use crate::canvas::Canvas;
use crate::text::{Font, system_font};
use crate::{effects, raster};

pub struct CpuPainter<'a> {
    canvas: &'a mut Canvas,
    base: &'a Canvas,
    font: &'a Font,
}

impl<'a> CpuPainter<'a> {
    /// Paint onto `canvas`, with image effects sampling `base`.
    ///
    /// Uses the process-wide [`system_font`], which is loaded lazily on first
    /// use and then shared.
    pub fn new(canvas: &'a mut Canvas, base: &'a Canvas) -> Self {
        Self::with_font(canvas, base, system_font())
    }

    pub fn with_font(canvas: &'a mut Canvas, base: &'a Canvas, font: &'a Font) -> Self {
        if canvas.width() != base.width() || canvas.height() != base.height() {
            log::debug!(
                "painting a {}x{} canvas over a {}x{} base; effects are clipped to the overlap",
                canvas.width(),
                canvas.height(),
                base.width(),
                base.height(),
            );
        }
        Self { canvas, base, font }
    }

    pub fn canvas(&self) -> &Canvas {
        self.canvas
    }

    pub fn base(&self) -> &Canvas {
        self.base
    }

    pub fn font(&self) -> &Font {
        self.font
    }
}

impl Painter for CpuPainter<'_> {
    fn fill_mesh(&mut self, mesh: &Mesh, color: Color) {
        raster::fill_mesh(self.canvas, mesh, color);
    }

    fn stroke_path(&mut self, path: &Path, stroke: Stroke) {
        raster::stroke_path(self.canvas, path, stroke);
    }

    fn draw_text(&mut self, text: &TextDraw<'_>) {
        crate::text::draw_text(self.canvas, self.font, text);
    }

    fn image_effect(&mut self, rect: Rect, effect: ImageEffect) {
        effects::apply_effect_in_region(self.canvas, self.base, rect, effect);
    }

    /// Real font metrics, so tools can size backing plates correctly. Falls
    /// back to core's estimate only when no face is loaded.
    fn measure_text(&self, text: &str, size: f32) -> Vec2D {
        self.font.measure(text, size)
    }

    /// Axis-aligned rectangles get exact analytic coverage instead of going
    /// through triangulation and supersampling. This is the hottest path in the
    /// crate — `dim_outside` alone issues four of them per frame — and it also
    /// makes a pixel-aligned rect land on exactly the right pixels.
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        raster::fill_axis_rect(self.canvas, rect, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::path::LineCap;

    fn canvases(w: u32, h: u32) -> (Canvas, Canvas) {
        let base = Canvas::filled(w, h, Color::white());
        (base.clone(), base)
    }

    #[test]
    fn fill_rect_lands_on_exactly_the_right_pixels() {
        let (mut canvas, base) = canvases(20, 20);
        let mut p = CpuPainter::new(&mut canvas, &base);
        p.fill_rect(Rect::from_xywh(5.0, 5.0, 10.0, 10.0), Color::red());

        assert_eq!(canvas.pixel(5, 5), Color::red(), "top-left interior");
        assert_eq!(canvas.pixel(14, 14), Color::red(), "bottom-right interior");
        assert_eq!(
            canvas.pixel(4, 5),
            Color::white(),
            "just outside on the left"
        );
        assert_eq!(
            canvas.pixel(15, 5),
            Color::white(),
            "just outside on the right"
        );
        assert_eq!(canvas.pixel(5, 4), Color::white(), "just above");
        assert_eq!(canvas.pixel(5, 15), Color::white(), "just below");
    }

    #[test]
    fn fill_path_of_a_rect_matches_fill_rect() {
        let (mut a, base) = canvases(20, 20);
        CpuPainter::new(&mut a, &base)
            .fill_rect(Rect::from_xywh(4.0, 4.0, 8.0, 8.0), Color::blue());

        let (mut b, base2) = canvases(20, 20);
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(4.0, 4.0, 8.0, 8.0));
        CpuPainter::new(&mut b, &base2).fill_path(&path, Color::blue());

        assert_eq!(a, b, "the triangulated and analytic paths must agree");
    }

    #[test]
    fn a_translucent_fill_blends_with_what_is_underneath() {
        let (mut canvas, base) = canvases(4, 4);
        let mut p = CpuPainter::new(&mut canvas, &base);
        p.fill_rect(
            Rect::from_xywh(0.0, 0.0, 4.0, 4.0),
            Color::new(255, 0, 0, 128),
        );
        assert_eq!(canvas.pixel(1, 1), Color::new(255, 127, 127, 255));
    }

    #[test]
    fn an_ellipse_fill_is_round() {
        let (mut canvas, base) = canvases(60, 60);
        let mut path = Path::new();
        path.add_ellipse(Rect::from_xywh(5.0, 5.0, 50.0, 50.0));
        CpuPainter::new(&mut canvas, &base).fill_path(&path, Color::green());

        assert_eq!(canvas.pixel(30, 30), Color::green(), "centre");
        assert_eq!(canvas.pixel(30, 6), Color::green(), "top of the ellipse");
        for (x, y) in [(6, 6), (53, 6), (6, 53), (53, 53)] {
            assert_eq!(
                canvas.pixel(x, y),
                Color::white(),
                "corner ({x},{y}) of the bounding box must stay clear"
            );
        }
    }

    #[test]
    fn dim_outside_leaves_the_hole_untouched() {
        let (mut canvas, base) = canvases(40, 40);
        let mut p = CpuPainter::new(&mut canvas, &base);
        let bounds = Rect::from_xywh(0.0, 0.0, 40.0, 40.0);
        p.dim_outside(
            Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
            bounds,
            Color::black().with_alpha(128),
        );
        assert_eq!(canvas.pixel(20, 20), Color::white(), "inside the hole");
        let dimmed = canvas.pixel(2, 2);
        assert!(dimmed.r < 200, "outside should be dimmed, got {dimmed}");
        // The four bands must not double-blend where they meet.
        assert_eq!(canvas.pixel(2, 2), canvas.pixel(2, 20), "corner vs. side");
    }

    #[test]
    fn measure_text_uses_the_painters_font() {
        let (mut canvas, base) = canvases(4, 4);
        let p = CpuPainter::new(&mut canvas, &base);
        let m = p.measure_text("Hello", 20.0);
        assert!(m.x > 0.0 && m.y > 0.0, "{m}");
        assert!((m.y - 24.0).abs() < 0.01, "one line at 1.2em");
    }

    #[test]
    fn an_effect_reads_the_base_even_when_drawn_last() {
        let mut base = Canvas::filled(40, 40, Color::white());
        for y in 0..40 {
            for x in 0..40 {
                if (x + y) % 2 == 0 {
                    base.set_pixel(x, y, Color::black());
                }
            }
        }
        let mut canvas = base.clone();
        {
            let mut p = CpuPainter::new(&mut canvas, &base);
            // A solid rectangle first...
            p.fill_rect(Rect::from_xywh(10.0, 10.0, 20.0, 20.0), Color::red());
            // ...then a blur over the same area.
            p.image_effect(
                Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
                ImageEffect::Blur { radius: 6.0 },
            );
        }
        let p = canvas.pixel(20, 20);
        assert!(
            p.r == p.g && p.g == p.b,
            "the blur must show the base checkerboard, not the red rect: {p}"
        );
    }

    #[test]
    fn nothing_panics_on_hostile_coordinates() {
        let (mut canvas, base) = canvases(30, 30);
        let mut p = CpuPainter::new(&mut canvas, &base);

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30, -1.0e30] {
            p.fill_rect(Rect::from_xywh(bad, bad, 10.0, 10.0), Color::red());
            p.fill_rect(Rect::from_xywh(0.0, 0.0, bad, bad), Color::red());

            let mut path = Path::new();
            path.add_polyline(&[Vec2D::new(bad, 0.0), Vec2D::new(10.0, bad)]);
            p.stroke_path(
                &path,
                Stroke::new(4.0, Color::red()).with_cap(LineCap::Round),
            );
            p.stroke_path(&path, Stroke::new(bad, Color::red()));

            p.image_effect(
                Rect::from_xywh(bad, 0.0, 10.0, 10.0),
                ImageEffect::Blur { radius: 5.0 },
            );
            p.draw_text(&TextDraw::new(
                Vec2D::new(bad, bad),
                "boom",
                20.0,
                Color::red(),
            ));
        }
        // 1e30 sized rects legitimately cover the canvas; only assert we did
        // not corrupt memory or panic, and that a far-away corner survived a
        // purely off-canvas draw.
        let mut clean = Canvas::filled(30, 30, Color::white());
        let base2 = clean.clone();
        let mut q = CpuPainter::new(&mut clean, &base2);
        q.fill_rect(Rect::from_xywh(-500.0, -500.0, 100.0, 100.0), Color::red());
        q.fill_rect(Rect::from_xywh(900.0, 900.0, 100.0, 100.0), Color::red());
        assert!((0..30).all(|y| (0..30).all(|x| clean.pixel(x, y) == Color::white())));
    }
}
