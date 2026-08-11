//! Line and arrow tools.
//!
//! The arrow geometry is ported from Satty (`src/tools/arrow.rs`), MPL-2.0,
//! Copyright the Satty authors. Satty builds it by rotating the canvas; here it
//! is built in a local frame and then rotated into image space, so the shape is
//! computable (and testable) without a renderer.

use crate::drawable::{Drawable, distance_to_segment, stroke_bounds};
use crate::input::{Key, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::math::{Angle, Rect, Vec2D};
use crate::painter::Painter;
use crate::path::{Path, Stroke};
use crate::style::Style;

use super::shapes::cap_of;
use super::{Tool, ToolUpdateResult, Tools};

/// A shape defined by a start and end point.
pub trait LineShape: Drawable + Clone + 'static {
    const KIND: Tools;
    fn from_points(start: Vec2D, end: Vec2D, style: Style) -> Self;
    /// Shorter drags than this are treated as accidental clicks.
    const MIN_LENGTH: f32 = 2.0;
}

#[derive(Debug, Clone)]
pub struct LineDragTool<S: LineShape> {
    shape: Option<S>,
    style: Style,
}

impl<S: LineShape> LineDragTool<S> {
    pub fn new(style: Style) -> Self {
        Self { shape: None, style }
    }

    /// End point for the current drag, applying Shift-to-15°.
    fn end_for(event: &MouseEvent) -> Vec2D {
        let delta = if event.modifiers.shift {
            event.delta.snapped_vector_15deg()
        } else {
            event.delta
        };
        event.start() + delta
    }
}

impl<S: LineShape> Tool for LineDragTool<S> {
    fn kind(&self) -> Tools {
        S::KIND
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }

        match event.kind {
            MouseEventKind::BeginDrag | MouseEventKind::UpdateDrag => {
                self.shape = Some(S::from_points(
                    event.start(),
                    Self::end_for(&event),
                    self.style,
                ));
                ToolUpdateResult::Redraw
            }
            MouseEventKind::EndDrag => {
                // See the note in `shapes.rs`: a release must not rebuild a
                // shape the tool is no longer drawing.
                if self.shape.take().is_none() {
                    return ToolUpdateResult::Unmodified;
                }
                let start = event.start();
                let end = Self::end_for(&event);
                if start.distance_to(&end) < S::MIN_LENGTH {
                    return ToolUpdateResult::Redraw;
                }
                ToolUpdateResult::Commit(Box::new(S::from_points(start, end, self.style)))
            }
            _ => ToolUpdateResult::Unmodified,
        }
    }

    fn handle_key_event(&mut self, event: KeyEvent) -> ToolUpdateResult {
        if event.key == Key::Escape && self.shape.is_some() {
            self.shape = None;
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_deactivated(&mut self) -> ToolUpdateResult {
        if self.shape.take().is_some() {
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_dismissed(&mut self) -> ToolUpdateResult {
        self.handle_deactivated()
    }

    fn handle_style_event(&mut self, style: Style) -> ToolUpdateResult {
        self.style = style;
        ToolUpdateResult::Unmodified
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        self.shape.as_ref().map(|s| s as &dyn Drawable)
    }
}

// --- Line -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub start: Vec2D,
    pub end: Vec2D,
    pub style: Style,
}

impl LineShape for Line {
    const KIND: Tools = Tools::Line;
    fn from_points(start: Vec2D, end: Vec2D, style: Style) -> Self {
        Self { start, end, style }
    }
}

impl Drawable for Line {
    fn draw(&self, painter: &mut dyn Painter) {
        let mut path = Path::new();
        path.add_polyline(&[self.start, self.end]);
        painter.stroke_path(
            &path,
            Stroke::new(self.style.line_width(), self.style.color).with_cap(cap_of(&self.style)),
        );
    }

    fn bounds(&self) -> Option<Rect> {
        stroke_bounds(&[self.start, self.end], self.style.line_width())
    }

    fn kind(&self) -> &'static str {
        "line"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.start += delta;
        self.end += delta;
    }

    fn hit_test(&self, point: Vec2D) -> bool {
        distance_to_segment(point, self.start, self.end)
            <= self.style.line_width() / 2.0 + crate::drawable::HIT_TOLERANCE
    }
}

// --- Arrow ------------------------------------------------------------------

/// Interior angle at the arrow tip.
const HEAD_ANGLE_DEGREES: f32 = 60.0;
/// How far the tail/head junction slides toward the tip, as a fraction of the
/// head side length. Positive sharpens the head; negative would make a diamond.
const MIDPOINT_OFFSET_RATIO: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrow {
    pub start: Vec2D,
    pub end: Vec2D,
    pub style: Style,
}

impl LineShape for Arrow {
    const KIND: Tools = Tools::Arrow;
    fn from_points(start: Vec2D, end: Vec2D, style: Style) -> Self {
        Self { start, end, style }
    }
}

impl Arrow {
    pub fn length(&self) -> f32 {
        self.start.distance_to(&self.end)
    }

    fn line_width(&self) -> f32 {
        // A filled arrow with butt caps is pure fill: no outline stroke, so the
        // geometry is not inset.
        if self.style.round_caps || !self.style.fill {
            self.style.line_width()
        } else {
            0.0
        }
    }

    /// The outline of a filled ("fat") arrow, in image space.
    ///
    /// Local frame: the arrow runs along +x from the origin.
    /// ```text
    ///           C
    ///   E       #
    ///     ######G###
    ///   A ######D##### B
    ///     ##########
    ///   F       #
    /// ```
    /// A start, B tip, C head corners, E/F tail corners, G tail/head junction.
    fn filled_path(&self) -> Option<Path> {
        let line_width = self.line_width();
        let arrow_length = self.length() - line_width / 2.0;
        if arrow_length <= f32::EPSILON {
            return None;
        }

        let tail_width = (self.style.arrow_tail_width() - line_width).max(0.0);
        let head_side_length = (self.style.arrow_head_length() - line_width).max(0.0);
        let midpoint_offset = head_side_length * MIDPOINT_OFFSET_RATIO;

        let head_half_angle = Angle::from_degrees(HEAD_ANGLE_DEGREES) * 0.5;
        let head_left =
            Vec2D::new(arrow_length, 0.0) - Vec2D::from_angle(head_half_angle) * head_side_length;
        let midpoint_x = head_left.x + midpoint_offset;

        let tail_half_head = tail_width / 2.0;
        let tail_half_end = if self.style.round_caps {
            line_width / 10.0
        } else {
            tail_width / 2.0
        };

        let mut points = vec![
            Vec2D::new(midpoint_x, tail_half_head),  // G
            Vec2D::new(head_left.x, -head_left.y),   // C
            Vec2D::new(arrow_length, 0.0),           // B (tip)
            Vec2D::new(head_left.x, head_left.y),    // C mirrored
            Vec2D::new(midpoint_x, -tail_half_head), // G mirrored
        ];
        if midpoint_x > line_width / 2.0 {
            // Otherwise the head swallows the tail entirely and these two
            // points would fold the polygon back on itself.
            points.push(Vec2D::new(line_width / 2.0, -tail_half_end)); // F
            points.push(Vec2D::new(line_width / 2.0, tail_half_end)); // E
        }

        let mut path = Path::new();
        path.add_polygon(&points);
        Some(
            path.rotated_around(Vec2D::ZERO, (self.end - self.start).angle())
                .translated(self.start),
        )
    }

    /// The two strokes of an outlined ("thin") arrow: the head chevron and the
    /// shaft.
    fn outlined_path(&self) -> Option<Path> {
        let line_width = self.line_width();
        let arrow_length = self.length() - line_width;
        if arrow_length <= f32::EPSILON {
            return None;
        }

        let head_side_length = (self.style.arrow_head_length() - line_width).max(0.0);
        let head_half_angle = Angle::from_degrees(HEAD_ANGLE_DEGREES) * 0.5;
        let head_left =
            Vec2D::new(arrow_length, 0.0) - Vec2D::from_angle(head_half_angle) * head_side_length;
        let shaft_start = if self.style.round_caps {
            line_width / 2.0
        } else {
            0.0
        };

        let mut path = Path::new();
        path.add_polyline(&[
            Vec2D::new(head_left.x, -head_left.y),
            Vec2D::new(arrow_length, 0.0),
            Vec2D::new(head_left.x, head_left.y),
        ]);
        path.add_polyline(&[Vec2D::new(shaft_start, 0.0), Vec2D::new(arrow_length, 0.0)]);
        Some(
            path.rotated_around(Vec2D::ZERO, (self.end - self.start).angle())
                .translated(self.start),
        )
    }

    /// The path this arrow renders as, whichever style is active.
    pub fn path(&self) -> Option<Path> {
        if self.style.fill {
            self.filled_path()
        } else {
            self.outlined_path()
        }
    }
}

impl Drawable for Arrow {
    fn draw(&self, painter: &mut dyn Painter) {
        let Some(path) = self.path() else { return };
        if self.style.fill {
            painter.fill_path(&path, self.style.color);
            if self.style.round_caps {
                // Stroking the same outline rounds off the corners, matching
                // Satty's round-join treatment of the fat arrow.
                painter.stroke_path(
                    &path,
                    Stroke::new(self.style.line_width(), self.style.color)
                        .with_cap(crate::path::LineCap::Round),
                );
            }
        } else {
            painter.stroke_path(
                &path,
                Stroke::new(self.style.line_width(), self.style.color)
                    .with_cap(cap_of(&self.style)),
            );
        }
    }

    fn bounds(&self) -> Option<Rect> {
        match self.path().and_then(|p| p.bounds()) {
            Some(b) => Some(b.expanded(self.style.line_width() / 2.0)),
            // Degenerate (zero-length) arrows still occupy their endpoints.
            None => stroke_bounds(&[self.start, self.end], self.style.line_width()),
        }
    }

    fn kind(&self) -> &'static str {
        "arrow"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.start += delta;
        self.end += delta;
    }

    fn hit_test(&self, point: Vec2D) -> bool {
        // The shaft dominates; the head is covered by the generous tolerance.
        distance_to_segment(point, self.start, self.end)
            <= self.style.arrow_tail_width().max(self.style.line_width()) / 2.0
                + crate::drawable::HIT_TOLERANCE
    }
}

pub type LineTool = LineDragTool<Line>;
pub type ArrowTool = LineDragTool<Arrow>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::RecordingPainter;
    use crate::style::Size;

    fn drag(tool: &mut dyn Tool, from: Vec2D, to: Vec2D, modifiers: Modifiers) -> ToolUpdateResult {
        let mut tracker = PointerTracker::new();
        tracker.press(from, MouseButton::Left, modifiers);
        let m = tracker.motion(to, modifiers);
        tool.handle_mouse_event(m);
        let end = tracker.release(to, modifiers).unwrap();
        tool.handle_mouse_event(end)
    }

    #[test]
    fn dragging_commits_a_line_between_the_endpoints() {
        let mut tool = LineTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::new(0.0, 0.0),
            Vec2D::new(100.0, 0.0),
            Modifiers::NONE,
        );
        let ToolUpdateResult::Commit(d) = result else {
            panic!("expected a commit, got {result:?}")
        };
        assert_eq!(d.kind(), "line");
        assert!(d.hit_test(Vec2D::new(50.0, 0.0)));
        assert!(!d.hit_test(Vec2D::new(50.0, 500.0)));
    }

    #[test]
    fn shift_snaps_the_line_angle_to_15_degree_steps() {
        let mut tool = LineTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::ZERO,
            Vec2D::new(100.0, 8.0),
            Modifiers::shift(),
        );
        let ToolUpdateResult::Commit(d) = result else {
            panic!("expected a commit")
        };
        let bounds = d.bounds().unwrap();
        // 4.6° snaps to 0°, so the line is horizontal: its height is only the
        // stroke width.
        assert!(bounds.height() < 10.0, "got {bounds:?}");
    }

    #[test]
    fn a_click_without_movement_commits_nothing() {
        let mut tool = LineTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let e = tracker.release(Vec2D::ZERO, Modifiers::NONE).unwrap();
        assert!(!tool.handle_mouse_event(e).is_commit());
    }

    #[test]
    fn filled_arrow_geometry_spans_start_to_end() {
        let arrow = Arrow {
            start: Vec2D::new(10.0, 10.0),
            end: Vec2D::new(210.0, 10.0),
            style: Style::default().with_fill(true),
        };
        let path = arrow.path().expect("arrow should have geometry");
        let bounds = path.bounds().unwrap();
        // The tip reaches (nearly) the end point and the tail starts at the
        // start point.
        assert!(bounds.right() <= 210.5, "tip overshoots: {bounds:?}");
        assert!(bounds.right() > 200.0, "tip falls short: {bounds:?}");
        assert!(bounds.left() >= 9.0 && bounds.left() < 15.0, "{bounds:?}");
        // The head is wider than the tail.
        assert!(bounds.height() >= arrow.style.arrow_tail_width());
    }

    #[test]
    fn filled_arrow_is_a_single_closed_polygon_that_triangulates() {
        let arrow = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(200.0, 0.0),
            style: Style::default().with_fill(true),
        };
        let path = arrow.path().unwrap();
        assert_eq!(path.subpaths.len(), 1);
        assert!(path.subpaths[0].closed);
        let mesh = path.fill_mesh();
        assert!(
            mesh.triangle_count() >= 3,
            "concave arrow should tessellate into several triangles, got {}",
            mesh.triangle_count()
        );
    }

    #[test]
    fn outlined_arrow_draws_a_chevron_and_a_shaft() {
        let arrow = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(200.0, 0.0),
            style: Style::default().with_fill(false),
        };
        let path = arrow.path().unwrap();
        assert_eq!(path.subpaths.len(), 2, "head chevron plus shaft");
        assert!(path.subpaths.iter().all(|s| !s.closed));
    }

    #[test]
    fn arrows_rotate_with_their_direction() {
        let style = Style::default().with_fill(true);
        let horizontal = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(200.0, 0.0),
            style,
        };
        let vertical = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(0.0, 200.0),
            style,
        };
        let h = horizontal.path().unwrap().bounds().unwrap();
        let v = vertical.path().unwrap().bounds().unwrap();
        // The bounding boxes should be transposes of one another.
        assert!((h.width() - v.height()).abs() < 1.0, "{h:?} vs {v:?}");
        assert!((h.height() - v.width()).abs() < 1.0, "{h:?} vs {v:?}");
    }

    #[test]
    fn a_zero_length_arrow_draws_nothing_but_still_has_bounds() {
        let arrow = Arrow {
            start: Vec2D::new(5.0, 5.0),
            end: Vec2D::new(5.0, 5.0),
            style: Style::default().with_fill(true),
        };
        assert!(arrow.path().is_none());
        let mut p = RecordingPainter::new();
        arrow.draw(&mut p);
        assert!(p.is_empty());
        assert!(arrow.bounds().is_some());
    }

    #[test]
    fn a_very_short_arrow_is_all_head_and_still_valid() {
        // Shorter than the head length: the tail points are dropped, which
        // must not produce a self-folding polygon.
        let arrow = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(12.0, 0.0),
            style: Style::default().with_fill(true).with_size(Size::Large),
        };
        let path = arrow.path().unwrap();
        assert_eq!(path.subpaths[0].points.len(), 5, "tail points dropped");
        assert!(!path.fill_mesh().is_empty());
    }

    #[test]
    fn larger_sizes_produce_larger_arrow_heads() {
        let make = |size| Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(300.0, 0.0),
            style: Style::default().with_fill(true).with_size(size),
        };
        let small = make(Size::Small).path().unwrap().bounds().unwrap();
        let large = make(Size::Large).path().unwrap().bounds().unwrap();
        assert!(large.height() > small.height());
    }

    #[test]
    fn arrow_size_factor_scales_the_whole_shape() {
        let base = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(300.0, 0.0),
            style: Style {
                fill: true,
                annotation_size_factor: 1.0,
                ..Default::default()
            },
        };
        let scaled = Arrow {
            style: Style {
                annotation_size_factor: 2.0,
                ..base.style
            },
            ..base
        };
        assert!(
            scaled.path().unwrap().bounds().unwrap().height()
                > base.path().unwrap().bounds().unwrap().height()
        );
    }

    #[test]
    fn translating_an_arrow_moves_both_ends() {
        let mut a = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(10.0, 0.0),
            style: Style::default(),
        };
        a.translate(Vec2D::new(3.0, 4.0));
        assert_eq!(a.start, Vec2D::new(3.0, 4.0));
        assert_eq!(a.end, Vec2D::new(13.0, 4.0));
    }

    #[test]
    fn filled_arrows_with_round_caps_also_stroke_their_outline() {
        let arrow = Arrow {
            start: Vec2D::ZERO,
            end: Vec2D::new(200.0, 0.0),
            style: Style {
                fill: true,
                round_caps: true,
                ..Default::default()
            },
        };
        let mut p = RecordingPainter::new();
        arrow.draw(&mut p);
        assert_eq!(p.meshes().count(), 1);
        assert_eq!(p.strokes().count(), 1, "round caps need the outline stroke");
    }
}
