//! The no-op tool.
//!
//! Selecting Pointer means "don't draw": clicks and drags fall through to the
//! shell for panning, and (from Phase 3) for selecting existing annotations.
//! It exists so there is always a safe tool to be in.

use crate::style::Style;

use super::{Tool, Tools};

#[derive(Debug, Default, Clone, Copy)]
pub struct PointerTool;

impl PointerTool {
    pub fn new(_style: Style) -> Self {
        Self
    }
}

impl Tool for PointerTool {
    fn kind(&self) -> Tools {
        Tools::Pointer
    }

    fn is_active(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind};
    use crate::math::Vec2D;

    #[test]
    fn the_pointer_tool_never_draws_or_commits() {
        let mut tool = PointerTool::new(Style::default());
        let event = MouseEvent {
            kind: MouseEventKind::EndDrag,
            pos: Vec2D::new(100.0, 100.0),
            delta: Vec2D::new(100.0, 100.0),
            button: MouseButton::Left,
            modifiers: Modifiers::NONE,
        };
        assert!(!tool.handle_mouse_event(event).needs_redraw());
        assert!(
            !tool
                .handle_key_event(KeyEvent::plain(Key::Escape))
                .needs_redraw()
        );
        assert!(tool.drawable().is_none());
        assert!(!tool.is_active());
    }
}
