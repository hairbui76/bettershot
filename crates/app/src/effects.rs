//! Live previews of blur and pixelate annotations.
//!
//! Obscure annotations sample the *original* screenshot, never whatever has
//! been painted over it — otherwise drawing an arrow across a blurred region
//! would smear the arrow, and undoing that arrow would leave its ghost behind.
//!
//! **What is on screen must be exactly what gets saved.** For a tool whose job
//! includes hiding sensitive information, that is a correctness property, not a
//! nicety: a redaction that looks opaque in the editor but exports softer would
//! leak. So the preview does not approximate the export with a faster blur —
//! it calls the very same function, [`bettershot_render::apply_effect_in_region`],
//! that the exporter uses, and uploads the result as a texture.
//!
//! Two consequences follow, and both are deliberate:
//!
//! - Effects are computed **per annotation rectangle**, not once for the whole
//!   screen. A typical redaction costs single-digit milliseconds, and the cost
//!   scales with the size of the redaction rather than the size of the display.
//!   Preprocessing a whole 4K screenshot would cost roughly a second.
//! - The cache key is the **exact** [`ImageEffect`] the drawable reports, not a
//!   rounded one. Quantising the strength would reintroduce the very mismatch
//!   this module exists to prevent, because the exporter does not quantise.

use std::collections::HashMap;

use bettershot_core::math::Rect;
use bettershot_core::painter::ImageEffect;
use bettershot_render::Canvas;
use image::RgbaImage;

/// Identifies one processed region.
///
/// Keyed on the **pixels actually covered**, not on the requested rectangle.
/// Rounding the rectangle for the key while flooring it for the copy groups
/// them differently — rects at 0.6 and 1.4 round to the same key but start on
/// different pixels — so the cache would hand back a texture computed for a
/// different region. Strength is keyed by bit pattern so it is exact; see the
/// module docs on why quantising it is not an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EffectKey {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    kind: u8,
    strength_bits: u32,
}

impl EffectKey {
    /// `None` when the effect covers no pixels of this image.
    fn new(rect: Rect, effect: ImageEffect, width: u32, height: u32) -> Option<Self> {
        let (x0, y0, x1, y1) = bettershot_render::effect_region_pixels(rect, width, height)?;
        let (kind, strength) = match effect {
            ImageEffect::Blur { radius } => (0u8, radius),
            ImageEffect::Pixelate { block_size } => (1u8, block_size),
        };
        Some(Self {
            x0,
            y0,
            x1,
            y1,
            kind,
            strength_bits: strength.to_bits(),
        })
    }

    /// The image-space rectangle this key covers.
    fn covered(&self) -> Rect {
        Rect::from_xywh(
            self.x0 as f32,
            self.y0 as f32,
            (self.x1 - self.x0) as f32,
            (self.y1 - self.y0) as f32,
        )
    }
}

/// Cache of processed regions of the base image, one texture per annotation.
#[derive(Default)]
pub struct EffectTextures {
    /// Size of the image every cached texture was derived from.
    image_size: Option<(u32, u32)>,
    /// Texture plus the image-space rectangle it exactly covers.
    textures: HashMap<EffectKey, (egui::TextureHandle, Rect)>,
    /// Keys requested during the current frame, used to evict the rest. A blur
    /// being dragged produces a new rectangle every frame, so without eviction
    /// the cache would grow for as long as the drag lasts.
    live: Vec<EffectKey>,
    /// Keys that produced no texture because they fall entirely outside the
    /// image. Without this they would count as "missing" forever and clone the
    /// whole screenshot on every frame — exactly the cost this module exists
    /// to avoid. Reachable by cropping so an existing blur falls outside.
    empty: Vec<EffectKey>,
}

impl EffectTextures {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.textures.clear();
        self.live.clear();
        self.empty.clear();
        self.image_size = None;
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// Make sure a texture exists for every requested region, then drop any
    /// that were not requested. Call once per frame, before painting.
    pub fn ensure(
        &mut self,
        ctx: &egui::Context,
        base: &RgbaImage,
        requests: impl IntoIterator<Item = (Rect, ImageEffect)>,
    ) {
        self.live.clear();
        if self.image_size != Some((base.width(), base.height())) {
            // A crop or a new capture invalidates every cached region.
            self.textures.clear();
            self.empty.clear();
            self.image_size = Some((base.width(), base.height()));
        }

        // Work out what is missing before touching the pixels: this runs every
        // frame, and copying the screenshot into a canvas each time would cost
        // tens of megabytes of memcpy per frame at 4K.
        let (w, h) = (base.width(), base.height());
        let mut missing: Vec<(EffectKey, Rect, ImageEffect)> = Vec::new();
        for (rect, effect) in requests {
            if !effect.is_visible() {
                continue;
            }
            // An effect covering no pixels of this image needs no texture, and
            // must not be retried every frame.
            let Some(key) = EffectKey::new(rect, effect, w, h) else {
                continue;
            };
            self.live.push(key);
            if !self.textures.contains_key(&key) && !self.empty.contains(&key) {
                missing.push((key, rect, effect));
            }
        }

        if !missing.is_empty() {
            match Canvas::from_rgba8(base.width(), base.height(), base.as_raw().clone()) {
                Ok(canvas) => {
                    for (key, rect, effect) in missing {
                        match render_region(ctx, &canvas, rect, effect) {
                            Some(entry) => {
                                self.textures.insert(key, entry);
                            }
                            None => self.empty.push(key),
                        }
                    }
                }
                Err(e) => log::error!("could not build a canvas for effect previews: {e}"),
            }
        }

        self.textures.retain(|key, _| self.live.contains(key));
        self.empty.retain(|key| self.live.contains(key));
    }

    /// The texture for this annotation, and the image-space rectangle it
    /// covers. The rectangle is not necessarily the one asked for: it is the
    /// region the exporter would actually touch, clipped to the image.
    pub fn texture_for(
        &self,
        rect: Rect,
        effect: ImageEffect,
        width: u32,
        height: u32,
    ) -> Option<(egui::TextureId, Rect)> {
        let key = EffectKey::new(rect, effect, width, height)?;
        self.textures
            .get(&key)
            .map(|(handle, covered)| (handle.id(), *covered))
    }

    /// The image the cached textures were built from, so the painter can ask
    /// for one without threading the size through separately.
    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.image_size
    }
}

/// Process one region and upload it.
///
/// The region is rendered into a copy of the base canvas and then cropped, so
/// the pixels are produced by exactly the call the exporter makes.
fn render_region(
    ctx: &egui::Context,
    base: &Canvas,
    rect: Rect,
    effect: ImageEffect,
) -> Option<(egui::TextureHandle, Rect)> {
    // Ask the renderer which pixels it would touch, rather than deciding here.
    // Rounding the rectangle independently used to be the rule, and it both
    // disagreed with the exporter by a pixel (a redaction leak) and could
    // round outward past the image edge and panic.
    // Derived from the same key the cache uses, so the texture and the
    // rectangle it is drawn over can never describe different pixels.
    let key = EffectKey::new(rect, effect, base.width(), base.height())?;
    let (x0, y0, x1, y1) = (key.x0, key.y0, key.x1, key.y1);
    let covered = key.covered();

    // The effect is applied with the ORIGINAL rectangle, exactly as export
    // does; only the window we copy out is the clipped one.
    let mut processed = base.clone();
    bettershot_render::apply_effect_in_region(&mut processed, base, rect, effect);

    // Copy out just the affected window; uploading the whole screen per
    // annotation would defeat the point of working per-region.
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y1 {
        for x in x0..x1 {
            // In range by construction, but `get_pixel` keeps a future change
            // to the clipping rule from turning into a panic in a user's face.
            let c = processed.get_pixel(x, y).unwrap_or_default();
            pixels.push(egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a));
        }
    }

    let image = egui::ColorImage {
        size: [w, h],
        pixels,
        source_size: egui::vec2(w as f32, h as f32),
    };
    Some((
        ctx.load_texture(
            "bettershot-effect",
            image,
            // NEAREST keeps pixelation crisp; a blurred region is smooth already.
            egui::TextureOptions::NEAREST,
        ),
        covered,
    ))
}

/// Collect the obscure annotations a scene contains, by replaying it onto a
/// recording painter. This keeps the editor from having to know which drawable
/// types are obscure annotations.
pub fn effects_in_scene(scene: &bettershot_core::Scene) -> Vec<(Rect, ImageEffect)> {
    let mut recorder = bettershot_core::painter::RecordingPainter::new();
    scene.draw(&mut recorder);
    effects_in_recording(&recorder)
}

/// The same, for a single in-progress drawable.
pub fn effects_in_drawable(drawable: &dyn bettershot_core::Drawable) -> Vec<(Rect, ImageEffect)> {
    let mut recorder = bettershot_core::painter::RecordingPainter::new();
    drawable.draw(&mut recorder);
    effects_in_recording(&recorder)
}

fn effects_in_recording(
    recorder: &bettershot_core::painter::RecordingPainter,
) -> Vec<(Rect, ImageEffect)> {
    recorder
        .ops
        .iter()
        .filter_map(|op| match op {
            bettershot_core::painter::PaintOp::Effect { rect, effect } => Some((*rect, *effect)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::Scene;
    use bettershot_core::math::Vec2D;
    use bettershot_core::style::{Size, Style};
    use bettershot_core::tools::{Blur, ObscureKind};

    fn blur_at(rect: Rect, size: Size, obscure: ObscureKind) -> Box<Blur> {
        Box::new(Blur {
            rect,
            style: Style::default().with_size(size),
            obscure,
        })
    }

    #[test]
    fn an_invisible_or_empty_effect_is_never_cached() {
        // Guards the early-outs in `ensure`, which are what stop a degenerate
        // drag from allocating a texture on every frame.
        let rect = Rect::from_xywh(0.0, 0.0, 0.0, 0.0);
        assert!(key(rect, ImageEffect::Blur { radius: 20.0 }).is_none());
        assert!(!ImageEffect::Blur { radius: 0.1 }.is_visible());
    }

    #[test]
    fn a_scene_reports_every_obscure_annotation_with_its_rectangle() {
        let mut scene = Scene::new(Vec2D::new(500.0, 500.0));
        scene.add(blur_at(
            Rect::from_xywh(0.0, 0.0, 50.0, 50.0),
            Size::Medium,
            ObscureKind::Blur,
        ));
        scene.add(blur_at(
            Rect::from_xywh(100.0, 100.0, 60.0, 60.0),
            Size::Medium,
            ObscureKind::Pixelate,
        ));

        let found = effects_in_scene(&scene);
        assert_eq!(found.len(), 2, "each annotation needs its own region");
        assert!(found.iter().any(|(r, _)| r.width() == 50.0));
        assert!(
            found
                .iter()
                .any(|(_, e)| matches!(e, ImageEffect::Pixelate { .. }))
        );
    }

    #[test]
    fn a_scene_without_obscure_annotations_needs_nothing() {
        let mut scene = Scene::new(Vec2D::new(100.0, 100.0));
        scene.add(Box::new(bettershot_core::tools::Rectangle {
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            style: Style::default(),
        }));
        assert!(effects_in_scene(&scene).is_empty());
    }

    /// Keys for a 200x200 image, which is what the tests below assume.
    fn key(rect: Rect, effect: ImageEffect) -> Option<EffectKey> {
        EffectKey::new(rect, effect, 200, 200)
    }

    #[test]
    fn identical_annotations_share_a_cache_key() {
        let rect = Rect::from_xywh(10.0, 10.0, 40.0, 40.0);
        let effect = ImageEffect::Blur { radius: 20.0 };
        assert_eq!(key(rect, effect), key(rect, effect));
        assert!(key(rect, effect).is_some());
    }

    #[test]
    fn rectangles_covering_different_pixels_never_share_a_key() {
        // The bug this guards: keying on the *rounded* rectangle while copying
        // the *floored* one groups them differently. 0.6 and 1.4 round to the
        // same integer but start on different pixels, so one would have been
        // served the other's texture.
        let effect = ImageEffect::Blur { radius: 20.0 };
        let a = key(Rect::from_xywh(0.6, 5.0, 30.0, 30.0), effect).unwrap();
        let b = key(Rect::from_xywh(1.4, 5.0, 30.0, 30.0), effect).unwrap();
        assert_ne!(a, b, "different starting pixels must not share a texture");
        assert_ne!(a.covered(), b.covered());
    }

    #[test]
    fn rectangles_covering_the_same_pixels_do_share_a_key() {
        // The other half: two rects that clip to the same pixel box genuinely
        // are the same texture, and should not be computed twice.
        let effect = ImageEffect::Blur { radius: 20.0 };
        let a = key(Rect::from_xywh(4.1, 4.1, 10.2, 10.2), effect).unwrap();
        let b = key(Rect::from_xywh(4.9, 4.9, 9.6, 9.6), effect).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.covered(), b.covered());
    }

    #[test]
    fn nearby_strengths_do_not_share_a_key() {
        // This is the whole point: the exporter does not round, so neither may
        // the preview. A 20.0 blur and a 20.4 blur are different images.
        let rect = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert_ne!(
            key(rect, ImageEffect::Blur { radius: 20.0 }),
            key(rect, ImageEffect::Blur { radius: 20.4 })
        );
    }

    #[test]
    fn blur_and_pixelate_never_collide_at_the_same_strength() {
        let rect = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert_ne!(
            key(rect, ImageEffect::Blur { radius: 20.0 }),
            key(rect, ImageEffect::Pixelate { block_size: 20.0 })
        );
    }

    #[test]
    fn moving_an_annotation_changes_its_key() {
        let effect = ImageEffect::Blur { radius: 20.0 };
        assert_ne!(
            key(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), effect),
            key(Rect::from_xywh(1.0, 0.0, 10.0, 10.0), effect)
        );
    }

    #[test]
    fn a_backwards_rectangle_keys_the_same_as_its_normalised_form() {
        let effect = ImageEffect::Blur { radius: 20.0 };
        assert_eq!(
            key(Rect::from_xywh(10.0, 10.0, -10.0, -10.0), effect),
            key(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), effect)
        );
    }

    /// The pixel box the preview copies out, exercised through the same
    /// function the editor calls. The previous tests bypassed this and so
    /// missed both a panic and a one-pixel redaction leak.
    fn preview_box(rect: Rect, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        bettershot_render::effect_region_pixels(rect, w, h)
    }

    #[test]
    fn a_half_pixel_rect_at_the_image_edge_does_not_run_off_it() {
        // The panic that shipped: clamping and then rounding independently
        // produced left+width one past the edge. Reachable by dragging a blur
        // off the right edge whenever the view origin lands on a half pixel,
        // which happens for any odd difference between image and window width.
        for (w, h) in [(1441u32, 900u32), (100, 100), (7, 5)] {
            for rect in [
                Rect::from_xywh(0.5, 0.5, w as f32, h as f32),
                Rect::from_xywh(w as f32 - 0.5, 0.0, 50.0, 10.0),
                Rect::from_xywh(-10.5, -10.5, w as f32 + 40.0, h as f32 + 40.0),
                Rect::from_xywh(w as f32 - 0.5, h as f32 - 0.5, 0.6, 0.6),
            ] {
                if let Some((x0, y0, x1, y1)) = preview_box(rect, w, h) {
                    assert!(x1 <= w && y1 <= h, "{rect:?} on {w}x{h} gave {x1},{y1}");
                    assert!(x0 < x1 && y0 < y1, "empty box reported as present");
                }
            }
        }
    }

    #[test]
    fn the_preview_covers_exactly_the_pixels_the_exporter_touches() {
        // The guarantee this module exists for, checked against the rule the
        // renderer actually uses rather than against a reimplementation of it.
        let (w, h) = (64u32, 48u32);
        let mut leaks = 0;
        let mut x = -3.0f32;
        while x < w as f32 + 3.0 {
            let mut width = 0.5f32;
            while width < 20.0 {
                let rect = Rect::from_xywh(x, 4.25, width, 9.5);
                let preview = preview_box(rect, w, h);
                let export = bettershot_render::effect_region_pixels(rect, w, h);
                if preview != export {
                    leaks += 1;
                }
                width += 0.5;
            }
            x += 0.5;
        }
        assert_eq!(
            leaks, 0,
            "preview and export disagreed on {leaks} rectangles"
        );
    }

    #[test]
    fn an_entirely_off_image_effect_reports_no_region() {
        let (w, h) = (32u32, 32u32);
        assert!(preview_box(Rect::from_xywh(100.0, 100.0, 10.0, 10.0), w, h).is_none());
        assert!(preview_box(Rect::from_xywh(-50.0, 0.0, 10.0, 10.0), w, h).is_none());
        assert!(preview_box(Rect::from_xywh(0.0, 0.0, 0.0, 0.0), w, h).is_none());
    }

    #[test]
    fn a_preview_matches_what_the_exporter_writes() {
        // The guarantee this module exists for, asserted directly: the pixels
        // the preview would upload are the pixels `render_scene` produces.
        let base = Canvas::from_rgba8(
            64,
            64,
            (0..64 * 64)
                .flat_map(|i| {
                    let v = (i % 251) as u8;
                    [v, v.wrapping_mul(3), 255 - v, 255]
                })
                .collect(),
        )
        .expect("canvas");

        let rect = Rect::from_xywh(8.0, 8.0, 32.0, 24.0);
        let effect = ImageEffect::Blur { radius: 20.0 };

        // What the preview computes.
        let mut previewed = base.clone();
        bettershot_render::apply_effect_in_region(&mut previewed, &base, rect, effect);

        // What export computes.
        let mut scene = Scene::new(Vec2D::new(64.0, 64.0));
        scene.add(blur_at(rect, Size::Medium, ObscureKind::Blur));
        let exported = bettershot_render::render_scene(&base, &scene);

        for y in rect.top() as u32..rect.bottom() as u32 {
            for x in rect.left() as u32..rect.right() as u32 {
                assert_eq!(
                    previewed.pixel(x, y),
                    exported.pixel(x, y),
                    "preview and export disagree at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn the_preview_uses_the_exact_strength_the_drawable_reports() {
        // A non-integral size factor is precisely the case a quantised cache
        // key used to get wrong.
        let drawable = Blur {
            rect: Rect::from_xywh(0.0, 0.0, 20.0, 20.0),
            style: Style {
                annotation_size_factor: 1.37,
                ..Default::default()
            },
            obscure: ObscureKind::Blur,
        };
        let requested = effects_in_drawable(&drawable);
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].1, drawable.effect(), "strength was altered");
    }
}
