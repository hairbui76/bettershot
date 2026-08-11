//! The drawing surface abstraction that decouples annotations from any
//! particular renderer.
//!
//! Backends implement [`Painter`]. The editor implements it over egui/wgpu; the
//! PNG exporter implements it over a CPU rasterizer; tests implement it over a
//! recording stub. Because core has already flattened curves and triangulated
//! fills, a backend only needs to handle meshes, polylines, text and image
//! effects.

use crate::math::{Rect, Vec2D};
use crate::path::{Mesh, Path, Stroke};
use crate::style::Color;

/// A pixel effect sampled from the underlying screenshot rather than painted
/// with a solid color. The original image is never mutated; effects are
/// re-evaluated at render and export time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageEffect {
    /// Gaussian-style blur with the given radius, in image pixels.
    Blur { radius: f32 },
    /// Mosaic with square blocks of `block_size` image pixels.
    Pixelate { block_size: f32 },
}

impl ImageEffect {
    /// Effects below roughly a pixel are invisible and can be skipped.
    pub fn is_visible(&self) -> bool {
        match self {
            ImageEffect::Blur { radius } => *radius >= 0.5,
            ImageEffect::Pixelate { block_size } => *block_size >= 1.5,
        }
    }
}

/// How [`TextDraw::pos`] anchors the text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// `pos` is the top-left corner.
    #[default]
    Left,
    /// `pos` is the centre of the block on both axes. Used by numbered
    /// markers, whose label must sit in the middle of the disc.
    Center,
}

/// A run of text to render. Layout and font handling stay in the backend; core
/// only says what to draw and where.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDraw<'a> {
    /// Anchor point in image space. The text baseline block grows downward
    /// from here.
    pub pos: Vec2D,
    pub text: &'a str,
    pub size: f32,
    pub color: Color,
    pub align: TextAlign,
    /// Byte offset of the editing caret, when this text is being edited.
    pub cursor: Option<usize>,
    /// Optional backing plate drawn behind the glyphs, for legibility over
    /// busy screenshots.
    pub background: Option<Color>,
}

impl<'a> TextDraw<'a> {
    pub fn new(pos: Vec2D, text: &'a str, size: f32, color: Color) -> Self {
        Self {
            pos,
            text,
            size,
            color,
            align: TextAlign::default(),
            cursor: None,
            background: None,
        }
    }

    pub fn centered(mut self) -> Self {
        self.align = TextAlign::Center;
        self
    }

    pub fn with_cursor(mut self, cursor: Option<usize>) -> Self {
        self.cursor = cursor;
        self
    }

    pub fn with_background(mut self, background: Option<Color>) -> Self {
        self.background = background;
        self
    }
}

/// A renderer that annotations can draw themselves onto.
///
/// All coordinates are image-pixel space. Implementations apply the view
/// transform themselves.
pub trait Painter {
    /// Fill a triangulated mesh with a solid color.
    fn fill_mesh(&mut self, mesh: &Mesh, color: Color);

    /// Stroke every subpath of `path`.
    fn stroke_path(&mut self, path: &Path, stroke: Stroke);

    /// Draw text.
    fn draw_text(&mut self, text: &TextDraw<'_>);

    /// Re-draw the region of the base image inside `rect` with `effect`
    /// applied.
    fn image_effect(&mut self, rect: Rect, effect: ImageEffect);

    /// Measure a text run so tools can size backing plates and hit-test.
    /// Backends with real font metrics should override this; the default is a
    /// monospace-ish estimate that is good enough for layout fallbacks.
    fn measure_text(&self, text: &str, size: f32) -> Vec2D {
        estimate_text_size(text, size)
    }

    /// Fill a path, triangulating it first. Backends rarely need to override
    /// this.
    fn fill_path(&mut self, path: &Path, color: Color) {
        if color.a == 0 {
            return;
        }
        let mesh = path.fill_mesh();
        if !mesh.is_empty() {
            self.fill_mesh(&mesh, color);
        }
    }

    /// Fill a rectangle.
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let mut path = Path::new();
        path.add_rect(rect);
        self.fill_path(&path, color);
    }

    /// Dim everything outside `hole` with `color`, leaving the hole untouched.
    /// Used by the crop tool and the region-selection overlay. The default
    /// implementation emits four rectangles around the hole, which avoids
    /// double-blending at the seams the way an even-odd path would not.
    fn dim_outside(&mut self, hole: Rect, bounds: Rect, color: Color) {
        let hole = hole.normalized().clamped_to(bounds);
        let b = bounds.normalized();
        if hole.is_empty() {
            self.fill_rect(b, color);
            return;
        }
        // Top, bottom, left-middle, right-middle.
        let bands = [
            Rect::from_xywh(b.left(), b.top(), b.width(), hole.top() - b.top()),
            Rect::from_xywh(
                b.left(),
                hole.bottom(),
                b.width(),
                b.bottom() - hole.bottom(),
            ),
            Rect::from_xywh(b.left(), hole.top(), hole.left() - b.left(), hole.height()),
            Rect::from_xywh(
                hole.right(),
                hole.top(),
                b.right() - hole.right(),
                hole.height(),
            ),
        ];
        for band in bands {
            if !band.is_empty() {
                self.fill_rect(band, color);
            }
        }
    }
}

/// Rough text metrics used when a backend has no font loaded. Assumes an
/// average advance of 0.5em and a line height of 1.2em.
pub fn estimate_text_size(text: &str, size: f32) -> Vec2D {
    let lines: Vec<&str> = text.split('\n').collect();
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    Vec2D::new(
        widest as f32 * size * 0.5,
        lines.len().max(1) as f32 * size * 1.2,
    )
}

/// A [`Painter`] that records every call instead of rendering. Lets the tool
/// and drawable tests assert on what would be drawn without a GPU.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RecordingPainter {
    pub ops: Vec<PaintOp>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaintOp {
    Mesh {
        triangles: usize,
        bounds: Option<Rect>,
        color: Color,
    },
    Stroke {
        points: usize,
        stroke: Stroke,
        bounds: Option<Rect>,
    },
    Text {
        text: String,
        pos: Vec2D,
        size: f32,
        color: Color,
    },
    Effect {
        rect: Rect,
        effect: ImageEffect,
    },
}

impl RecordingPainter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn meshes(&self) -> impl Iterator<Item = &PaintOp> {
        self.ops
            .iter()
            .filter(|op| matches!(op, PaintOp::Mesh { .. }))
    }

    pub fn strokes(&self) -> impl Iterator<Item = &PaintOp> {
        self.ops
            .iter()
            .filter(|op| matches!(op, PaintOp::Stroke { .. }))
    }

    pub fn effects(&self) -> impl Iterator<Item = &PaintOp> {
        self.ops
            .iter()
            .filter(|op| matches!(op, PaintOp::Effect { .. }))
    }

    pub fn texts(&self) -> impl Iterator<Item = &PaintOp> {
        self.ops
            .iter()
            .filter(|op| matches!(op, PaintOp::Text { .. }))
    }

    /// Union of the bounds of everything recorded.
    pub fn drawn_bounds(&self) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        let mut merge = |r: Rect| {
            acc = Some(match acc {
                None => r,
                Some(a) => {
                    let left = a.left().min(r.left());
                    let top = a.top().min(r.top());
                    let right = a.right().max(r.right());
                    let bottom = a.bottom().max(r.bottom());
                    Rect::from_xywh(left, top, right - left, bottom - top)
                }
            });
        };
        for op in &self.ops {
            match op {
                PaintOp::Mesh { bounds, .. } | PaintOp::Stroke { bounds, .. } => {
                    if let Some(b) = bounds {
                        merge(*b);
                    }
                }
                PaintOp::Effect { rect, .. } => merge(*rect),
                PaintOp::Text {
                    pos, size, text, ..
                } => {
                    let s = estimate_text_size(text, *size);
                    merge(Rect::new(*pos, s));
                }
            }
        }
        acc
    }
}

fn bounds_of(points: impl Iterator<Item = Vec2D>) -> Option<Rect> {
    let mut points = points;
    let first = points.next()?;
    let (mut min, mut max) = (first, first);
    for p in points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Some(Rect::new(min, max - min))
}

impl Painter for RecordingPainter {
    fn fill_mesh(&mut self, mesh: &Mesh, color: Color) {
        self.ops.push(PaintOp::Mesh {
            triangles: mesh.triangle_count(),
            bounds: bounds_of(mesh.vertices.iter().copied()),
            color,
        });
    }

    fn stroke_path(&mut self, path: &Path, stroke: Stroke) {
        self.ops.push(PaintOp::Stroke {
            points: path.subpaths.iter().map(|s| s.points.len()).sum(),
            stroke,
            bounds: path.bounds(),
        });
    }

    fn draw_text(&mut self, text: &TextDraw<'_>) {
        self.ops.push(PaintOp::Text {
            text: text.text.to_owned(),
            pos: text.pos,
            size: text.size,
            color: text.color,
        });
    }

    fn image_effect(&mut self, rect: Rect, effect: ImageEffect) {
        self.ops.push(PaintOp::Effect { rect, effect });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_painter_captures_fills_and_strokes() {
        let mut p = RecordingPainter::new();
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        p.fill_path(&path, Color::red());
        p.stroke_path(&path, Stroke::new(2.0, Color::blue()));

        assert_eq!(p.meshes().count(), 1);
        assert_eq!(p.strokes().count(), 1);
        assert_eq!(
            p.drawn_bounds().unwrap(),
            Rect::from_xywh(0.0, 0.0, 10.0, 10.0)
        );
    }

    #[test]
    fn fully_transparent_fills_are_skipped() {
        let mut p = RecordingPainter::new();
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        p.fill_path(&path, Color::transparent());
        assert!(p.is_empty());
    }

    #[test]
    fn dim_outside_emits_four_bands_that_avoid_the_hole() {
        let mut p = RecordingPainter::new();
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::from_xywh(25.0, 25.0, 50.0, 50.0);
        p.dim_outside(hole, bounds, Color::black().with_alpha(128));

        assert_eq!(p.meshes().count(), 4);
        // The dimmed area is everything except the hole, counted once.
        let total: f32 = p
            .ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Mesh { bounds, .. } => bounds.map(|b| b.area()),
                _ => None,
            })
            .sum();
        assert!(
            (total - (100.0 * 100.0 - 50.0 * 50.0)).abs() < 1e-2,
            "{total}"
        );
    }

    #[test]
    fn dim_outside_with_an_empty_hole_covers_everything() {
        let mut p = RecordingPainter::new();
        let bounds = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        p.dim_outside(Rect::default(), bounds, Color::black());
        assert_eq!(p.meshes().count(), 1);
        assert_eq!(p.drawn_bounds().unwrap(), bounds);
    }

    #[test]
    fn dim_outside_clips_a_hole_that_pokes_out_of_bounds() {
        let mut p = RecordingPainter::new();
        let bounds = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        p.dim_outside(
            Rect::from_xywh(50.0, 50.0, 500.0, 500.0),
            bounds,
            Color::black(),
        );
        let total: f32 = p
            .ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Mesh { bounds, .. } => bounds.map(|b| b.area()),
                _ => None,
            })
            .sum();
        assert!((total - (10000.0 - 2500.0)).abs() < 1e-2, "{total}");
    }

    #[test]
    fn effect_visibility_thresholds() {
        assert!(ImageEffect::Blur { radius: 5.0 }.is_visible());
        assert!(!ImageEffect::Blur { radius: 0.1 }.is_visible());
        assert!(ImageEffect::Pixelate { block_size: 8.0 }.is_visible());
        assert!(!ImageEffect::Pixelate { block_size: 1.0 }.is_visible());
    }

    #[test]
    fn text_estimate_grows_with_lines_and_length() {
        let one = estimate_text_size("hello", 10.0);
        let two = estimate_text_size("hello\nworld!", 10.0);
        assert!(two.y > one.y);
        assert!(two.x > one.x);
        assert_eq!(estimate_text_size("", 10.0).y, 12.0);
    }
}
