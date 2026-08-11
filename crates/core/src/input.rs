//! Semantic input events, independent of any windowing toolkit.
//!
//! The app shell translates winit/egui input into these types; tools only ever
//! see this vocabulary. [`PointerTracker`] turns raw press/move/release streams
//! into click-vs-drag gestures, which is fiddly enough to be worth unit
//! testing away from the event loop.

use crate::math::Vec2D;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Windows key / Command key.
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };

    pub fn shift() -> Self {
        Self {
            shift: true,
            ..Self::NONE
        }
    }

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            ..Self::NONE
        }
    }

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// True when the platform's primary accelerator modifier is held: Command
    /// on macOS, Control elsewhere.
    pub fn command(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta
        } else {
            self.ctrl
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    /// The pointer moved with no button held.
    Motion,
    /// Button went down; may still become either a click or a drag.
    Press,
    /// Movement past the drag threshold while a button is held.
    BeginDrag,
    UpdateDrag,
    EndDrag,
    /// Button released without ever passing the drag threshold.
    Click,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    /// Absolute position in image-pixel space.
    pub pos: Vec2D,
    /// Offset from where the current gesture started. Zero outside a drag.
    pub delta: Vec2D,
    pub button: MouseButton,
    pub modifiers: Modifiers,
}

impl MouseEvent {
    /// Where the current gesture began.
    pub fn start(&self) -> Vec2D {
        self.pos - self.delta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Plus,
    Minus,
    Character(char),
    /// A key this crate does not model.
    Other,
}

impl Key {
    /// The character this key produces, if any.
    pub fn as_char(&self) -> Option<char> {
        match self {
            Key::Character(c) => Some(*c),
            Key::Space => Some(' '),
            Key::Plus => Some('+'),
            Key::Minus => Some('-'),
            _ => None,
        }
    }

    /// The digit this key represents, treating `0` as 10 the way the colour
    /// palette shortcuts do.
    pub fn as_palette_index(&self) -> Option<usize> {
        match self.as_char()? {
            '0' => Some(9),
            c @ '1'..='9' => Some(c as usize - '1' as usize),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    pub fn plain(key: Key) -> Self {
        Self::new(key, Modifiers::NONE)
    }
}

/// Committed text from the keyboard or an input method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEvent {
    /// Text was committed (typed directly, or accepted from an IME).
    Commit(String),
    /// In-progress IME composition, to be shown but not yet committed.
    Preedit(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Text(TextEvent),
}

impl InputEvent {
    pub fn as_mouse(&self) -> Option<&MouseEvent> {
        match self {
            InputEvent::Mouse(e) => Some(e),
            _ => None,
        }
    }
}

/// Distance in image pixels the pointer must travel with a button held before
/// the gesture counts as a drag rather than a click.
pub const DRAG_THRESHOLD: f32 = 3.0;

/// Converts raw pointer input into click and drag gestures.
///
/// The tricky parts this centralizes: a press is not reported as a drag until
/// the threshold is crossed (so a sloppy click is still a click), a release
/// before the threshold produces `Click`, and a release after it produces
/// `EndDrag` — never both.
#[derive(Debug, Default, Clone)]
pub struct PointerTracker {
    state: Option<GestureState>,
    threshold: f32,
}

#[derive(Debug, Clone, Copy)]
struct GestureState {
    button: MouseButton,
    start: Vec2D,
    dragging: bool,
}

impl PointerTracker {
    pub fn new() -> Self {
        Self {
            state: None,
            threshold: DRAG_THRESHOLD,
        }
    }

    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            state: None,
            threshold,
        }
    }

    /// True while a button is held.
    pub fn is_pressed(&self) -> bool {
        self.state.is_some()
    }

    /// True once the current gesture has become a drag.
    pub fn is_dragging(&self) -> bool {
        self.state.is_some_and(|s| s.dragging)
    }

    pub fn drag_start(&self) -> Option<Vec2D> {
        self.state.map(|s| s.start)
    }

    /// Abandon the current gesture without emitting an event (used when a tool
    /// is switched or the window loses focus).
    pub fn cancel(&mut self) {
        self.state = None;
    }

    pub fn press(&mut self, pos: Vec2D, button: MouseButton, modifiers: Modifiers) -> MouseEvent {
        self.state = Some(GestureState {
            button,
            start: pos,
            dragging: false,
        });
        MouseEvent {
            kind: MouseEventKind::Press,
            pos,
            delta: Vec2D::ZERO,
            button,
            modifiers,
        }
    }

    pub fn motion(&mut self, pos: Vec2D, modifiers: Modifiers) -> MouseEvent {
        match &mut self.state {
            Some(state) => {
                let delta = pos - state.start;
                let kind = if state.dragging {
                    MouseEventKind::UpdateDrag
                } else if delta.norm() >= self.threshold {
                    state.dragging = true;
                    MouseEventKind::BeginDrag
                } else {
                    // Still within the slop radius: not a drag yet.
                    MouseEventKind::Motion
                };
                MouseEvent {
                    kind,
                    pos,
                    delta,
                    button: state.button,
                    modifiers,
                }
            }
            None => MouseEvent {
                kind: MouseEventKind::Motion,
                pos,
                delta: Vec2D::ZERO,
                button: MouseButton::Left,
                modifiers,
            },
        }
    }

    pub fn release(&mut self, pos: Vec2D, modifiers: Modifiers) -> Option<MouseEvent> {
        let state = self.state.take()?;
        let delta = pos - state.start;
        Some(MouseEvent {
            kind: if state.dragging {
                MouseEventKind::EndDrag
            } else {
                MouseEventKind::Click
            },
            pos,
            delta,
            button: state.button,
            modifiers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_and_release_in_place_is_a_click() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::new(10.0, 10.0), MouseButton::Left, Modifiers::NONE);
        let e = t.release(Vec2D::new(10.0, 10.0), Modifiers::NONE).unwrap();
        assert_eq!(e.kind, MouseEventKind::Click);
        assert!(!t.is_pressed());
    }

    #[test]
    fn small_jitter_still_counts_as_a_click() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::new(10.0, 10.0), MouseButton::Left, Modifiers::NONE);
        let m = t.motion(Vec2D::new(11.0, 10.0), Modifiers::NONE);
        assert_eq!(m.kind, MouseEventKind::Motion, "under threshold");
        let e = t.release(Vec2D::new(11.0, 10.0), Modifiers::NONE).unwrap();
        assert_eq!(e.kind, MouseEventKind::Click);
    }

    #[test]
    fn crossing_the_threshold_begins_a_drag_exactly_once() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::new(0.0, 0.0), MouseButton::Left, Modifiers::NONE);
        assert_eq!(
            t.motion(Vec2D::new(10.0, 0.0), Modifiers::NONE).kind,
            MouseEventKind::BeginDrag
        );
        assert_eq!(
            t.motion(Vec2D::new(20.0, 0.0), Modifiers::NONE).kind,
            MouseEventKind::UpdateDrag
        );
        assert_eq!(
            t.motion(Vec2D::new(30.0, 0.0), Modifiers::NONE).kind,
            MouseEventKind::UpdateDrag
        );
        let end = t.release(Vec2D::new(30.0, 0.0), Modifiers::NONE).unwrap();
        assert_eq!(end.kind, MouseEventKind::EndDrag);
        assert_eq!(end.delta, Vec2D::new(30.0, 0.0));
        assert_eq!(end.start(), Vec2D::ZERO);
    }

    #[test]
    fn a_drag_never_also_reports_a_click() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        t.motion(Vec2D::new(50.0, 50.0), Modifiers::NONE);
        // Returning to the origin does not undo the drag.
        t.motion(Vec2D::ZERO, Modifiers::NONE);
        let e = t.release(Vec2D::ZERO, Modifiers::NONE).unwrap();
        assert_eq!(e.kind, MouseEventKind::EndDrag);
    }

    #[test]
    fn motion_without_a_press_is_plain_motion() {
        let mut t = PointerTracker::new();
        let e = t.motion(Vec2D::new(5.0, 5.0), Modifiers::NONE);
        assert_eq!(e.kind, MouseEventKind::Motion);
        assert_eq!(e.delta, Vec2D::ZERO);
    }

    #[test]
    fn release_without_a_press_yields_nothing() {
        let mut t = PointerTracker::new();
        assert!(t.release(Vec2D::ZERO, Modifiers::NONE).is_none());
    }

    #[test]
    fn cancel_drops_the_gesture_silently() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        t.motion(Vec2D::new(50.0, 0.0), Modifiers::NONE);
        assert!(t.is_dragging());
        t.cancel();
        assert!(!t.is_pressed());
        assert!(t.release(Vec2D::ZERO, Modifiers::NONE).is_none());
    }

    #[test]
    fn the_button_is_carried_through_the_gesture() {
        let mut t = PointerTracker::new();
        t.press(Vec2D::ZERO, MouseButton::Middle, Modifiers::NONE);
        let m = t.motion(Vec2D::new(20.0, 0.0), Modifiers::NONE);
        assert_eq!(m.button, MouseButton::Middle);
        let e = t.release(Vec2D::new(20.0, 0.0), Modifiers::NONE).unwrap();
        assert_eq!(e.button, MouseButton::Middle);
    }

    #[test]
    fn palette_index_maps_digits_with_zero_last() {
        assert_eq!(Key::Character('1').as_palette_index(), Some(0));
        assert_eq!(Key::Character('9').as_palette_index(), Some(8));
        assert_eq!(Key::Character('0').as_palette_index(), Some(9));
        assert_eq!(Key::Character('a').as_palette_index(), None);
        assert_eq!(Key::Escape.as_palette_index(), None);
    }

    #[test]
    fn command_modifier_follows_the_platform() {
        let ctrl = Modifiers::ctrl();
        let meta = Modifiers {
            meta: true,
            ..Modifiers::NONE
        };
        if cfg!(target_os = "macos") {
            assert!(meta.command() && !ctrl.command());
        } else {
            assert!(ctrl.command() && !meta.command());
        }
    }
}
