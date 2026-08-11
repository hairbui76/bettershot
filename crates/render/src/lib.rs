//! CPU rasterizer for bettershot annotations.
//!
//! This crate is the answer to two questions at once:
//!
//! * **How does an annotated screenshot get saved?** The editor draws with a
//!   GPU, but the exported file must be produced deterministically, at full
//!   image resolution, without depending on whatever the compositor happened to
//!   be showing. So export replays the same [`Scene`] onto a software painter.
//! * **How is any of this tested?** `crates/core` can only assert on *what*
//!   would be drawn (via its `RecordingPainter`). Everything downstream —
//!   anti-aliasing, stroke geometry, glyph placement, whether a blur actually
//!   hides anything — needs real pixels. A CPU rasterizer gives the project
//!   end-to-end coverage with no GPU and no display, which is exactly what CI
//!   has.
//!
//! # Shape of the API
//!
//! ```no_run
//! use bettershot_core::{Scene, Vec2D};
//! use bettershot_render::{Canvas, render_scene};
//!
//! let base = Canvas::load("shot.png")?;
//! let scene = Scene::new(base.size());
//! // ... commit annotations into the scene ...
//! let out = render_scene(&base, &scene);
//! out.save_png("annotated.png")?;
//! # Ok::<(), bettershot_render::RenderError>(())
//! ```
//!
//! [`CpuPainter`] is available directly for callers that want to paint an
//! in-progress tool preview or a crop overlay on top of a committed scene.
//!
//! # Invariants the whole crate upholds
//!
//! * **Image-pixel space, y down.** No view transform lives here; the app
//!   shell applies zoom and pan before it gets this far.
//! * **Effects sample the base image.** `Blur` and `Pixelate` re-draw the
//!   *original* screenshot, never the working canvas. It is what makes a
//!   redaction mean "hide what was here" instead of "smear whatever is on
//!   screen", and it is what makes a render idempotent; the full argument is in
//!   the `effects` module docs (`src/effects.rs`).
//! * **What you see is what you save.** [`apply_effect`] processes a whole
//!   image, which is how the editor builds the texture it previews a redaction
//!   with; [`apply_effect_in_region`] is what export runs over each annotation's
//!   rect. The two are guaranteed to produce the same bytes for the same pixels,
//!   so the preview is not a lookalike of the exported redaction — it *is* it.
//! * **Nothing panics on hostile geometry.** Negative, huge, NaN and infinite
//!   coordinates are clipped or skipped. A malformed annotation must not be
//!   able to take down an export.
//! * **Everything is anti-aliased.** Edge functions classify each pixel as
//!   inside, outside or straddling, and only straddling pixels pay for a 4x4
//!   supersample. A whole fill or stroke accumulates into one coverage mask
//!   before compositing, so shared triangle edges and round joins do not leave
//!   double-blended seams; see the `raster` module docs (`src/raster.rs`).
//!
//! Parts of the model this renders are adapted from
//! [Satty](https://github.com/Satty-org/Satty) (MPL-2.0); the rasterizer itself
//! is original.

mod canvas;
mod effects;
mod error;
mod painter;
mod raster;
mod text;

pub use canvas::{BYTES_PER_PIXEL, Canvas};
pub use effects::{apply_effect, apply_effect_in_region};
pub use error::RenderError;
pub use painter::CpuPainter;
pub use text::{
    FONT_ENV_VAR, FONT_SEARCH_PATHS, Font, FontSource, LINE_HEIGHT_FACTOR, system_font,
};

use bettershot_core::scene::Scene;

/// The exact pixels an image effect covers for `rect` on a `width`×`height`
/// image, as `(x0, y0, x1, y1)` with `x1`/`y1` exclusive.
///
/// This is the **single definition** of that rule. A live preview must use it
/// rather than rounding the rectangle itself: the two used to disagree, and a
/// redaction that covers one set of pixels on screen and a different set in the
/// exported file is a privacy defect, not a cosmetic one. It also guarantees
/// the result is inside the image, so callers can index without checking.
pub fn effect_region_pixels(
    rect: bettershot_core::math::Rect,
    width: u32,
    height: u32,
) -> Option<(u32, u32, u32, u32)> {
    raster::clip_to_pixels(rect, width, height)
        .map(|b| (b.x0 as u32, b.y0 as u32, b.x1 as u32, b.y1 as u32))
}

/// Render `scene`'s committed annotations over `base` and return the result.
///
/// `base` is left untouched and is what the image effects sample, so calling
/// this twice on the same inputs always produces identical pixels.
///
/// The canvas takes its size from `base`, not from `scene.size()`: after a crop
/// the two disagree, and it is the caller's job to crop the base image to match
/// before exporting. Anything that falls outside is clipped.
pub fn render_scene(base: &Canvas, scene: &Scene) -> Canvas {
    render_scene_with_font(base, scene, system_font())
}

/// [`render_scene`] with an explicit font, for callers that ship their own face
/// or want deterministic text in tests.
pub fn render_scene_with_font(base: &Canvas, scene: &Scene, font: &Font) -> Canvas {
    let mut out = base.clone();
    {
        let mut painter = CpuPainter::with_font(&mut out, base, font);
        scene.draw(&mut painter);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::math::{Rect, Vec2D};
    use bettershot_core::style::{Color, Style};
    use bettershot_core::tools::Rectangle;

    #[test]
    fn rendering_an_empty_scene_returns_the_base_unchanged() {
        let base = Canvas::filled(16, 16, Color::white());
        let scene = Scene::new(base.size());
        assert_eq!(render_scene(&base, &scene), base);
    }

    #[test]
    fn rendering_does_not_mutate_the_base() {
        let base = Canvas::filled(32, 32, Color::white());
        let before = base.clone();
        let mut scene = Scene::new(base.size());
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(4.0, 4.0, 20.0, 20.0),
            style: Style::default().with_fill(true),
        }));
        let out = render_scene(&base, &scene);
        assert_eq!(base, before, "the base must be read-only");
        assert_ne!(out, base, "and something must have been drawn");
    }

    #[test]
    fn rendering_is_deterministic() {
        let base = Canvas::filled(48, 48, Color::rgb(30, 30, 30));
        let mut scene = Scene::new(base.size());
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(6.0, 6.0, 30.0, 20.0),
            style: Style::default(),
        }));
        assert_eq!(render_scene(&base, &scene), render_scene(&base, &scene));
    }

    #[test]
    fn a_scene_larger_than_the_base_is_clipped_not_panicked() {
        let base = Canvas::filled(8, 8, Color::white());
        let mut scene = Scene::new(Vec2D::new(1000.0, 1000.0));
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(500.0, 500.0, 200.0, 200.0),
            style: Style::default().with_fill(true),
        }));
        assert_eq!(render_scene(&base, &scene), base);
    }
}
