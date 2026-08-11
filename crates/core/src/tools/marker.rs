//! Numbered step markers.
//!
//! Clicking drops a filled disc containing the next number in the sequence,
//! for annotating a screenshot as an ordered walkthrough.

use crate::drawable::Drawable;
use crate::input::{MouseButton, MouseEvent, MouseEventKind};
use crate::math::{Rect, Vec2D};
use crate::painter::{Painter, TextDraw};
use crate::path::Path;
use crate::style::Style;

use super::{Tool, ToolUpdateResult, Tools};

/// Disc radius as a fraction of the style's text size. Chosen so two digits
/// fit comfortably inside the circle.
const RADIUS_RATIO: f32 = 0.62;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub pos: Vec2D,
    pub number: u16,
    pub style: Style,
}

impl Marker {
    pub fn radius(&self) -> f32 {
        (self.style.text_size() * RADIUS_RATIO).max(1.0)
    }
}

impl Drawable for Marker {
    fn draw(&self, painter: &mut dyn Painter) {
        let mut path = Path::new();
        path.add_circle(self.pos, self.radius());
        painter.fill_path(&path, self.style.color);

        let label = self.number.to_string();
        painter.draw_text(
            &TextDraw::new(
                self.pos,
                &label,
                self.style.text_size(),
                // Black or white, whichever stays readable on the disc.
                self.style.color.contrast(),
            )
            .centered(),
        );
    }

    fn bounds(&self) -> Option<Rect> {
        let r = self.radius();
        Some(Rect::new(self.pos - Vec2D::splat(r), Vec2D::splat(r * 2.0)))
    }

    fn kind(&self) -> &'static str {
        "marker"
    }

    fn sequence_number(&self) -> Option<u16> {
        Some(self.number)
    }

    fn translate(&mut self, delta: Vec2D) {
        self.pos += delta;
    }

    fn hit_test(&self, point: Vec2D) -> bool {
        self.pos.distance_to(&point) <= self.radius() + crate::drawable::HIT_TOLERANCE
    }
}

#[derive(Debug, Clone)]
pub struct MarkerTool {
    style: Style,
    next_number: u16,
    /// Preview shown while dragging to reposition before release.
    preview: Option<Marker>,
}

impl MarkerTool {
    pub fn new(style: Style) -> Self {
        Self {
            style,
            next_number: 1,
            preview: None,
        }
    }

    pub fn next_number(&self) -> u16 {
        self.next_number
    }

    /// Resync the counter, e.g. after an undo removed the highest marker.
    pub fn set_next_number(&mut self, number: u16) {
        self.next_number = number.max(1);
    }

    fn take_number(&mut self) -> u16 {
        let n = self.next_number;
        self.next_number = self.next_number.saturating_add(1);
        n
    }
}

impl Tool for MarkerTool {
    fn kind(&self) -> Tools {
        Tools::Marker
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }

        match event.kind {
            MouseEventKind::Click => {
                let marker = Marker {
                    pos: event.pos,
                    number: self.take_number(),
                    style: self.style,
                };
                self.preview = None;
                ToolUpdateResult::Commit(Box::new(marker))
            }
            // Dragging lets the marker be positioned before it is dropped.
            MouseEventKind::BeginDrag | MouseEventKind::UpdateDrag => {
                self.preview = Some(Marker {
                    pos: event.pos,
                    number: self.next_number,
                    style: self.style,
                });
                ToolUpdateResult::Redraw
            }
            MouseEventKind::EndDrag => {
                // Only drop a marker if one was being positioned. Escape
                // clears the preview, and the release that follows must not
                // place it anyway.
                if self.preview.take().is_none() {
                    return ToolUpdateResult::Unmodified;
                }
                let marker = Marker {
                    pos: event.pos,
                    number: self.take_number(),
                    style: self.style,
                };
                ToolUpdateResult::Commit(Box::new(marker))
            }
            _ => ToolUpdateResult::Unmodified,
        }
    }

    fn handle_style_event(&mut self, style: Style) -> ToolUpdateResult {
        self.style = style;
        if let Some(preview) = &mut self.preview {
            preview.style = style;
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_deactivated(&mut self) -> ToolUpdateResult {
        if self.preview.take().is_some() {
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_dismissed(&mut self) -> ToolUpdateResult {
        self.handle_deactivated()
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        self.preview.as_ref().map(|m| m as &dyn Drawable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::{PaintOp, RecordingPainter};
    use crate::style::Color;

    fn click(tool: &mut dyn Tool, at: Vec2D) -> ToolUpdateResult {
        let mut tracker = PointerTracker::new();
        tracker.press(at, MouseButton::Left, Modifiers::NONE);
        let e = tracker.release(at, Modifiers::NONE).unwrap();
        tool.handle_mouse_event(e)
    }

    #[test]
    fn clicks_drop_markers_numbered_in_sequence() {
        let mut tool = MarkerTool::new(Style::default());
        for expected in 1..=3u16 {
            let ToolUpdateResult::Commit(d) =
                click(&mut tool, Vec2D::new(10.0 * expected as f32, 0.0))
            else {
                panic!("expected a commit")
            };
            assert_eq!(d.kind(), "marker");
            assert_eq!(tool.next_number(), expected + 1);
        }
    }

    #[test]
    fn the_counter_can_be_resynced_after_an_undo() {
        let mut tool = MarkerTool::new(Style::default());
        click(&mut tool, Vec2D::ZERO);
        click(&mut tool, Vec2D::ZERO);
        assert_eq!(tool.next_number(), 3);
        tool.set_next_number(2);
        assert_eq!(tool.next_number(), 2);
        // Never drops below 1, even if asked to.
        tool.set_next_number(0);
        assert_eq!(tool.next_number(), 1);
    }

    #[test]
    fn a_marker_draws_a_disc_and_its_number() {
        let marker = Marker {
            pos: Vec2D::new(50.0, 50.0),
            number: 7,
            style: Style::default(),
        };
        let mut p = RecordingPainter::new();
        marker.draw(&mut p);
        assert_eq!(p.meshes().count(), 1, "the disc");
        let Some(PaintOp::Text { text, pos, .. }) = p.texts().next() else {
            panic!("expected the number to be drawn")
        };
        assert_eq!(text, "7");
        assert_eq!(*pos, marker.pos, "the label is centred on the disc");
    }

    #[test]
    fn the_number_is_drawn_in_a_contrasting_colour() {
        let on_dark = Marker {
            pos: Vec2D::ZERO,
            number: 1,
            style: Style::default().with_color(Color::black()),
        };
        let mut p = RecordingPainter::new();
        on_dark.draw(&mut p);
        let Some(PaintOp::Text { color, .. }) = p.texts().next() else {
            panic!("expected text")
        };
        assert_eq!(*color, Color::white());
    }

    #[test]
    fn hit_testing_is_circular() {
        let marker = Marker {
            pos: Vec2D::new(100.0, 100.0),
            number: 1,
            style: Style::default(),
        };
        assert!(marker.hit_test(Vec2D::new(100.0, 100.0)));
        let r = marker.radius();
        assert!(!marker.hit_test(Vec2D::new(100.0 + r * 3.0, 100.0)));
    }

    #[test]
    fn dragging_previews_without_consuming_a_number_until_release() {
        let mut tool = MarkerTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(40.0, 0.0), Modifiers::NONE);
        tool.handle_mouse_event(m);
        assert!(tool.drawable().is_some(), "should preview while dragging");
        assert_eq!(tool.next_number(), 1, "number not consumed yet");

        let end = tracker
            .release(Vec2D::new(40.0, 0.0), Modifiers::NONE)
            .unwrap();
        assert!(tool.handle_mouse_event(end).is_commit());
        assert_eq!(tool.next_number(), 2);
        assert!(tool.drawable().is_none());
    }

    #[test]
    fn larger_sizes_produce_larger_discs() {
        let small = Marker {
            pos: Vec2D::ZERO,
            number: 1,
            style: Style::default().with_size(crate::style::Size::Small),
        };
        let large = Marker {
            style: Style::default().with_size(crate::style::Size::Large),
            ..small
        };
        assert!(large.radius() > small.radius());
    }
}
