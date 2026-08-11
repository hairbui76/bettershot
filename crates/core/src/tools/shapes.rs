//! Rectangle-drag tools: rectangle, ellipse, highlight and blur.
//!
//! All four share one gesture — press, drag, release — so they share one state
//! machine, [`RectDragTool`], parameterized by the shape it produces.

use crate::drawable::Drawable;
use crate::input::{Key, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::math::{Rect, Vec2D};
use crate::painter::{ImageEffect, Painter};
use crate::path::{LineCap, Path, Stroke};
use crate::style::Style;

use super::{Tool, ToolUpdateResult, Tools};

/// Alpha applied to the highlight colour so the screenshot stays readable
/// underneath.
pub const HIGHLIGHT_ALPHA: u8 = 100;

/// A shape defined entirely by a rectangle and a style.
pub trait RectShape: Drawable + Clone + 'static {
    const KIND: Tools;
    fn from_rect(rect: Rect, style: Style) -> Self;
    /// Smallest side length (image pixels) worth committing; smaller drags are
    /// treated as accidental and discarded.
    const MIN_EXTENT: f32 = 2.0;
}

/// Shared press/drag/release state machine for the rectangle-based tools.
#[derive(Debug, Clone)]
pub struct RectDragTool<S: RectShape> {
    shape: Option<S>,
    style: Style,
    /// The rectangle the current drag defines, kept separately because a
    /// shape's `bounds()` includes its stroke expansion — rebuilding from
    /// bounds grew the preview a little every time the style changed.
    rect: Rect,
}

impl<S: RectShape> RectDragTool<S> {
    pub fn new(style: Style) -> Self {
        Self {
            shape: None,
            style,
            rect: Rect::default(),
        }
    }

    /// The rect for the current drag, applying Shift-to-square.
    fn rect_for(event: &MouseEvent) -> Rect {
        let delta = if event.modifiers.shift {
            event.delta.snapped_square()
        } else {
            event.delta
        };
        Rect::from_corners(event.start(), event.start() + delta)
    }
}

impl<S: RectShape> Tool for RectDragTool<S> {
    fn kind(&self) -> Tools {
        S::KIND
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        // Middle-drag is reserved for panning by the shell.
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }

        match event.kind {
            MouseEventKind::BeginDrag | MouseEventKind::UpdateDrag => {
                self.rect = Self::rect_for(&event);
                self.shape = Some(S::from_rect(self.rect, self.style));
                ToolUpdateResult::Redraw
            }
            MouseEventKind::EndDrag => {
                // A release only completes a shape this tool was actually
                // drawing. Rebuilding one from the event alone would resurrect
                // work the user had already cancelled with Escape, or invent a
                // shape from a stray release.
                if self.shape.take().is_none() {
                    return ToolUpdateResult::Unmodified;
                }
                let rect = Self::rect_for(&event);
                if rect.width() < S::MIN_EXTENT || rect.height() < S::MIN_EXTENT {
                    // Too small to be intentional.
                    return ToolUpdateResult::Redraw;
                }
                ToolUpdateResult::Commit(Box::new(S::from_rect(rect, self.style)))
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
        // Drop the unfinished shape rather than committing a half-drag.
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
        // Restyle the in-progress shape so the preview matches the toolbar,
        // rebuilding from the dragged rectangle rather than from the shape's
        // bounds, which are inflated by the stroke width.
        if self.shape.is_some() {
            self.shape = Some(S::from_rect(self.rect, style));
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        self.shape.as_ref().map(|s| s as &dyn Drawable)
    }
}

// --- Rectangle --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    pub rect: Rect,
    pub style: Style,
}

impl RectShape for Rectangle {
    const KIND: Tools = Tools::Rectangle;
    fn from_rect(rect: Rect, style: Style) -> Self {
        Self { rect, style }
    }
}

impl Drawable for Rectangle {
    fn draw(&self, painter: &mut dyn Painter) {
        let mut path = Path::new();
        path.add_rect(self.rect);
        if self.style.fill {
            painter.fill_path(&path, self.style.color);
        } else {
            painter.stroke_path(
                &path,
                Stroke::new(self.style.line_width(), self.style.color)
                    .with_cap(cap_of(&self.style)),
            );
        }
    }

    fn bounds(&self) -> Option<Rect> {
        Some(self.rect.expanded(self.style.line_width() / 2.0))
    }

    fn kind(&self) -> &'static str {
        "rectangle"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.rect = self.rect.translated(delta);
    }
}

// --- Ellipse ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipse {
    pub rect: Rect,
    pub style: Style,
}

impl RectShape for Ellipse {
    const KIND: Tools = Tools::Ellipse;
    fn from_rect(rect: Rect, style: Style) -> Self {
        Self { rect, style }
    }
}

impl Drawable for Ellipse {
    fn draw(&self, painter: &mut dyn Painter) {
        let mut path = Path::new();
        path.add_ellipse(self.rect);
        if self.style.fill {
            painter.fill_path(&path, self.style.color);
        } else {
            painter.stroke_path(
                &path,
                Stroke::new(self.style.line_width(), self.style.color)
                    .with_cap(cap_of(&self.style)),
            );
        }
    }

    fn bounds(&self) -> Option<Rect> {
        Some(self.rect.expanded(self.style.line_width() / 2.0))
    }

    fn kind(&self) -> &'static str {
        "ellipse"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.rect = self.rect.translated(delta);
    }

    fn hit_test(&self, point: Vec2D) -> bool {
        // Proper ellipse test, so clicks in the corners of the bounding box
        // fall through to whatever is underneath.
        let r = self.rect.normalized();
        let radii = r.size * 0.5;
        if radii.x <= 0.0 || radii.y <= 0.0 {
            return false;
        }
        let d = point - r.center();
        let tol = crate::drawable::HIT_TOLERANCE;
        let nx = d.x / (radii.x + tol);
        let ny = d.y / (radii.y + tol);
        nx * nx + ny * ny <= 1.0
    }
}

// --- Highlight --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Highlight {
    pub rect: Rect,
    pub style: Style,
}

impl RectShape for Highlight {
    const KIND: Tools = Tools::Highlight;
    fn from_rect(rect: Rect, style: Style) -> Self {
        Self { rect, style }
    }
}

impl Drawable for Highlight {
    fn draw(&self, painter: &mut dyn Painter) {
        let mut path = Path::new();
        path.add_rect(self.rect);
        painter.fill_path(&path, self.style.color.with_alpha(HIGHLIGHT_ALPHA));
    }

    fn bounds(&self) -> Option<Rect> {
        Some(self.rect)
    }

    fn kind(&self) -> &'static str {
        "highlight"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.rect = self.rect.translated(delta);
    }
}

// --- Blur -------------------------------------------------------------------

/// How a blur region obscures its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObscureKind {
    #[default]
    Blur,
    Pixelate,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blur {
    pub rect: Rect,
    pub style: Style,
    pub obscure: ObscureKind,
}

impl RectShape for Blur {
    const KIND: Tools = Tools::Blur;
    fn from_rect(rect: Rect, style: Style) -> Self {
        Self {
            rect,
            style,
            obscure: ObscureKind::default(),
        }
    }
}

impl Blur {
    pub fn effect(&self) -> ImageEffect {
        let strength = self.style.blur_factor();
        match self.obscure {
            ObscureKind::Blur => ImageEffect::Blur { radius: strength },
            ObscureKind::Pixelate => ImageEffect::Pixelate {
                block_size: strength,
            },
        }
    }
}

impl Drawable for Blur {
    fn draw(&self, painter: &mut dyn Painter) {
        let effect = self.effect();
        if effect.is_visible() && !self.rect.is_empty() {
            painter.image_effect(self.rect, effect);
        }
    }

    fn bounds(&self) -> Option<Rect> {
        Some(self.rect)
    }

    fn kind(&self) -> &'static str {
        "blur"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.rect = self.rect.translated(delta);
    }
}

pub(crate) fn cap_of(style: &Style) -> LineCap {
    if style.round_caps {
        LineCap::Round
    } else {
        LineCap::Butt
    }
}

pub type RectangleTool = RectDragTool<Rectangle>;
pub type EllipseTool = RectDragTool<Ellipse>;
pub type HighlightTool = RectDragTool<Highlight>;
pub type BlurTool = RectDragTool<Blur>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::RecordingPainter;
    use crate::style::Color;

    fn drag(tool: &mut dyn Tool, from: Vec2D, to: Vec2D, modifiers: Modifiers) -> ToolUpdateResult {
        let mut tracker = PointerTracker::new();
        tracker.press(from, MouseButton::Left, modifiers);
        let begin = tracker.motion(to, modifiers);
        tool.handle_mouse_event(begin);
        let end = tracker.release(to, modifiers).unwrap();
        tool.handle_mouse_event(end)
    }

    #[test]
    fn a_drag_commits_a_rectangle_with_the_dragged_bounds() {
        let mut tool = RectangleTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::new(10.0, 20.0),
            Vec2D::new(60.0, 50.0),
            Modifiers::NONE,
        );
        match result {
            ToolUpdateResult::Commit(d) => {
                assert_eq!(d.kind(), "rectangle");
                let b = d.bounds().unwrap();
                // Bounds include half the stroke, so compare the centre/size
                // with tolerance for the stroke expansion.
                assert!((b.center().x - 35.0).abs() < 3.0);
                assert!((b.center().y - 35.0).abs() < 3.0);
            }
            other => panic!("expected a commit, got {other:?}"),
        }
        assert!(!tool.is_active(), "tool should reset after committing");
    }

    #[test]
    fn dragging_backwards_still_produces_a_positive_rect() {
        let mut tool = RectangleTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::new(60.0, 50.0),
            Vec2D::new(10.0, 20.0),
            Modifiers::NONE,
        );
        let ToolUpdateResult::Commit(d) = result else {
            panic!("expected a commit")
        };
        let b = d.bounds().unwrap();
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }

    #[test]
    fn shift_snaps_the_rectangle_to_a_square() {
        let mut tool = RectangleTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::ZERO,
            Vec2D::new(100.0, 30.0),
            Modifiers::shift(),
        );
        let ToolUpdateResult::Commit(d) = result else {
            panic!("expected a commit")
        };
        let b = d.bounds().unwrap();
        assert!(
            (b.width() - b.height()).abs() < 0.01,
            "expected a square, got {}x{}",
            b.width(),
            b.height()
        );
    }

    #[test]
    fn a_tiny_drag_is_discarded_rather_than_committed() {
        let mut tool = RectangleTool::new(Style::default());
        let result = drag(
            &mut tool,
            Vec2D::ZERO,
            Vec2D::new(4.0, 0.5),
            Modifiers::NONE,
        );
        assert!(!result.is_commit(), "got {result:?}");
    }

    #[test]
    fn escape_cancels_the_in_progress_shape() {
        let mut tool = RectangleTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(50.0, 50.0), Modifiers::NONE);
        tool.handle_mouse_event(m);
        assert!(tool.is_active());

        let r = tool.handle_key_event(KeyEvent::plain(Key::Escape));
        assert!(r.needs_redraw());
        assert!(!tool.is_active());
    }

    #[test]
    fn deactivating_drops_unfinished_work() {
        let mut tool = EllipseTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(50.0, 50.0), Modifiers::NONE);
        tool.handle_mouse_event(m);
        assert!(tool.is_active());
        tool.handle_deactivated();
        assert!(!tool.is_active());
    }

    #[test]
    fn middle_button_drags_are_ignored_so_the_shell_can_pan() {
        let mut tool = RectangleTool::new(Style::default());
        let event = MouseEvent {
            kind: MouseEventKind::BeginDrag,
            pos: Vec2D::new(10.0, 10.0),
            delta: Vec2D::new(10.0, 10.0),
            button: MouseButton::Middle,
            modifiers: Modifiers::NONE,
        };
        assert!(!tool.handle_mouse_event(event).needs_redraw());
        assert!(!tool.is_active());
    }

    #[test]
    fn filled_and_outlined_rectangles_paint_differently() {
        let filled = Rectangle {
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            style: Style::default().with_fill(true),
        };
        let outlined = Rectangle {
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            style: Style::default().with_fill(false),
        };

        let mut p = RecordingPainter::new();
        filled.draw(&mut p);
        assert_eq!(p.meshes().count(), 1);
        assert_eq!(p.strokes().count(), 0);

        let mut p = RecordingPainter::new();
        outlined.draw(&mut p);
        assert_eq!(p.meshes().count(), 0);
        assert_eq!(p.strokes().count(), 1);
    }

    #[test]
    fn highlight_is_drawn_translucent() {
        let h = Highlight {
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            style: Style::default(),
        };
        let mut p = RecordingPainter::new();
        h.draw(&mut p);
        let Some(crate::painter::PaintOp::Mesh { color, .. }) = p.ops.first() else {
            panic!("expected a filled mesh")
        };
        assert_eq!(color.a, HIGHLIGHT_ALPHA);
    }

    #[test]
    fn blur_emits_an_image_effect_scaled_by_size() {
        let small = Blur {
            rect: Rect::from_xywh(0.0, 0.0, 50.0, 50.0),
            style: Style::default().with_size(crate::style::Size::Small),
            obscure: ObscureKind::Blur,
        };
        let large = Blur {
            style: Style::default().with_size(crate::style::Size::Large),
            ..small
        };
        let (ImageEffect::Blur { radius: r_small }, ImageEffect::Blur { radius: r_large }) =
            (small.effect(), large.effect())
        else {
            panic!("expected blur effects")
        };
        assert!(r_large > r_small);

        let mut p = RecordingPainter::new();
        small.draw(&mut p);
        assert_eq!(p.effects().count(), 1);
    }

    #[test]
    fn pixelate_mode_emits_a_pixelate_effect() {
        let b = Blur {
            rect: Rect::from_xywh(0.0, 0.0, 50.0, 50.0),
            style: Style::default(),
            obscure: ObscureKind::Pixelate,
        };
        assert!(matches!(b.effect(), ImageEffect::Pixelate { .. }));
    }

    #[test]
    fn an_empty_blur_rect_draws_nothing() {
        let b = Blur {
            rect: Rect::from_xywh(0.0, 0.0, 0.0, 0.0),
            style: Style::default(),
            obscure: ObscureKind::Blur,
        };
        let mut p = RecordingPainter::new();
        b.draw(&mut p);
        assert!(p.is_empty());
    }

    #[test]
    fn ellipse_hit_testing_excludes_the_bounding_box_corners() {
        let e = Ellipse {
            rect: Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            style: Style::default(),
        };
        assert!(e.hit_test(Vec2D::new(50.0, 50.0)), "centre should hit");
        assert!(
            !e.hit_test(Vec2D::new(2.0, 2.0)),
            "the corner is outside the ellipse"
        );
    }

    #[test]
    fn translating_moves_the_shape_without_resizing_it() {
        let mut r = Rectangle {
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 20.0),
            style: Style::default(),
        };
        r.translate(Vec2D::new(5.0, 7.0));
        assert_eq!(r.rect, Rect::from_xywh(5.0, 7.0, 10.0, 20.0));
    }

    #[test]
    fn restyling_mid_drag_does_not_grow_the_shape() {
        // The preview used to be rebuilt from `bounds()`, which includes half
        // the stroke width, so every palette keypress inflated it.
        let mut tool = RectangleTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(200.0, 150.0), Modifiers::NONE);
        tool.handle_mouse_event(m);

        let first = tool.drawable().unwrap().bounds().unwrap();
        for color in [Color::green(), Color::blue(), Color::orange()] {
            tool.handle_style_event(Style::default().with_color(color));
        }
        let after = tool.drawable().unwrap().bounds().unwrap();
        assert_eq!(
            (after.width(), after.height()),
            (first.width(), first.height()),
            "the preview grew while only the colour changed"
        );
    }

    #[test]
    fn changing_style_mid_drag_restyles_the_preview() {
        let mut tool = RectangleTool::new(Style::default());
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(50.0, 50.0), Modifiers::NONE);
        tool.handle_mouse_event(m);

        let restyled = Style::default().with_color(Color::green()).with_fill(true);
        assert!(tool.handle_style_event(restyled).needs_redraw());

        let mut p = RecordingPainter::new();
        tool.drawable().unwrap().draw(&mut p);
        // Now filled, so it paints a mesh rather than a stroke.
        assert_eq!(p.meshes().count(), 1);
    }
}
