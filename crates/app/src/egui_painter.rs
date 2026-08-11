//! The egui implementation of [`bettershot_core::Painter`].
//!
//! Core hands us geometry in image-pixel space that has already been flattened
//! and triangulated, so this file is deliberately thin: transform coordinates
//! through the [`View`], convert colours, and hand the result to egui. All the
//! interesting geometry decisions were made in core, where they are testable.

use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::painter::{ImageEffect, Painter, TextAlign, TextDraw};
use bettershot_core::path::{LineCap, Mesh, Path, Stroke};
use bettershot_core::style::Color;

use crate::effects::EffectTextures;
use crate::view::View;

pub fn to_pos(v: Vec2D) -> egui::Pos2 {
    egui::pos2(v.x, v.y)
}

pub fn to_vec(v: Vec2D) -> egui::Vec2 {
    egui::vec2(v.x, v.y)
}

pub fn from_pos(p: egui::Pos2) -> Vec2D {
    Vec2D::new(p.x, p.y)
}

pub fn to_rect(r: Rect) -> egui::Rect {
    let r = r.normalized();
    egui::Rect::from_min_size(to_pos(r.pos), to_vec(r.size))
}

pub fn from_rect(r: egui::Rect) -> Rect {
    Rect::new(from_pos(r.min), Vec2D::new(r.width(), r.height()))
}

pub fn to_color(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the inverse conversion is exercised by tests")
)]
pub fn from_color(c: egui::Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::new(r, g, b, a)
}

/// Draws a scene into an egui painter through a view transform.
pub struct EguiPainter<'a> {
    painter: &'a egui::Painter,
    view: View,
    effects: Option<&'a EffectTextures>,
}

impl<'a> EguiPainter<'a> {
    pub fn new(painter: &'a egui::Painter, view: View) -> Self {
        Self {
            painter,
            view,
            effects: None,
        }
    }

    /// Supply the pre-computed blurred/pixelated copies of the base image.
    /// Without them, obscure annotations cannot be drawn — see
    /// [`crate::effects`] for why they are textures rather than a shader.
    pub fn with_effects(mut self, effects: &'a EffectTextures) -> Self {
        self.effects = Some(effects);
        self
    }

    /// Screen-space width of a stroke specified in image pixels. Clamped to a
    /// visible minimum so annotations do not vanish when zoomed far out.
    fn stroke_width(&self, image_width: f32) -> f32 {
        self.view.scale_length(image_width).max(0.75)
    }

    fn egui_stroke(&self, stroke: Stroke) -> egui::Stroke {
        egui::Stroke {
            width: self.stroke_width(stroke.width),
            color: to_color(stroke.color),
        }
    }

    /// egui strokes have square ends; round caps and joins are emulated by
    /// stamping a disc at every vertex, which also hides the gaps that
    /// polyline joins leave on sharp corners.
    fn add_round_caps(&self, points: &[Vec2D], stroke: Stroke) {
        let radius = self.stroke_width(stroke.width) / 2.0;
        if radius <= 0.5 {
            return;
        }
        let color = to_color(stroke.color);
        for p in points {
            self.painter
                .circle_filled(to_pos(self.view.image_to_screen(*p)), radius, color);
        }
    }
}

impl Painter for EguiPainter<'_> {
    fn fill_mesh(&mut self, mesh: &Mesh, color: Color) {
        if mesh.is_empty() || color.a == 0 {
            return;
        }
        let mut out = egui::epaint::Mesh::default();
        let color = to_color(color);
        out.vertices.reserve(mesh.vertices.len());
        for v in &mesh.vertices {
            let p = self.view.image_to_screen(*v);
            if !p.x.is_finite() || !p.y.is_finite() {
                return;
            }
            out.vertices.push(egui::epaint::Vertex {
                pos: to_pos(p),
                // A blank UV samples the font atlas's white pixel, which is
                // how epaint draws untextured geometry.
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        out.indices.extend_from_slice(&mesh.indices);
        self.painter.add(egui::Shape::mesh(out));
    }

    fn stroke_path(&mut self, path: &Path, stroke: Stroke) {
        if !stroke.is_visible() {
            return;
        }
        let egui_stroke = self.egui_stroke(stroke);
        for sub in &path.subpaths {
            if sub.points.len() < 2 {
                continue;
            }
            let mut points: Vec<egui::Pos2> = sub
                .points
                .iter()
                .map(|p| to_pos(self.view.image_to_screen(*p)))
                .collect();
            if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
                continue;
            }
            if sub.closed {
                points.push(points[0]);
            }
            self.painter.add(egui::Shape::line(points, egui_stroke));
            if stroke.cap == LineCap::Round {
                self.add_round_caps(&sub.points, stroke);
            }
        }
    }

    fn draw_text(&mut self, text: &TextDraw<'_>) {
        let font = egui::FontId::proportional(self.view.scale_length(text.size).max(4.0));
        let galley =
            self.painter
                .layout_no_wrap(text.text.to_owned(), font.clone(), to_color(text.color));

        let anchor = to_pos(self.view.image_to_screen(text.pos));
        let top_left = match text.align {
            TextAlign::Left => anchor,
            TextAlign::Center => anchor - galley.size() / 2.0,
        };

        if let Some(background) = text.background {
            let padding = galley.size().y * 0.1;
            self.painter.rect_filled(
                egui::Rect::from_min_size(top_left, galley.size()).expand(padding),
                padding,
                to_color(background),
            );
        }

        // The caret is drawn before the glyphs so it never covers them.
        if let Some(cursor) = text.cursor {
            let char_index = text.text.get(..cursor).map_or(0, |s| s.chars().count());
            let ccursor = egui::text::CCursor::new(char_index);
            let caret = galley.pos_from_cursor(ccursor);
            let caret = caret.translate(top_left.to_vec2());
            self.painter.rect_filled(
                egui::Rect::from_min_size(
                    caret.min,
                    egui::vec2(1.5_f32.max(font.size * 0.06), caret.height()),
                ),
                0.0,
                to_color(text.color),
            );
        }

        self.painter.galley(top_left, galley, to_color(text.color));
    }

    fn image_effect(&mut self, rect: Rect, effect: ImageEffect) {
        if !effect.is_visible() {
            return;
        }
        let rect = rect.normalized();
        if rect.is_empty() {
            return;
        }
        let Some(effects) = self.effects else {
            // No pre-computed textures (the very first frame after loading an
            // image). Skipping is better than painting a solid block, which
            // would flash.
            return;
        };
        let Some((width, height)) = effects.image_size() else {
            return;
        };
        let Some((texture, covered)) = effects.texture_for(rect, effect, width, height) else {
            return;
        };

        // Draw it over the region it actually covers, not the region that was
        // asked for. A blur dragged off the edge of the image produces a
        // clipped texture; stretching that back over the full rectangle would
        // scale the pixels and misrepresent what was obscured.
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let screen = to_rect(self.view.image_rect_to_screen(covered));
        let mut mesh = egui::epaint::Mesh::with_texture(texture);
        mesh.add_rect_with_uv(screen, uv, egui::Color32::WHITE);
        self.painter.add(egui::Shape::mesh(mesh));
    }

    fn measure_text(&self, text: &str, size: f32) -> Vec2D {
        let font = egui::FontId::proportional(self.view.scale_length(size).max(4.0));
        let galley = self
            .painter
            .layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE);
        // Report in image space, undoing the view scale.
        let inv = 1.0 / self.view.zoom().max(f32::EPSILON);
        Vec2D::new(galley.size().x * inv, galley.size().y * inv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_colours_round_trip_exactly() {
        for c in [
            Color::red(),
            Color::black(),
            Color::white(),
            Color::orange(),
        ] {
            assert_eq!(from_color(to_color(c)), c);
        }
    }

    #[test]
    fn translucent_colours_survive_the_round_trip_approximately() {
        // egui stores colours premultiplied, so the round trip loses precision
        // in proportion to how transparent the colour is. This is fine for what
        // it is used for — the highlight tool at alpha 100 drifts by about one
        // unit per channel — but it means the conversion is not an identity and
        // nothing should rely on it being one.
        let highlight = Color::red().with_alpha(100);
        let back = from_color(to_color(highlight));
        assert_eq!(back.a, 100);
        for (a, b) in [
            (back.r, highlight.r),
            (back.g, highlight.g),
            (back.b, highlight.b),
        ] {
            assert!(a.abs_diff(b) <= 3, "{a} drifted too far from {b}");
        }

        // Fully transparent is exact, because there is nothing to lose.
        assert_eq!(from_color(to_color(Color::transparent())).a, 0);
    }

    #[test]
    fn geometry_conversion_round_trips() {
        let v = Vec2D::new(3.5, -7.25);
        assert_eq!(from_pos(to_pos(v)), v);

        let r = Rect::from_xywh(1.0, 2.0, 30.0, 40.0);
        let back = from_rect(to_rect(r));
        assert_eq!(back, r);
    }

    #[test]
    fn negative_size_rects_are_normalised_for_egui() {
        // egui panics on inverted rects, so the conversion must normalise.
        let r = to_rect(Rect::from_xywh(10.0, 10.0, -5.0, -5.0));
        assert!(r.min.x <= r.max.x && r.min.y <= r.max.y);
        assert_eq!(r.min, egui::pos2(5.0, 5.0));
    }
}
