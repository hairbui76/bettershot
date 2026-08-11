//! Freehand brush.
//!
//! Points are appended while dragging and thinned as they arrive: a raw
//! pointer stream at display refresh rate produces thousands of near-identical
//! points, which bloats both the scene and the exported render for no visual
//! gain.

use crate::drawable::{Drawable, distance_to_segment, stroke_bounds};
use crate::input::{Key, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::math::{Rect, Vec2D};
use crate::painter::Painter;
use crate::path::{Path, Stroke};
use crate::style::Style;

use super::shapes::cap_of;
use super::{Tool, ToolUpdateResult, Tools};

/// Minimum distance between consecutive recorded points, in image pixels.
const MIN_POINT_SPACING: f32 = 1.5;

#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub points: Vec<Vec2D>,
    pub style: Style,
}

impl Brush {
    pub fn new(style: Style) -> Self {
        Self {
            points: Vec::new(),
            style,
        }
    }

    /// Append a point unless it is too close to the previous one.
    pub fn push(&mut self, point: Vec2D) -> bool {
        match self.points.last() {
            Some(last) if last.distance_to(&point) < MIN_POINT_SPACING => false,
            _ => {
                self.points.push(point);
                true
            }
        }
    }

    /// A single click produces one point, which would stroke nothing; give it
    /// a second point so it renders as a dot.
    fn drawable_points(&self) -> Vec<Vec2D> {
        if self.points.len() == 1 {
            let p = self.points[0];
            vec![p, p + Vec2D::new(0.01, 0.0)]
        } else {
            self.points.clone()
        }
    }
}

impl Drawable for Brush {
    fn draw(&self, painter: &mut dyn Painter) {
        if self.points.is_empty() {
            return;
        }
        let mut path = Path::new();
        path.add_polyline(&self.drawable_points());
        painter.stroke_path(
            &path,
            Stroke::new(self.style.line_width(), self.style.color).with_cap(cap_of(&self.style)),
        );
    }

    fn bounds(&self) -> Option<Rect> {
        stroke_bounds(&self.points, self.style.line_width())
    }

    fn kind(&self) -> &'static str {
        "brush"
    }

    fn translate(&mut self, delta: Vec2D) {
        for p in &mut self.points {
            *p += delta;
        }
    }

    fn hit_test(&self, point: Vec2D) -> bool {
        let tolerance = self.style.line_width() / 2.0 + crate::drawable::HIT_TOLERANCE;
        match self.points.len() {
            0 => false,
            1 => self.points[0].distance_to(&point) <= tolerance,
            _ => self
                .points
                .windows(2)
                .any(|w| distance_to_segment(point, w[0], w[1]) <= tolerance),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrushTool {
    stroke: Option<Brush>,
    style: Style,
}

impl BrushTool {
    pub fn new(style: Style) -> Self {
        Self {
            stroke: None,
            style,
        }
    }
}

impl Tool for BrushTool {
    fn kind(&self) -> Tools {
        Tools::Brush
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }

        match event.kind {
            MouseEventKind::BeginDrag => {
                let mut brush = Brush::new(self.style);
                // Include the press point so the stroke starts where the user
                // pressed, not where the drag threshold was crossed.
                brush.push(event.start());
                brush.push(event.pos);
                self.stroke = Some(brush);
                ToolUpdateResult::Redraw
            }
            MouseEventKind::UpdateDrag => match &mut self.stroke {
                Some(brush) => {
                    if brush.push(event.pos) {
                        ToolUpdateResult::Redraw
                    } else {
                        ToolUpdateResult::Unmodified
                    }
                }
                None => ToolUpdateResult::Unmodified,
            },
            MouseEventKind::EndDrag => match self.stroke.take() {
                Some(mut brush) => {
                    brush.push(event.pos);
                    if brush.points.len() < 2 {
                        ToolUpdateResult::Redraw
                    } else {
                        ToolUpdateResult::Commit(Box::new(brush))
                    }
                }
                None => ToolUpdateResult::Unmodified,
            },
            MouseEventKind::Click => {
                // A plain click leaves a dot.
                let mut brush = Brush::new(self.style);
                brush.push(event.pos);
                ToolUpdateResult::Commit(Box::new(brush))
            }
            _ => ToolUpdateResult::Unmodified,
        }
    }

    fn handle_key_event(&mut self, event: KeyEvent) -> ToolUpdateResult {
        if event.key == Key::Escape && self.stroke.is_some() {
            self.stroke = None;
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_deactivated(&mut self) -> ToolUpdateResult {
        if self.stroke.take().is_some() {
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
        if let Some(brush) = &mut self.stroke {
            brush.style = style;
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        self.stroke.as_ref().map(|s| s as &dyn Drawable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::RecordingPainter;

    #[test]
    fn a_drag_records_a_stroke_through_every_far_enough_point() {
        let mut tool = BrushTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        for x in [10.0, 20.0, 30.0, 40.0] {
            let e = tracker.motion(Vec2D::new(x, 0.0), Modifiers::NONE);
            tool.handle_mouse_event(e);
        }
        let end = tracker
            .release(Vec2D::new(40.0, 0.0), Modifiers::NONE)
            .unwrap();
        let ToolUpdateResult::Commit(d) = tool.handle_mouse_event(end) else {
            panic!("expected a commit")
        };
        assert_eq!(d.kind(), "brush");
        assert!(d.hit_test(Vec2D::new(20.0, 0.0)));
        assert!(!tool.is_active());
    }

    #[test]
    fn nearly_identical_points_are_thinned_out() {
        let mut brush = Brush::new(Style::default());
        assert!(brush.push(Vec2D::ZERO));
        assert!(!brush.push(Vec2D::new(0.5, 0.0)), "too close to record");
        assert!(brush.push(Vec2D::new(5.0, 0.0)));
        assert_eq!(brush.points.len(), 2);
    }

    #[test]
    fn the_stroke_starts_at_the_press_point_not_the_drag_threshold() {
        let mut tool = BrushTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        let press = Vec2D::new(100.0, 100.0);
        tracker.press(press, MouseButton::Left, Modifiers::NONE);
        let begin = tracker.motion(Vec2D::new(110.0, 100.0), Modifiers::NONE);
        tool.handle_mouse_event(begin);

        let drawable = tool.drawable().unwrap();
        assert!(
            drawable.hit_test(press),
            "the press point should be part of the stroke"
        );
    }

    #[test]
    fn a_click_leaves_a_visible_dot() {
        let mut tool = BrushTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::new(7.0, 7.0), MouseButton::Left, Modifiers::NONE);
        let e = tracker
            .release(Vec2D::new(7.0, 7.0), Modifiers::NONE)
            .unwrap();
        let ToolUpdateResult::Commit(d) = tool.handle_mouse_event(e) else {
            panic!("a click should still leave a mark")
        };
        let mut p = RecordingPainter::new();
        d.draw(&mut p);
        assert_eq!(p.strokes().count(), 1, "a single point must still render");
        assert!(d.hit_test(Vec2D::new(7.0, 7.0)));
    }

    #[test]
    fn escape_discards_the_stroke_in_progress() {
        let mut tool = BrushTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let e = tracker.motion(Vec2D::new(30.0, 0.0), Modifiers::NONE);
        tool.handle_mouse_event(e);
        assert!(tool.is_active());
        tool.handle_key_event(KeyEvent::plain(Key::Escape));
        assert!(!tool.is_active());
    }

    #[test]
    fn hit_testing_follows_the_stroke_rather_than_its_bounding_box() {
        let brush = Brush {
            points: vec![
                Vec2D::new(0.0, 0.0),
                Vec2D::new(100.0, 0.0),
                Vec2D::new(100.0, 100.0),
            ],
            style: Style::default(),
        };
        assert!(brush.hit_test(Vec2D::new(50.0, 0.0)), "on the stroke");
        assert!(
            !brush.hit_test(Vec2D::new(20.0, 80.0)),
            "inside the bounding box but far from the stroke"
        );
    }

    #[test]
    fn translating_moves_every_point() {
        let mut brush = Brush {
            points: vec![Vec2D::ZERO, Vec2D::new(10.0, 10.0)],
            style: Style::default(),
        };
        brush.translate(Vec2D::new(5.0, 5.0));
        assert_eq!(brush.points[0], Vec2D::new(5.0, 5.0));
        assert_eq!(brush.points[1], Vec2D::new(15.0, 15.0));
    }

    #[test]
    fn an_empty_brush_draws_nothing() {
        let brush = Brush::new(Style::default());
        let mut p = RecordingPainter::new();
        brush.draw(&mut p);
        assert!(p.is_empty());
        assert!(brush.bounds().is_none());
    }
}
