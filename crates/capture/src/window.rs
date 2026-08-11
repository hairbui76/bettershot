//! Top-level windows, and hit-testing them for the region-selection overlay.
//!
//! # Z-order convention
//!
//! [`WindowInfo::z_order`] is **0 for the frontmost window**, increasing away
//! from the viewer. Platforms disagree (X11's `_NET_CLIENT_LIST_STACKING` is
//! bottom-to-top, `xcap` on Windows gives larger-is-nearer), so every backend
//! normalises to this rule and hit-testing only has to implement it once.

use std::fmt;

use bettershot_core::{Rect, Vec2D};

use crate::{CaptureError, geometry::contains_half_open};

/// Backend-assigned window handle (an `HWND` on Windows, an X11 window id on
/// X11). Opaque, and only valid as long as the window lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// Wrap a backend-native handle.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The backend-native handle.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "window#{}", self.0)
    }
}

/// One top-level window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// Backend handle.
    pub id: WindowId,
    /// Window title as shown in the title bar / task switcher.
    pub title: String,
    /// Owning application, e.g. `firefox` or `Code.exe`.
    pub app_name: String,
    /// Frame position and size on the virtual desktop, in **physical pixels**.
    pub bounds: Rect,
    /// Minimised / iconified windows have no on-screen pixels and are skipped
    /// by hit-testing.
    pub is_minimized: bool,
    /// Stacking position, `0` = frontmost. See the module docs.
    pub z_order: u32,
}

impl WindowInfo {
    /// Build a window description.
    pub fn new(
        id: WindowId,
        title: impl Into<String>,
        app_name: impl Into<String>,
        bounds: Rect,
        is_minimized: bool,
        z_order: u32,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            app_name: app_name.into(),
            bounds: bounds.normalized(),
            is_minimized,
            z_order,
        }
    }

    /// Whether this window can contribute pixels: on screen and non-degenerate.
    pub fn is_visible(&self) -> bool {
        !self.is_minimized && !self.bounds.is_empty()
    }

    /// Half-open hit test against the window frame.
    pub fn contains(&self, point: Vec2D) -> bool {
        contains_half_open(self.bounds, point)
    }

    /// A short label for pickers: `title` when it has one, else the app name.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.app_name
        } else {
            &self.title
        }
    }
}

/// The topmost visible window under `point`, or `None` when the point is over
/// bare desktop.
///
/// Minimised and zero-sized windows are skipped. Among candidates the lowest
/// `z_order` wins; if two windows report the same `z_order` (some window
/// managers do not give a total order) the earlier entry in `windows` wins, so
/// the result is stable across calls.
pub fn window_at(windows: &[WindowInfo], point: Vec2D) -> Option<&WindowInfo> {
    windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.is_visible() && w.contains(point))
        .min_by_key(|(index, w)| (w.z_order, *index))
        .map(|(_, w)| w)
}

/// Every visible window under `point`, front to back. Useful for a "pick a
/// window behind this one" affordance.
pub fn windows_at(windows: &[WindowInfo], point: Vec2D) -> Vec<&WindowInfo> {
    let mut hits: Vec<(usize, &WindowInfo)> = windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.is_visible() && w.contains(point))
        .collect();
    hits.sort_by_key(|(index, w)| (w.z_order, *index));
    hits.into_iter().map(|(_, w)| w).collect()
}

/// All visible windows, front to back.
pub fn sorted_front_to_back(windows: &[WindowInfo]) -> Vec<&WindowInfo> {
    let mut visible: Vec<(usize, &WindowInfo)> = windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.is_visible())
        .collect();
    visible.sort_by_key(|(index, w)| (w.z_order, *index));
    visible.into_iter().map(|(_, w)| w).collect()
}

/// The frontmost visible window anywhere, i.e. the likely focused one.
pub fn topmost(windows: &[WindowInfo]) -> Option<&WindowInfo> {
    windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.is_visible())
        .min_by_key(|(index, w)| (w.z_order, *index))
        .map(|(_, w)| w)
}

/// Look a window up by id, or fail with [`CaptureError::NoSuchWindow`].
pub fn require(windows: &[WindowInfo], id: WindowId) -> Result<&WindowInfo, CaptureError> {
    windows
        .iter()
        .find(|w| w.id == id)
        .ok_or(CaptureError::NoSuchWindow(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u64, x: f32, y: f32, w: f32, h: f32, z: u32, minimized: bool) -> WindowInfo {
        WindowInfo::new(
            WindowId::new(id),
            format!("Window {id}"),
            "testapp",
            Rect::from_xywh(x, y, w, h),
            minimized,
            z,
        )
    }

    /// Three stacked windows, all overlapping at (150,150):
    /// id 1 front, id 2 middle, id 3 back.
    fn stack() -> Vec<WindowInfo> {
        vec![
            win(3, 0.0, 0.0, 800.0, 600.0, 2, false),
            win(1, 100.0, 100.0, 200.0, 200.0, 0, false),
            win(2, 50.0, 50.0, 400.0, 400.0, 1, false),
        ]
    }

    #[test]
    fn window_at_returns_the_frontmost_hit() {
        let windows = stack();
        assert_eq!(
            window_at(&windows, Vec2D::new(150.0, 150.0)).unwrap().id,
            WindowId::new(1)
        );
        // Outside window 1 but inside 2 and 3 -> 2 wins.
        assert_eq!(
            window_at(&windows, Vec2D::new(60.0, 60.0)).unwrap().id,
            WindowId::new(2)
        );
        // Only window 3 covers this.
        assert_eq!(
            window_at(&windows, Vec2D::new(10.0, 10.0)).unwrap().id,
            WindowId::new(3)
        );
    }

    #[test]
    fn window_at_misses_bare_desktop() {
        assert!(window_at(&stack(), Vec2D::new(5000.0, 5000.0)).is_none());
        assert!(window_at(&[], Vec2D::ZERO).is_none());
    }

    #[test]
    fn window_at_skips_minimized_windows_even_when_frontmost() {
        let mut windows = stack();
        windows[1].is_minimized = true; // id 1, z_order 0
        assert_eq!(
            window_at(&windows, Vec2D::new(150.0, 150.0)).unwrap().id,
            WindowId::new(2)
        );
    }

    #[test]
    fn window_at_skips_zero_sized_windows() {
        let windows = vec![
            win(1, 100.0, 100.0, 0.0, 200.0, 0, false),
            win(2, 50.0, 50.0, 400.0, 400.0, 1, false),
        ];
        assert_eq!(
            window_at(&windows, Vec2D::new(150.0, 150.0)).unwrap().id,
            WindowId::new(2)
        );
    }

    #[test]
    fn window_at_breaks_z_order_ties_by_enumeration_order() {
        let windows = vec![
            win(7, 0.0, 0.0, 100.0, 100.0, 4, false),
            win(8, 0.0, 0.0, 100.0, 100.0, 4, false),
        ];
        assert_eq!(
            window_at(&windows, Vec2D::new(50.0, 50.0)).unwrap().id,
            WindowId::new(7)
        );
    }

    #[test]
    fn window_hit_testing_is_half_open() {
        let windows = vec![win(1, 0.0, 0.0, 100.0, 100.0, 0, false)];
        assert!(window_at(&windows, Vec2D::new(0.0, 0.0)).is_some());
        assert!(window_at(&windows, Vec2D::new(99.0, 99.0)).is_some());
        assert!(window_at(&windows, Vec2D::new(100.0, 50.0)).is_none());
        assert!(window_at(&windows, Vec2D::new(50.0, 100.0)).is_none());
    }

    #[test]
    fn window_at_works_with_negative_coordinates() {
        let windows = vec![win(1, -900.0, -300.0, 400.0, 400.0, 0, false)];
        assert_eq!(
            window_at(&windows, Vec2D::new(-700.0, -100.0)).unwrap().id,
            WindowId::new(1)
        );
        assert!(window_at(&windows, Vec2D::new(-1000.0, -100.0)).is_none());
    }

    #[test]
    fn windows_at_lists_every_hit_front_to_back() {
        let ids: Vec<u64> = windows_at(&stack(), Vec2D::new(150.0, 150.0))
            .iter()
            .map(|w| w.id.get())
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn sorted_front_to_back_drops_minimized_windows() {
        let mut windows = stack();
        windows[0].is_minimized = true; // id 3
        let ids: Vec<u64> = sorted_front_to_back(&windows)
            .iter()
            .map(|w| w.id.get())
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn topmost_is_the_lowest_z_order_visible_window() {
        assert_eq!(topmost(&stack()).unwrap().id, WindowId::new(1));
        let all_minimized: Vec<WindowInfo> = stack()
            .into_iter()
            .map(|mut w| {
                w.is_minimized = true;
                w
            })
            .collect();
        assert!(topmost(&all_minimized).is_none());
    }

    #[test]
    fn require_reports_missing_windows() {
        let windows = stack();
        assert!(require(&windows, WindowId::new(2)).is_ok());
        assert!(matches!(
            require(&windows, WindowId::new(404)),
            Err(CaptureError::NoSuchWindow(id)) if id == WindowId::new(404)
        ));
    }

    #[test]
    fn label_falls_back_to_the_app_name() {
        let mut w = win(1, 0.0, 0.0, 10.0, 10.0, 0, false);
        assert_eq!(w.label(), "Window 1");
        w.title = "   ".into();
        assert_eq!(w.label(), "testapp");
    }

    #[test]
    fn window_id_displays_readably() {
        assert_eq!(WindowId::new(12).to_string(), "window#12");
        assert_eq!(WindowId::new(12).get(), 12);
    }
}
