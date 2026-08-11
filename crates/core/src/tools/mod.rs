//! Annotation tools.
//!
//! Adapted in shape from Satty (`src/tools/`), MPL-2.0, Copyright the Satty
//! authors, with GTK and femtovg types replaced by this crate's toolkit-neutral
//! ones.
//!
//! A [`Tool`] is a small state machine driven by [`ToolEvent`]s. It owns an
//! in-progress shape, exposes it as a preview [`Drawable`], and on completion
//! returns [`ToolUpdateResult::Commit`] handing ownership to the scene. Tools
//! never draw directly and never touch the OS.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::drawable::Drawable;
use crate::input::{InputEvent, KeyEvent, MouseEvent, TextEvent};
use crate::style::Style;

mod brush;
mod crop;
mod line;
mod marker;
mod pointer;
mod shapes;
mod text;

pub use brush::{Brush, BrushTool};
pub use crop::{CropHandle, CropOverlay, CropTool, MIN_CROP_SIZE};
pub use line::{Arrow, ArrowTool, Line, LineDragTool, LineShape, LineTool};
pub use marker::{Marker, MarkerTool};
pub use pointer::PointerTool;
pub use shapes::{
    Blur, BlurTool, Ellipse, EllipseTool, HIGHLIGHT_ALPHA, Highlight, HighlightTool, ObscureKind,
    RectDragTool, RectShape, Rectangle, RectangleTool,
};
pub use text::{Text, TextTool};

/// What a tool wants the editor to do after handling an event.
pub enum ToolUpdateResult {
    /// Nothing changed; skip the repaint.
    Unmodified,
    /// Visual state changed but nothing was finished.
    Redraw,
    /// The annotation is finished: push it onto the scene's undo stack.
    Commit(Box<dyn Drawable>),
}

impl ToolUpdateResult {
    pub fn needs_redraw(&self) -> bool {
        !matches!(self, ToolUpdateResult::Unmodified)
    }

    pub fn is_commit(&self) -> bool {
        matches!(self, ToolUpdateResult::Commit(_))
    }
}

impl std::fmt::Debug for ToolUpdateResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolUpdateResult::Unmodified => f.write_str("Unmodified"),
            ToolUpdateResult::Redraw => f.write_str("Redraw"),
            ToolUpdateResult::Commit(d) => write!(f, "Commit({})", d.kind()),
        }
    }
}

pub enum ToolEvent {
    Activated,
    Deactivated,
    /// Escape, or a click that lands outside the tool's interaction area.
    Dismissed,
    Input(InputEvent),
    StyleChanged(Style),
}

pub trait Tool {
    fn kind(&self) -> Tools;

    /// Dispatch an event. The default implementation routes to the specific
    /// handlers; tools override the handlers, not this.
    fn handle_event(&mut self, event: ToolEvent) -> ToolUpdateResult {
        match event {
            ToolEvent::Activated => self.handle_activated(),
            ToolEvent::Deactivated => self.handle_deactivated(),
            ToolEvent::Dismissed => self.handle_dismissed(),
            ToolEvent::StyleChanged(style) => self.handle_style_event(style),
            ToolEvent::Input(InputEvent::Mouse(e)) => self.handle_mouse_event(e),
            ToolEvent::Input(InputEvent::Key(e)) => self.handle_key_event(e),
            ToolEvent::Input(InputEvent::Text(e)) => self.handle_text_event(&e),
        }
    }

    fn handle_activated(&mut self) -> ToolUpdateResult {
        ToolUpdateResult::Unmodified
    }

    /// Deactivating must not silently discard work: tools with an in-progress
    /// shape should commit it here (text does) or drop it (shapes do).
    fn handle_deactivated(&mut self) -> ToolUpdateResult {
        ToolUpdateResult::Unmodified
    }

    fn handle_dismissed(&mut self) -> ToolUpdateResult {
        ToolUpdateResult::Unmodified
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        let _ = event;
        ToolUpdateResult::Unmodified
    }

    fn handle_key_event(&mut self, event: KeyEvent) -> ToolUpdateResult {
        let _ = event;
        ToolUpdateResult::Unmodified
    }

    fn handle_text_event(&mut self, event: &TextEvent) -> ToolUpdateResult {
        let _ = event;
        ToolUpdateResult::Unmodified
    }

    fn handle_style_event(&mut self, style: Style) -> ToolUpdateResult {
        let _ = style;
        ToolUpdateResult::Unmodified
    }

    /// The in-progress annotation, drawn on top of the committed ones.
    fn drawable(&self) -> Option<&dyn Drawable> {
        None
    }

    /// True while the tool holds unfinished state (so the editor knows Escape
    /// should cancel the shape rather than exit the app).
    fn is_active(&self) -> bool {
        self.drawable().is_some()
    }

    /// True when the tool wants raw text input routed to it, which also tells
    /// the shell to enable the input method.
    fn wants_text_input(&self) -> bool {
        false
    }

    /// Tell the tool how large the image is. The editor calls this when a tool
    /// is activated and whenever the image changes (a crop, a new capture).
    /// Only crop currently cares.
    fn set_canvas_bounds(&mut self, bounds: crate::math::Rect) {
        let _ = bounds;
    }

    /// The selection to crop to, for the one tool whose output is applied to
    /// the whole document rather than committed as an annotation.
    ///
    /// `None` means "nothing to apply". This is a trait method rather than a
    /// downcast because the editor should never need to know the concrete tool
    /// types it is holding.
    fn crop_selection(&self) -> Option<crate::math::Rect> {
        None
    }
}

/// Every tool, in toolbar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tools {
    #[default]
    Pointer,
    Crop,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Marker,
    Brush,
    Highlight,
    Blur,
}

impl Tools {
    pub const ALL: [Tools; 11] = [
        Tools::Pointer,
        Tools::Crop,
        Tools::Line,
        Tools::Arrow,
        Tools::Rectangle,
        Tools::Ellipse,
        Tools::Text,
        Tools::Marker,
        Tools::Brush,
        Tools::Highlight,
        Tools::Blur,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Tools::Pointer => "pointer",
            Tools::Crop => "crop",
            Tools::Line => "line",
            Tools::Arrow => "arrow",
            Tools::Rectangle => "rectangle",
            Tools::Ellipse => "ellipse",
            Tools::Text => "text",
            Tools::Marker => "marker",
            Tools::Brush => "brush",
            Tools::Highlight => "highlight",
            Tools::Blur => "blur",
        }
    }

    /// Whether the style controls (color, size, fill) apply to this tool.
    pub fn uses_style(&self) -> bool {
        !matches!(self, Tools::Pointer | Tools::Crop)
    }

    /// Build the tool. A fresh instance is created on every switch, which is
    /// what makes "switching tools cancels the in-progress shape" fall out for
    /// free.
    pub fn create(&self, style: Style) -> Box<dyn Tool> {
        match self {
            Tools::Pointer => Box::new(PointerTool::new(style)),
            Tools::Crop => Box::new(CropTool::new()),
            Tools::Line => Box::new(LineTool::new(style)),
            Tools::Arrow => Box::new(ArrowTool::new(style)),
            Tools::Rectangle => Box::new(RectangleTool::new(style)),
            Tools::Ellipse => Box::new(EllipseTool::new(style)),
            Tools::Text => Box::new(TextTool::new(style)),
            Tools::Marker => Box::new(MarkerTool::new(style)),
            Tools::Brush => Box::new(BrushTool::new(style)),
            Tools::Highlight => Box::new(HighlightTool::new(style)),
            Tools::Blur => Box::new(BlurTool::new(style)),
        }
    }
}

impl Display for Tools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown tool `{0}`")]
pub struct ToolParseError(String);

impl FromStr for Tools {
    type Err = ToolParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        Tools::ALL
            .into_iter()
            .find(|t| t.name() == needle)
            .ok_or_else(|| ToolParseError(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_roundtrip() {
        for tool in Tools::ALL {
            assert_eq!(tool.name().parse::<Tools>().unwrap(), tool);
            assert_eq!(tool.to_string().parse::<Tools>().unwrap(), tool);
        }
    }

    #[test]
    fn tool_parsing_is_case_insensitive_and_rejects_junk() {
        assert_eq!("ARROW".parse::<Tools>().unwrap(), Tools::Arrow);
        assert_eq!("  brush ".parse::<Tools>().unwrap(), Tools::Brush);
        assert!("wobble".parse::<Tools>().is_err());
    }

    #[test]
    fn every_tool_can_be_created_and_starts_idle() {
        for tool in Tools::ALL {
            let created = tool.create(Style::default());
            assert_eq!(created.kind(), tool, "{tool} reports the wrong kind");
            assert!(
                !created.is_active(),
                "{tool} should not start with pending state"
            );
            // Crop is the exception: its overlay is always visible, it just
            // starts out selecting the whole canvas.
            if tool != Tools::Crop {
                assert!(created.drawable().is_none(), "{tool} previews too early");
            }
        }
    }

    #[test]
    fn only_text_wants_text_input() {
        for tool in Tools::ALL {
            let created = tool.create(Style::default());
            assert_eq!(
                created.wants_text_input(),
                tool == Tools::Text,
                "{tool} disagrees about text input"
            );
        }
    }

    #[test]
    fn style_free_tools_are_pointer_and_crop() {
        assert!(!Tools::Pointer.uses_style());
        assert!(!Tools::Crop.uses_style());
        assert!(Tools::Arrow.uses_style());
    }

    /// Escape (or a tool switch) must mean the shape is gone, including when
    /// the mouse button is released afterwards — which is the normal order of
    /// events, since a user holds the button while pressing Escape.
    ///
    /// Seven tools used to rebuild the shape from the release event and commit
    /// it anyway. The tests that were supposed to cover this stopped at
    /// `is_active()` and never delivered the release.
    #[test]
    fn no_tool_commits_a_shape_that_was_cancelled_mid_drag() {
        use crate::input::{Modifiers, MouseButton, PointerTracker};
        use crate::math::Vec2D;

        for kind in Tools::ALL {
            let mut tool = kind.create(Style::default());
            tool.set_canvas_bounds(crate::math::Rect::from_xywh(0.0, 0.0, 500.0, 500.0));

            let mut tracker = PointerTracker::new();
            tracker.press(Vec2D::new(50.0, 50.0), MouseButton::Left, Modifiers::NONE);
            let drag = tracker.motion(Vec2D::new(250.0, 200.0), Modifiers::NONE);
            tool.handle_event(ToolEvent::Input(InputEvent::Mouse(drag)));

            // The user presses Escape while still holding the button.
            tool.handle_event(ToolEvent::Dismissed);

            // ...then lets go.
            let release = tracker
                .release(Vec2D::new(250.0, 200.0), Modifiers::NONE)
                .expect("a press was delivered");
            let result = tool.handle_event(ToolEvent::Input(InputEvent::Mouse(release)));

            assert!(
                !result.is_commit(),
                "{kind} committed a shape after it was cancelled"
            );
        }
    }

    /// A release with no matching press must never produce an annotation.
    /// This can happen when the pointer is pressed outside the canvas, or
    /// after focus is lost and regained.
    #[test]
    fn no_tool_commits_from_a_release_it_never_saw_the_press_for() {
        use crate::input::{Modifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::math::Vec2D;

        for kind in Tools::ALL {
            let mut tool = kind.create(Style::default());
            tool.set_canvas_bounds(crate::math::Rect::from_xywh(0.0, 0.0, 500.0, 500.0));

            let stray = MouseEvent {
                kind: MouseEventKind::EndDrag,
                pos: Vec2D::new(300.0, 300.0),
                delta: Vec2D::new(200.0, 200.0),
                button: MouseButton::Left,
                modifiers: Modifiers::NONE,
            };
            let result = tool.handle_event(ToolEvent::Input(InputEvent::Mouse(stray)));
            assert!(
                !result.is_commit(),
                "{kind} invented a shape from a stray release"
            );
        }
    }

    #[test]
    fn update_result_classification() {
        assert!(!ToolUpdateResult::Unmodified.needs_redraw());
        assert!(ToolUpdateResult::Redraw.needs_redraw());
        assert!(!ToolUpdateResult::Redraw.is_commit());
    }
}
