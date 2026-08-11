//! Post-shot cropping.
//!
//! Crop is the one tool that does not commit a drawable. It maintains a
//! selection rectangle which the editor reads and applies to the whole scene,
//! rebasing every existing annotation onto the new origin.

use crate::drawable::Drawable;
use crate::input::{Key, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::math::{Rect, Vec2D};
use crate::painter::Painter;
use crate::path::{Path, Stroke};
use crate::style::Color;

use super::{Tool, ToolUpdateResult, Tools};

/// How close (in image pixels) the pointer must be to a handle to grab it.
pub const HANDLE_TOLERANCE: f32 = 12.0;
/// Smallest crop the tool will produce.
pub const MIN_CROP_SIZE: f32 = 8.0;
/// Opacity of the dimming outside the selection.
const DIM_ALPHA: u8 = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// Dragging the interior moves the whole selection.
    Move,
}

impl CropHandle {
    pub const CORNERS: [CropHandle; 4] = [
        CropHandle::TopLeft,
        CropHandle::TopRight,
        CropHandle::BottomRight,
        CropHandle::BottomLeft,
    ];

    pub const EDGES: [CropHandle; 4] = [
        CropHandle::Top,
        CropHandle::Right,
        CropHandle::Bottom,
        CropHandle::Left,
    ];

    /// Position of this handle on `rect` (the midpoint, for edges).
    pub fn position(&self, rect: Rect) -> Vec2D {
        let r = rect.normalized();
        match self {
            CropHandle::TopLeft => r.top_left(),
            CropHandle::Top => Vec2D::new(r.center().x, r.top()),
            CropHandle::TopRight => r.top_right(),
            CropHandle::Right => Vec2D::new(r.right(), r.center().y),
            CropHandle::BottomRight => r.bottom_right(),
            CropHandle::Bottom => Vec2D::new(r.center().x, r.bottom()),
            CropHandle::BottomLeft => r.bottom_left(),
            CropHandle::Left => Vec2D::new(r.left(), r.center().y),
            CropHandle::Move => r.center(),
        }
    }

    fn moves_left(&self) -> bool {
        matches!(
            self,
            CropHandle::TopLeft | CropHandle::Left | CropHandle::BottomLeft
        )
    }
    fn moves_right(&self) -> bool {
        matches!(
            self,
            CropHandle::TopRight | CropHandle::Right | CropHandle::BottomRight
        )
    }
    fn moves_top(&self) -> bool {
        matches!(
            self,
            CropHandle::TopLeft | CropHandle::Top | CropHandle::TopRight
        )
    }
    fn moves_bottom(&self) -> bool {
        matches!(
            self,
            CropHandle::BottomLeft | CropHandle::Bottom | CropHandle::BottomRight
        )
    }
}

/// The dim-and-outline overlay drawn while cropping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropOverlay {
    pub rect: Rect,
    pub canvas: Rect,
}

impl Drawable for CropOverlay {
    fn draw(&self, painter: &mut dyn Painter) {
        painter.dim_outside(self.rect, self.canvas, Color::black().with_alpha(DIM_ALPHA));

        let mut outline = Path::new();
        outline.add_rect(self.rect);
        painter.stroke_path(&outline, Stroke::new(1.0, Color::white()));

        // Handle pips, so the grab points are discoverable.
        let mut pips = Path::new();
        for handle in CropHandle::CORNERS.iter().chain(CropHandle::EDGES.iter()) {
            pips.add_rect(Rect::new(
                handle.position(self.rect) - Vec2D::splat(4.0),
                Vec2D::splat(8.0),
            ));
        }
        painter.fill_path(&pips, Color::white());
    }

    fn bounds(&self) -> Option<Rect> {
        Some(self.canvas)
    }

    fn kind(&self) -> &'static str {
        "crop-overlay"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.rect = self.rect.translated(delta);
    }
}

#[derive(Debug, Clone)]
pub struct CropTool {
    canvas: Rect,
    rect: Rect,
    overlay: CropOverlay,
    /// Handle being dragged, plus the rect as it was when the drag started.
    dragging: Option<(CropHandle, Rect)>,
}

impl Default for CropTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CropTool {
    pub fn new() -> Self {
        let canvas = Rect::default();
        Self {
            canvas,
            rect: canvas,
            overlay: CropOverlay {
                rect: canvas,
                canvas,
            },
            dragging: None,
        }
    }

    /// The current selection.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Whether the selection differs from the full canvas, i.e. whether
    /// applying it would actually change anything.
    pub fn is_cropped(&self) -> bool {
        let r = self.rect.rounded();
        let c = self.canvas.rounded();
        r != c && !r.is_empty()
    }

    pub fn reset(&mut self) {
        self.set_rect(self.canvas);
    }

    fn set_rect(&mut self, rect: Rect) {
        self.rect = rect;
        self.overlay = CropOverlay {
            rect,
            canvas: self.canvas,
        };
    }

    /// Which handle, if any, is under `pos`.
    pub fn handle_at(&self, pos: Vec2D) -> Option<CropHandle> {
        let r = self.rect.normalized();
        // Corners win over edges, edges over the interior, so the smaller
        // target is always reachable.
        for handle in CropHandle::CORNERS {
            if handle.position(r).distance_to(&pos) <= HANDLE_TOLERANCE {
                return Some(handle);
            }
        }
        let near_x = (pos.x - r.left()).abs() <= HANDLE_TOLERANCE
            || (pos.x - r.right()).abs() <= HANDLE_TOLERANCE;
        let near_y = (pos.y - r.top()).abs() <= HANDLE_TOLERANCE
            || (pos.y - r.bottom()).abs() <= HANDLE_TOLERANCE;
        let within_x =
            pos.x >= r.left() - HANDLE_TOLERANCE && pos.x <= r.right() + HANDLE_TOLERANCE;
        let within_y =
            pos.y >= r.top() - HANDLE_TOLERANCE && pos.y <= r.bottom() + HANDLE_TOLERANCE;

        if near_y && within_x {
            return Some(if (pos.y - r.top()).abs() <= HANDLE_TOLERANCE {
                CropHandle::Top
            } else {
                CropHandle::Bottom
            });
        }
        if near_x && within_y {
            return Some(if (pos.x - r.left()).abs() <= HANDLE_TOLERANCE {
                CropHandle::Left
            } else {
                CropHandle::Right
            });
        }
        // Only offer to move a selection that has actually been narrowed.
        // While the selection still covers the whole image there is nowhere to
        // move it to, and an interior drag should start a fresh selection.
        if self.is_cropped() && r.contains(pos) {
            return Some(CropHandle::Move);
        }
        None
    }

    /// Apply a drag of `delta` on `handle`, starting from `origin`.
    fn resized(&self, handle: CropHandle, origin: Rect, delta: Vec2D) -> Rect {
        let canvas = self.canvas.normalized();
        if handle == CropHandle::Move {
            // Slide the whole rect, stopping at the canvas edges rather than
            // shrinking against them.
            let max = Vec2D::new(
                canvas.right() - origin.width(),
                canvas.bottom() - origin.height(),
            );
            let pos = (origin.pos + delta).clamp(canvas.top_left(), max);
            return Rect::new(pos, origin.size);
        }

        let mut left = origin.left();
        let mut top = origin.top();
        let mut right = origin.right();
        let mut bottom = origin.bottom();

        // On an image narrower or shorter than the minimum crop there is no
        // room for that minimum, and `f32::clamp` panics when handed min > max.
        // Shrink the floor to fit rather than refusing to work on small images:
        // a 4px favicon is a legitimate thing to open.
        let min_w = MIN_CROP_SIZE.min(canvas.width());
        let min_h = MIN_CROP_SIZE.min(canvas.height());

        if handle.moves_left() {
            left = (left + delta.x).clamp(canvas.left(), (right - min_w).max(canvas.left()));
        }
        if handle.moves_right() {
            right = (right + delta.x).clamp((left + min_w).min(canvas.right()), canvas.right());
        }
        if handle.moves_top() {
            top = (top + delta.y).clamp(canvas.top(), (bottom - min_h).max(canvas.top()));
        }
        if handle.moves_bottom() {
            bottom = (bottom + delta.y).clamp((top + min_h).min(canvas.bottom()), canvas.bottom());
        }

        Rect::from_xywh(left, top, right - left, bottom - top)
    }
}

impl Tool for CropTool {
    fn kind(&self) -> Tools {
        Tools::Crop
    }

    fn set_canvas_bounds(&mut self, bounds: Rect) {
        let changed = self.canvas != bounds;
        self.canvas = bounds;
        if changed {
            // A new image means the old selection is meaningless.
            self.set_rect(bounds);
        }
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }

        match event.kind {
            MouseEventKind::BeginDrag => {
                let handle = self.handle_at(event.start());
                match handle {
                    Some(handle) => {
                        self.dragging = Some((handle, self.rect.normalized()));
                        let rect = self.resized(handle, self.rect.normalized(), event.delta);
                        self.set_rect(rect);
                    }
                    None => {
                        // Starting outside the selection draws a fresh one.
                        self.dragging = Some((
                            CropHandle::BottomRight,
                            Rect::new(event.start(), Vec2D::ZERO),
                        ));
                        let rect =
                            Rect::from_corners(event.start(), event.pos).clamped_to(self.canvas);
                        self.set_rect(rect);
                    }
                }
                ToolUpdateResult::Redraw
            }
            MouseEventKind::UpdateDrag | MouseEventKind::EndDrag => {
                let Some((handle, origin)) = self.dragging else {
                    return ToolUpdateResult::Unmodified;
                };
                let rect = if origin.size.is_zero() {
                    Rect::from_corners(origin.pos, event.pos).clamped_to(self.canvas)
                } else {
                    self.resized(handle, origin, event.delta)
                };
                self.set_rect(rect);
                if event.kind == MouseEventKind::EndDrag {
                    self.dragging = None;
                    // A stray click-drag that collapsed the selection would
                    // otherwise leave nothing croppable.
                    if self.rect.width() < MIN_CROP_SIZE || self.rect.height() < MIN_CROP_SIZE {
                        self.reset();
                    }
                }
                ToolUpdateResult::Redraw
            }
            _ => ToolUpdateResult::Unmodified,
        }
    }

    fn handle_key_event(&mut self, event: KeyEvent) -> ToolUpdateResult {
        if event.key == Key::Escape && self.is_cropped() {
            self.reset();
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn crop_selection(&self) -> Option<Rect> {
        self.is_cropped().then(|| self.rect.normalized().rounded())
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        Some(&self.overlay)
    }

    fn is_active(&self) -> bool {
        // Crop always shows its overlay, but it only counts as "has pending
        // work" when the selection is actually narrowed.
        self.is_cropped() || self.dragging.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::RecordingPainter;

    fn tool_with_canvas(w: f32, h: f32) -> CropTool {
        let mut tool = CropTool::new();
        tool.set_canvas_bounds(Rect::from_xywh(0.0, 0.0, w, h));
        tool
    }

    fn drag(tool: &mut CropTool, from: Vec2D, to: Vec2D) {
        let mut tracker = PointerTracker::new();
        tracker.press(from, MouseButton::Left, Modifiers::NONE);
        let begin = tracker.motion(to, Modifiers::NONE);
        tool.handle_mouse_event(begin);
        let end = tracker.release(to, Modifiers::NONE).unwrap();
        tool.handle_mouse_event(end);
    }

    #[test]
    fn a_fresh_tool_selects_the_whole_canvas() {
        let tool = tool_with_canvas(800.0, 600.0);
        assert_eq!(tool.rect(), Rect::from_xywh(0.0, 0.0, 800.0, 600.0));
        assert!(!tool.is_cropped(), "nothing to apply yet");
    }

    #[test]
    fn corner_handles_take_priority_over_edges() {
        let tool = tool_with_canvas(800.0, 600.0);
        assert_eq!(tool.handle_at(Vec2D::ZERO), Some(CropHandle::TopLeft));
        assert_eq!(
            tool.handle_at(Vec2D::new(800.0, 600.0)),
            Some(CropHandle::BottomRight)
        );
        assert_eq!(
            tool.handle_at(Vec2D::new(400.0, 0.0)),
            Some(CropHandle::Top)
        );
        // The interior is not a move target until something is cropped:
        // dragging there should start a new selection instead.
        assert_eq!(tool.handle_at(Vec2D::new(400.0, 300.0)), None);
    }

    #[test]
    fn the_interior_becomes_a_move_target_once_something_is_cropped() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(
            &mut tool,
            Vec2D::new(200.0, 200.0),
            Vec2D::new(400.0, 350.0),
        );
        assert_eq!(
            tool.handle_at(Vec2D::new(300.0, 275.0)),
            Some(CropHandle::Move)
        );
    }

    #[test]
    fn dragging_a_corner_resizes_the_selection() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(100.0, 50.0));
        assert_eq!(tool.rect(), Rect::from_xywh(100.0, 50.0, 700.0, 550.0));
        assert!(tool.is_cropped());
    }

    #[test]
    fn dragging_an_edge_moves_only_that_edge() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(
            &mut tool,
            Vec2D::new(400.0, 600.0),
            Vec2D::new(400.0, 500.0),
        );
        assert_eq!(tool.rect(), Rect::from_xywh(0.0, 0.0, 800.0, 500.0));
    }

    #[test]
    fn the_selection_can_never_leave_the_canvas() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        // Drag the top-left corner far outside the image.
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(-500.0, -500.0));
        let r = tool.rect();
        assert!(r.left() >= 0.0 && r.top() >= 0.0, "{r:?}");
        assert!(r.right() <= 800.0 && r.bottom() <= 600.0, "{r:?}");
    }

    #[test]
    fn a_handle_cannot_be_dragged_past_the_opposite_edge() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        // Push the left edge far past the right edge.
        drag(&mut tool, Vec2D::new(0.0, 300.0), Vec2D::new(5000.0, 300.0));
        let r = tool.rect();
        assert!(r.width() >= MIN_CROP_SIZE, "collapsed to {r:?}");
        assert!(r.left() < r.right());
    }

    #[test]
    fn moving_the_interior_slides_without_resizing() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        // Draw a small selection well inside the canvas so there is room to
        // slide it without hitting the clamp.
        drag(
            &mut tool,
            Vec2D::new(200.0, 200.0),
            Vec2D::new(400.0, 350.0),
        );
        let before = tool.rect();
        assert_eq!(before, Rect::from_xywh(200.0, 200.0, 200.0, 150.0));

        drag(
            &mut tool,
            before.center(),
            before.center() + Vec2D::new(50.0, 20.0),
        );
        let after = tool.rect();
        assert_eq!(after.size, before.size, "move must not resize");
        assert_eq!(after.pos, before.pos + Vec2D::new(50.0, 20.0));
    }

    #[test]
    fn moving_stops_at_the_canvas_edge_instead_of_shrinking() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(200.0, 200.0));
        let size_before = tool.rect().size;

        let centre = tool.rect().center();
        drag(&mut tool, centre, centre + Vec2D::new(10_000.0, 10_000.0));
        let after = tool.rect();
        assert_eq!(after.size, size_before, "size must survive the clamp");
        assert!((after.right() - 800.0).abs() < 0.01, "{after:?}");
        assert!((after.bottom() - 600.0).abs() < 0.01, "{after:?}");
    }

    #[test]
    fn dragging_from_outside_the_selection_draws_a_new_one() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        // Narrow the selection first so there is an "outside" to start from.
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(300.0, 300.0));
        assert_eq!(tool.rect(), Rect::from_xywh(300.0, 300.0, 500.0, 300.0));

        drag(&mut tool, Vec2D::new(50.0, 50.0), Vec2D::new(150.0, 120.0));
        assert_eq!(tool.rect(), Rect::from_xywh(50.0, 50.0, 100.0, 70.0));
    }

    #[test]
    fn a_collapsed_selection_resets_to_the_full_canvas() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(300.0, 300.0));
        // Now make a selection too small to be useful. It has to exceed the
        // pointer drag threshold to register as a drag at all, but stay under
        // MIN_CROP_SIZE.
        drag(&mut tool, Vec2D::new(10.0, 10.0), Vec2D::new(15.0, 13.0));
        assert_eq!(tool.rect(), Rect::from_xywh(0.0, 0.0, 800.0, 600.0));
        assert!(!tool.is_cropped());
    }

    #[test]
    fn a_tiny_image_can_be_cropped_without_panicking() {
        // `f32::clamp` panics when min > max, which it was whenever the canvas
        // was smaller than MIN_CROP_SIZE. Reachable from a 4-7px region
        // selection, or `--filename` on a favicon.
        for size in [1.0f32, 2.0, 4.0, 7.0, 8.0, 9.0] {
            let mut tool = tool_with_canvas(size, size);
            // Every handle, dragged well past every edge.
            for from in [
                Vec2D::ZERO,
                Vec2D::new(size, size),
                Vec2D::new(size / 2.0, 0.0),
                Vec2D::new(0.0, size / 2.0),
            ] {
                for to in [
                    Vec2D::new(-100.0, -100.0),
                    Vec2D::new(100.0, 100.0),
                    Vec2D::new(size, 0.0),
                ] {
                    drag(&mut tool, from, to);
                    let r = tool.rect();
                    assert!(r.width() >= 0.0 && r.height() >= 0.0, "{size}px: {r:?}");
                    assert!(r.left() >= -0.01 && r.top() >= -0.01, "{size}px: {r:?}");
                    assert!(
                        r.right() <= size + 0.01 && r.bottom() <= size + 0.01,
                        "{size}px: {r:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn escape_resets_the_selection() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(100.0, 100.0));
        assert!(tool.is_cropped());
        assert!(
            tool.handle_key_event(KeyEvent::plain(Key::Escape))
                .needs_redraw()
        );
        assert!(!tool.is_cropped());
    }

    #[test]
    fn a_new_canvas_size_resets_the_selection() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        drag(&mut tool, Vec2D::ZERO, Vec2D::new(100.0, 100.0));
        assert!(tool.is_cropped());
        tool.set_canvas_bounds(Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(tool.rect(), Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0));
        assert!(!tool.is_cropped());
    }

    #[test]
    fn the_overlay_dims_outside_and_outlines_the_selection() {
        let overlay = CropOverlay {
            rect: Rect::from_xywh(100.0, 100.0, 200.0, 200.0),
            canvas: Rect::from_xywh(0.0, 0.0, 800.0, 600.0),
        };
        let mut p = RecordingPainter::new();
        overlay.draw(&mut p);
        // A selection inset on all four sides needs four dim bands, plus the
        // handle pips; the outline is the single stroke.
        assert_eq!(p.meshes().count(), 5);
        assert_eq!(p.strokes().count(), 1);
    }

    #[test]
    fn an_edge_flush_selection_skips_the_empty_dim_bands() {
        let overlay = CropOverlay {
            // Flush against the right and bottom edges.
            rect: Rect::from_xywh(100.0, 100.0, 700.0, 500.0),
            canvas: Rect::from_xywh(0.0, 0.0, 800.0, 600.0),
        };
        let mut p = RecordingPainter::new();
        overlay.draw(&mut p);
        assert_eq!(p.meshes().count(), 3, "two bands plus the pips");
    }

    #[test]
    fn crop_never_commits_a_drawable() {
        let mut tool = tool_with_canvas(800.0, 600.0);
        let mut tracker = PointerTracker::new();
        tracker.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        let m = tracker.motion(Vec2D::new(100.0, 100.0), Modifiers::NONE);
        assert!(!tool.handle_mouse_event(m).is_commit());
        let e = tracker
            .release(Vec2D::new(100.0, 100.0), Modifiers::NONE)
            .unwrap();
        assert!(!tool.handle_mouse_event(e).is_commit());
    }
}
