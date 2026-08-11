//! Text annotation with an in-place editing caret.
//!
//! The caret is a **byte** offset into a UTF-8 string, so every edit and every
//! movement has to respect char boundaries — getting this wrong panics on the
//! first non-ASCII character, which is exactly the input an IME produces.
//! Those operations live here, away from the event loop, and are tested
//! against multi-byte text directly.

use crate::drawable::Drawable;
use crate::input::{Key, KeyEvent, MouseButton, MouseEvent, MouseEventKind, TextEvent};
use crate::math::{Rect, Vec2D};
use crate::painter::{Painter, TextDraw};
use crate::style::Style;

use super::{Tool, ToolUpdateResult, Tools};

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub pos: Vec2D,
    pub text: String,
    pub style: Style,
    /// Caret byte offset while this text is being edited.
    ///
    /// Private because it carries an invariant the type system cannot express:
    /// it must sit on a UTF-8 character boundary. A value in the middle of a
    /// multi-byte character panics on the next edit. Use [`Text::set_cursor`],
    /// which snaps to the nearest boundary.
    cursor: Option<usize>,
    /// In-flight IME composition, shown after the caret but not yet part of
    /// `text`.
    pub preedit: String,
}

impl Text {
    pub fn new(pos: Vec2D, style: Style) -> Self {
        Self {
            pos,
            text: String::new(),
            style,
            cursor: Some(0),
            preedit: String::new(),
        }
    }

    /// Build a finished text annotation: content, no caret, no composition.
    /// This is what a committed one looks like.
    pub fn committed(pos: Vec2D, text: impl Into<String>, style: Style) -> Self {
        Self {
            pos,
            text: text.into(),
            style,
            cursor: None,
            preedit: String::new(),
        }
    }

    /// Build one that is mid-edit, with the caret snapped to a character
    /// boundary.
    pub fn editing(pos: Vec2D, text: impl Into<String>, style: Style, cursor: usize) -> Self {
        let mut this = Self::committed(pos, text, style);
        this.set_cursor(Some(cursor));
        this
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Stop editing: drop the caret and any uncommitted composition.
    pub fn finish(&mut self) {
        self.cursor = None;
        self.preedit.clear();
    }

    /// The caret position, if this text is being edited.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Move the caret, snapping to the nearest character boundary at or before
    /// the requested offset.
    pub fn set_cursor(&mut self, cursor: Option<usize>) {
        self.cursor = cursor.map(|at| {
            let mut at = at.min(self.text.len());
            while at > 0 && !self.text.is_char_boundary(at) {
                at -= 1;
            }
            at
        });
    }

    fn cursor_or_end(&self) -> usize {
        let at = self.cursor.unwrap_or(self.text.len()).min(self.text.len());
        // Defensive: an offset that is not on a boundary would panic in
        // `insert_str`/`replace_range` below.
        if self.text.is_char_boundary(at) {
            at
        } else {
            let mut fixed = at;
            while fixed > 0 && !self.text.is_char_boundary(fixed) {
                fixed -= 1;
            }
            fixed
        }
    }

    pub fn insert(&mut self, s: &str) {
        let at = self.cursor_or_end();
        self.text.insert_str(at, s);
        self.cursor = Some(at + s.len());
    }

    /// Delete the character before the caret. Returns whether anything changed.
    pub fn backspace(&mut self) -> bool {
        let at = self.cursor_or_end();
        let Some(prev) = prev_boundary(&self.text, at) else {
            return false;
        };
        self.text.replace_range(prev..at, "");
        self.cursor = Some(prev);
        true
    }

    /// Delete the character after the caret.
    pub fn delete(&mut self) -> bool {
        let at = self.cursor_or_end();
        let Some(next) = next_boundary(&self.text, at) else {
            return false;
        };
        self.text.replace_range(at..next, "");
        self.cursor = Some(at);
        true
    }

    pub fn move_left(&mut self) -> bool {
        let at = self.cursor_or_end();
        match prev_boundary(&self.text, at) {
            Some(prev) => {
                self.cursor = Some(prev);
                true
            }
            None => false,
        }
    }

    pub fn move_right(&mut self) -> bool {
        let at = self.cursor_or_end();
        match next_boundary(&self.text, at) {
            Some(next) => {
                self.cursor = Some(next);
                true
            }
            None => false,
        }
    }

    /// Move to the start of the current line.
    pub fn move_home(&mut self) -> bool {
        let at = self.cursor_or_end();
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let moved = start != at;
        self.cursor = Some(start);
        moved
    }

    /// Move to the end of the current line.
    pub fn move_end(&mut self) -> bool {
        let at = self.cursor_or_end();
        let end = self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i);
        let moved = end != at;
        self.cursor = Some(end);
        moved
    }

    /// The text as displayed, including any in-flight composition.
    pub fn displayed(&self) -> String {
        if self.preedit.is_empty() {
            self.text.clone()
        } else {
            let at = self.cursor_or_end();
            let mut s = String::with_capacity(self.text.len() + self.preedit.len());
            s.push_str(&self.text[..at]);
            s.push_str(&self.preedit);
            s.push_str(&self.text[at..]);
            s
        }
    }
}

fn prev_boundary(s: &str, at: usize) -> Option<usize> {
    if at == 0 {
        return None;
    }
    let at = at.min(s.len());
    Some(at - s[..at].chars().next_back()?.len_utf8())
}

fn next_boundary(s: &str, at: usize) -> Option<usize> {
    if at >= s.len() {
        return None;
    }
    Some(at + s[at..].chars().next()?.len_utf8())
}

impl Drawable for Text {
    fn draw(&self, painter: &mut dyn Painter) {
        let displayed = self.displayed();
        if displayed.is_empty() && self.cursor.is_none() {
            return;
        }
        painter.draw_text(
            &TextDraw::new(
                self.pos,
                &displayed,
                self.style.text_size(),
                self.style.color,
            )
            .with_cursor(self.cursor),
        );
    }

    fn bounds(&self) -> Option<Rect> {
        if self.text.is_empty() && self.preedit.is_empty() {
            return None;
        }
        let size = crate::painter::estimate_text_size(&self.displayed(), self.style.text_size());
        Some(Rect::new(self.pos, size))
    }

    fn kind(&self) -> &'static str {
        "text"
    }

    fn translate(&mut self, delta: Vec2D) {
        self.pos += delta;
    }
}

#[derive(Debug, Clone)]
pub struct TextTool {
    editing: Option<Text>,
    style: Style,
}

impl TextTool {
    pub fn new(style: Style) -> Self {
        Self {
            editing: None,
            style,
        }
    }

    /// Finish the current text, returning a commit if it has content.
    fn finish(&mut self) -> ToolUpdateResult {
        match self.editing.take() {
            Some(mut text) if !text.is_empty() => {
                text.finish();
                ToolUpdateResult::Commit(Box::new(text))
            }
            // An empty text box leaves nothing behind.
            Some(_) => ToolUpdateResult::Redraw,
            None => ToolUpdateResult::Unmodified,
        }
    }
}

impl Tool for TextTool {
    fn kind(&self) -> Tools {
        Tools::Text
    }

    fn wants_text_input(&self) -> bool {
        true
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> ToolUpdateResult {
        if event.button == MouseButton::Middle {
            return ToolUpdateResult::Unmodified;
        }
        match event.kind {
            MouseEventKind::Click | MouseEventKind::EndDrag => {
                // Committing the previous box and starting a new one is a
                // single gesture, so commit first and let the editor pick up
                // the new box on the next frame.
                let committed = self.finish();
                self.editing = Some(Text::new(event.pos, self.style));
                match committed {
                    ToolUpdateResult::Commit(d) => ToolUpdateResult::Commit(d),
                    _ => ToolUpdateResult::Redraw,
                }
            }
            _ => ToolUpdateResult::Unmodified,
        }
    }

    fn handle_key_event(&mut self, event: KeyEvent) -> ToolUpdateResult {
        let Some(text) = &mut self.editing else {
            return ToolUpdateResult::Unmodified;
        };

        // Let the shell keep its accelerators (Ctrl+S, Ctrl+Z, ...).
        if event.modifiers.command() {
            return ToolUpdateResult::Unmodified;
        }

        let changed = match event.key {
            Key::Escape => return self.finish(),
            // Enter inserts a newline here, masking the global Enter action,
            // as it does in Satty.
            Key::Enter => {
                text.insert("\n");
                true
            }
            Key::Backspace => text.backspace(),
            Key::Delete => text.delete(),
            Key::Left => text.move_left(),
            Key::Right => text.move_right(),
            Key::Home => text.move_home(),
            Key::End => text.move_end(),
            _ => match event.key.as_char() {
                Some(c) => {
                    text.insert(&c.to_string());
                    true
                }
                None => false,
            },
        };

        if changed {
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    fn handle_text_event(&mut self, event: &TextEvent) -> ToolUpdateResult {
        let Some(text) = &mut self.editing else {
            return ToolUpdateResult::Unmodified;
        };
        match event {
            TextEvent::Commit(s) => {
                text.preedit.clear();
                text.insert(s);
                ToolUpdateResult::Redraw
            }
            TextEvent::Preedit(s) => {
                if text.preedit == *s {
                    ToolUpdateResult::Unmodified
                } else {
                    text.preedit = s.clone();
                    ToolUpdateResult::Redraw
                }
            }
        }
    }

    fn handle_style_event(&mut self, style: Style) -> ToolUpdateResult {
        self.style = style;
        if let Some(text) = &mut self.editing {
            text.style = style;
            ToolUpdateResult::Redraw
        } else {
            ToolUpdateResult::Unmodified
        }
    }

    /// Switching away from the text tool keeps what was typed rather than
    /// throwing it away.
    fn handle_deactivated(&mut self) -> ToolUpdateResult {
        self.finish()
    }

    fn handle_dismissed(&mut self) -> ToolUpdateResult {
        self.finish()
    }

    fn drawable(&self) -> Option<&dyn Drawable> {
        self.editing.as_ref().map(|t| t as &dyn Drawable)
    }

    fn is_active(&self) -> bool {
        self.editing.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Modifiers, PointerTracker};
    use crate::painter::{PaintOp, RecordingPainter};

    fn click_at(tool: &mut TextTool, at: Vec2D) -> ToolUpdateResult {
        let mut tracker = PointerTracker::new();
        tracker.press(at, MouseButton::Left, Modifiers::NONE);
        let e = tracker.release(at, Modifiers::NONE).unwrap();
        tool.handle_mouse_event(e)
    }

    fn type_str(tool: &mut TextTool, s: &str) {
        tool.handle_text_event(&TextEvent::Commit(s.to_owned()));
    }

    #[test]
    fn clicking_starts_an_editable_box_and_typing_fills_it() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::new(20.0, 30.0));
        assert!(tool.is_active());
        type_str(&mut tool, "hello");

        let mut p = RecordingPainter::new();
        tool.drawable().unwrap().draw(&mut p);
        let Some(PaintOp::Text { text, pos, .. }) = p.texts().next() else {
            panic!("expected text to be drawn")
        };
        assert_eq!(text, "hello");
        assert_eq!(*pos, Vec2D::new(20.0, 30.0));
    }

    #[test]
    fn escape_commits_the_text() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "done");
        let result = tool.handle_key_event(KeyEvent::plain(Key::Escape));
        assert!(result.is_commit(), "got {result:?}");
        assert!(!tool.is_active());
    }

    #[test]
    fn an_empty_box_commits_nothing() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        let result = tool.handle_key_event(KeyEvent::plain(Key::Escape));
        assert!(!result.is_commit());
        assert!(!tool.is_active());
    }

    #[test]
    fn switching_tools_keeps_what_was_typed() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "keep me");
        assert!(
            tool.handle_deactivated().is_commit(),
            "deactivating must not discard text"
        );
    }

    #[test]
    fn clicking_elsewhere_commits_the_old_box_and_opens_a_new_one() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "first");
        let result = click_at(&mut tool, Vec2D::new(100.0, 100.0));
        assert!(result.is_commit());
        assert!(tool.is_active(), "a new box should be open");
        assert!(tool.drawable().unwrap().bounds().is_none(), "and be empty");
    }

    #[test]
    fn enter_inserts_a_newline_instead_of_committing() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "a");
        let result = tool.handle_key_event(KeyEvent::plain(Key::Enter));
        assert!(!result.is_commit());
        type_str(&mut tool, "b");

        let mut p = RecordingPainter::new();
        tool.drawable().unwrap().draw(&mut p);
        let Some(PaintOp::Text { text, .. }) = p.texts().next() else {
            panic!("expected text")
        };
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn accelerators_pass_through_to_the_shell() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "x");
        let ctrl_s = KeyEvent::new(Key::Character('s'), Modifiers::ctrl());
        assert!(!tool.handle_key_event(ctrl_s).needs_redraw());
        // The 's' must not have been typed into the box.
        assert_eq!(tool.editing.as_ref().unwrap().text, "x");
    }

    // --- caret arithmetic on multi-byte text --------------------------------

    #[test]
    fn a_caret_set_mid_character_snaps_to_a_boundary_instead_of_panicking() {
        // The field used to be public with no invariant, so a caller could put
        // the caret inside a multi-byte character; the next edit then panicked
        // inside `insert_str`.
        let mut t = Text::committed(Vec2D::ZERO, "🙈abc", Style::default());
        for at in 0..="🙈abc".len() + 4 {
            t.set_cursor(Some(at));
            let cursor = t.cursor().unwrap();
            assert!(
                t.text.is_char_boundary(cursor),
                "caret {cursor} is not on a boundary for offset {at}"
            );
            // And every edit at that position is safe.
            let mut probe = t.clone();
            probe.insert("x");
            probe.backspace();
            probe.delete();
        }
    }

    #[test]
    fn the_editing_constructor_snaps_the_caret() {
        // Byte 2 is inside the emoji.
        let t = Text::editing(Vec2D::ZERO, "🙈nope", Style::default(), 2);
        assert_eq!(t.cursor(), Some(0), "should snap back to the boundary");
    }

    #[test]
    fn editing_multibyte_text_never_splits_a_character() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("한글");
        assert_eq!(t.cursor, Some("한글".len()));

        assert!(t.backspace());
        assert_eq!(t.text, "한");
        assert_eq!(t.cursor, Some("한".len()));

        assert!(t.move_left());
        assert_eq!(t.cursor, Some(0));
        assert!(!t.move_left(), "already at the start");

        assert!(t.delete());
        assert_eq!(t.text, "");
        assert!(!t.delete(), "nothing left to delete");
    }

    #[test]
    fn emoji_are_treated_as_single_characters() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("a🎉b");
        t.cursor = Some("a🎉".len());
        assert!(t.backspace());
        assert_eq!(t.text, "ab");
    }

    #[test]
    fn insertion_happens_at_the_caret_not_the_end() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("ac");
        t.move_left();
        t.insert("b");
        assert_eq!(t.text, "abc");
        assert_eq!(t.cursor, Some(2));
    }

    #[test]
    fn home_and_end_work_per_line() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("first\nsecond");
        assert!(t.move_home());
        assert_eq!(t.cursor, Some("first\n".len()));
        assert!(!t.move_home(), "already at the line start");
        assert!(t.move_end());
        assert_eq!(t.cursor, Some("first\nsecond".len()));
    }

    #[test]
    fn the_caret_cannot_run_off_either_end() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("hi");
        assert!(!t.move_right(), "already at the end");
        assert!(!t.delete());
        t.move_home();
        assert!(!t.move_left());
        assert!(!t.backspace());
        assert_eq!(t.text, "hi");
    }

    // --- input method composition -------------------------------------------

    #[test]
    fn preedit_is_displayed_at_the_caret_but_not_stored() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        type_str(&mut tool, "ab");
        tool.handle_text_event(&TextEvent::Preedit("한".to_owned()));

        let editing = tool.editing.as_ref().unwrap();
        assert_eq!(editing.text, "ab", "composition is not committed yet");
        assert_eq!(editing.displayed(), "ab한");
    }

    #[test]
    fn committing_a_composition_replaces_the_preedit() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        tool.handle_text_event(&TextEvent::Preedit("ㅎ".to_owned()));
        tool.handle_text_event(&TextEvent::Commit("한".to_owned()));

        let editing = tool.editing.as_ref().unwrap();
        assert_eq!(editing.text, "한");
        assert!(editing.preedit.is_empty());
        assert_eq!(editing.displayed(), "한");
    }

    #[test]
    fn an_unchanged_preedit_does_not_force_a_repaint() {
        let mut tool = TextTool::new(Style::default());
        click_at(&mut tool, Vec2D::ZERO);
        assert!(
            tool.handle_text_event(&TextEvent::Preedit("ㅎ".into()))
                .needs_redraw()
        );
        assert!(
            !tool
                .handle_text_event(&TextEvent::Preedit("ㅎ".into()))
                .needs_redraw()
        );
    }

    #[test]
    fn finishing_clears_the_caret_and_any_composition() {
        let mut t = Text::new(Vec2D::ZERO, Style::default());
        t.insert("x");
        t.preedit = "y".into();
        t.finish();
        assert!(t.cursor.is_none());
        assert!(t.preedit.is_empty());
        assert_eq!(t.displayed(), "x");
    }

    #[test]
    fn text_input_is_ignored_when_no_box_is_open() {
        let mut tool = TextTool::new(Style::default());
        assert!(
            !tool
                .handle_text_event(&TextEvent::Commit("x".into()))
                .needs_redraw()
        );
        assert!(!tool.is_active());
    }
}
