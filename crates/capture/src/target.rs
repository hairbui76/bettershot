//! What to capture, what a backend can capture, and the pure resolution step
//! between the two.
//!
//! Resolving a [`CaptureTarget`] against an enumeration result is where every
//! "that monitor is gone", "that window closed" and "that region is off-screen"
//! error is produced. Keeping it in one pure function means the messy cases are
//! testable without a display.

use bettershot_core::Rect;

use crate::{
    CaptureError, MonitorId, VirtualDesktop, WindowId, WindowInfo, geometry::PixelRect, window,
};

/// What the caller wants a picture of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureTarget {
    /// Every monitor, stitched into a single image spanning the virtual
    /// desktop's bounding box.
    FullDesktop,
    /// One monitor, in full.
    Monitor(MonitorId),
    /// One window's frame, as currently stacked.
    Window(WindowId),
    /// A rectangle.
    ///
    /// * `monitor: None` — `rect` is in **virtual-desktop** coordinates and is
    ///   clipped to the whole desktop.
    /// * `monitor: Some(id)` — `rect` is in that monitor's **local** physical
    ///   coordinates (origin at its top-left) and is clipped to it. This is
    ///   what a per-monitor selection overlay produces, and it means the
    ///   overlay never has to know where its monitor sits on the desktop.
    Region {
        /// The monitor `rect` is relative to, if any.
        monitor: Option<MonitorId>,
        /// The requested rectangle.
        rect: Rect,
    },
}

impl CaptureTarget {
    /// A whole-desktop-relative region.
    pub fn region(rect: Rect) -> Self {
        Self::Region {
            monitor: None,
            rect,
        }
    }

    /// A region in one monitor's local coordinates.
    pub fn region_on(monitor: MonitorId, rect: Rect) -> Self {
        Self::Region {
            monitor: Some(monitor),
            rect,
        }
    }

    /// A short name for logs and error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FullDesktop => "full-desktop",
            Self::Monitor(_) => "monitor",
            Self::Window(_) => "window",
            Self::Region { .. } => "region",
        }
    }
}

/// Which targets a backend can actually serve.
///
/// The app shell uses this to grey out capture modes rather than letting the
/// user pick something that will fail — the Wayland screenshot portal, for
/// instance, can produce a full-desktop image but cannot enumerate monitors or
/// target a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Can capture the whole virtual desktop.
    pub full_desktop: bool,
    /// Can capture a named monitor.
    pub monitor: bool,
    /// Can capture a named window.
    pub window: bool,
    /// Can capture an arbitrary rectangle.
    pub region: bool,
    /// `monitors()` returns real data rather than an error.
    pub monitor_enumeration: bool,
    /// `windows()` returns real data rather than an error.
    pub window_enumeration: bool,
    /// Every capture pops a system dialog the user must confirm. Callers should
    /// not use this backend for background/hotkey captures without warning.
    pub interactive_only: bool,
    /// Capturing may raise a permission prompt, and may fail with
    /// [`CaptureError::PermissionDenied`].
    pub may_prompt_for_permission: bool,
    /// [`crate::CaptureBackend::cursor`] can return a real cursor bitmap, so
    /// `--include-cursor` will do something. False on the Wayland portal, whose
    /// Screenshot API has no cursor control at all.
    pub cursor: bool,
}

impl Capabilities {
    /// A backend that can do nothing — the starting point for stubs.
    pub const NONE: Self = Self {
        full_desktop: false,
        monitor: false,
        window: false,
        region: false,
        monitor_enumeration: false,
        window_enumeration: false,
        interactive_only: false,
        may_prompt_for_permission: false,
        cursor: false,
    };

    /// A backend with unrestricted direct access to the display server: every
    /// target, no prompts. X11 and Windows both look like this.
    pub const FULL: Self = Self {
        full_desktop: true,
        monitor: true,
        window: true,
        region: true,
        monitor_enumeration: true,
        window_enumeration: true,
        interactive_only: false,
        may_prompt_for_permission: false,
        cursor: true,
    };

    /// Can this backend serve `target`?
    pub fn supports(&self, target: &CaptureTarget) -> bool {
        match target {
            CaptureTarget::FullDesktop => self.full_desktop,
            CaptureTarget::Monitor(_) => self.monitor,
            CaptureTarget::Window(_) => self.window,
            CaptureTarget::Region { .. } => self.region,
        }
    }

    /// [`Capabilities::supports`] as a `Result`, with a message naming the
    /// backend so the user knows *why* it is refusing.
    pub fn ensure_supports(
        &self,
        backend: &str,
        target: &CaptureTarget,
    ) -> Result<(), CaptureError> {
        if self.supports(target) {
            Ok(())
        } else {
            Err(CaptureError::unsupported(format!(
                "the {backend} backend cannot capture a {} target",
                target.kind()
            )))
        }
    }
}

/// A [`CaptureTarget`] checked against reality: the ids exist, and the
/// rectangle is a non-empty area of the virtual desktop in physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTarget {
    /// The target this came from.
    pub target: CaptureTarget,
    /// Area to grab, in virtual-desktop physical pixels.
    pub bounds: Rect,
    /// The monitor whose pixel grid the result belongs to: the target monitor,
    /// the one a window or region mostly sits on, or `None` for a full-desktop
    /// capture spanning several.
    pub monitor: Option<MonitorId>,
    /// Scale factor to stamp on the resulting [`crate::RawFrame`].
    pub scale_factor: f32,
}

impl ResolvedTarget {
    /// The area to grab, snapped to the pixel grid.
    pub fn pixel_bounds(&self) -> Result<PixelRect, CaptureError> {
        PixelRect::from_rect(self.bounds)
    }
}

/// Turn a request into concrete pixels-on-the-desktop, or explain why it cannot
/// be served.
///
/// Errors:
/// * [`CaptureError::NoDisplay`] — the desktop has no monitors at all.
/// * [`CaptureError::NoSuchMonitor`] / [`CaptureError::NoSuchWindow`] — the id
///   went stale between enumeration and capture.
/// * [`CaptureError::EmptyRegion`] — the region is degenerate, or misses the
///   monitor / desktop it was clipped against.
pub fn resolve_target(
    target: CaptureTarget,
    desktop: &VirtualDesktop,
    windows: &[WindowInfo],
) -> Result<ResolvedTarget, CaptureError> {
    if desktop.is_empty() {
        return Err(CaptureError::NoDisplay);
    }

    let (bounds, monitor) = match target {
        CaptureTarget::FullDesktop => {
            let bounds = desktop.bounds();
            if bounds.is_empty() {
                return Err(CaptureError::EmptyRegion);
            }
            // Only claim a single monitor when there literally is only one.
            let single = (desktop.len() == 1).then(|| desktop.monitors()[0].id);
            (bounds, single)
        }
        CaptureTarget::Monitor(id) => {
            let monitor = desktop.require(id)?;
            if monitor.bounds.is_empty() {
                return Err(CaptureError::EmptyRegion);
            }
            (monitor.bounds, Some(id))
        }
        CaptureTarget::Window(id) => {
            let win = window::require(windows, id)?;
            if win.is_minimized {
                return Err(CaptureError::unsupported(format!(
                    "window {id} ({}) is minimized and has no on-screen pixels",
                    win.label()
                )));
            }
            let bounds = win.bounds;
            if bounds.is_empty() {
                return Err(CaptureError::EmptyRegion);
            }
            // A window may hang off the edge of the desktop; capture only the
            // part that exists.
            let visible = desktop.clamp_region(bounds)?;
            let monitor = desktop.monitor_for_region(visible).map(|m| m.id);
            (visible, monitor)
        }
        CaptureTarget::Region { monitor, rect } => match monitor {
            Some(id) => {
                let m = desktop.require(id)?;
                (m.clamp_local_region(rect)?, Some(id))
            }
            None => {
                let clipped = desktop.clamp_region(rect)?;
                let owner = desktop.monitor_for_region(clipped).map(|m| m.id);
                (clipped, owner)
            }
        },
    };

    let scale_factor = monitor
        .and_then(|id| desktop.get(id))
        .map(|m| m.scale_factor)
        .unwrap_or_else(|| desktop.max_scale_factor());

    Ok(ResolvedTarget {
        target,
        bounds,
        monitor,
        scale_factor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MonitorInfo, WindowInfo};
    use bettershot_core::Vec2D;

    fn desktop() -> VirtualDesktop {
        VirtualDesktop::new(vec![
            MonitorInfo::new(
                MonitorId::new(1),
                "primary",
                Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0),
                1.0,
                true,
            ),
            MonitorInfo::new(
                MonitorId::new(2),
                "hidpi-left",
                Rect::from_xywh(-2560.0, 0.0, 2560.0, 1440.0),
                1.5,
                false,
            ),
        ])
    }

    fn windows() -> Vec<WindowInfo> {
        vec![
            WindowInfo::new(
                WindowId::new(10),
                "Editor",
                "code",
                Rect::from_xywh(100.0, 100.0, 800.0, 600.0),
                false,
                0,
            ),
            WindowInfo::new(
                WindowId::new(11),
                "Chat",
                "chat",
                Rect::from_xywh(-2400.0, 200.0, 600.0, 400.0),
                false,
                1,
            ),
            WindowInfo::new(
                WindowId::new(12),
                "Hidden",
                "bg",
                Rect::from_xywh(0.0, 0.0, 400.0, 400.0),
                true,
                2,
            ),
            WindowInfo::new(
                WindowId::new(13),
                "Half off-screen",
                "stray",
                Rect::from_xywh(1800.0, 0.0, 400.0, 200.0),
                false,
                3,
            ),
        ]
    }

    #[test]
    fn full_desktop_resolves_to_the_bounding_box() {
        let r = resolve_target(CaptureTarget::FullDesktop, &desktop(), &[]).unwrap();
        assert_eq!(r.bounds, Rect::from_xywh(-2560.0, 0.0, 4480.0, 1440.0));
        assert_eq!(r.monitor, None);
        // Mixed DPI: the stitched frame inherits the densest grid.
        assert_eq!(r.scale_factor, 1.5);
    }

    #[test]
    fn full_desktop_on_a_single_monitor_names_that_monitor() {
        let single = VirtualDesktop::new(vec![MonitorInfo::new(
            MonitorId::new(1),
            "only",
            Rect::from_xywh(0.0, 0.0, 800.0, 600.0),
            2.0,
            true,
        )]);
        let r = resolve_target(CaptureTarget::FullDesktop, &single, &[]).unwrap();
        assert_eq!(r.monitor, Some(MonitorId::new(1)));
        assert_eq!(r.scale_factor, 2.0);
    }

    #[test]
    fn every_target_fails_with_no_display_when_nothing_is_attached() {
        let empty = VirtualDesktop::default();
        for target in [
            CaptureTarget::FullDesktop,
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(10)),
            CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
        ] {
            assert!(matches!(
                resolve_target(target, &empty, &windows()),
                Err(CaptureError::NoDisplay)
            ));
        }
    }

    #[test]
    fn monitor_target_resolves_to_its_physical_bounds_and_scale() {
        let r = resolve_target(CaptureTarget::Monitor(MonitorId::new(2)), &desktop(), &[]).unwrap();
        assert_eq!(r.bounds, Rect::from_xywh(-2560.0, 0.0, 2560.0, 1440.0));
        assert_eq!(r.monitor, Some(MonitorId::new(2)));
        assert_eq!(r.scale_factor, 1.5);
    }

    #[test]
    fn stale_monitor_ids_are_rejected() {
        assert!(matches!(
            resolve_target(CaptureTarget::Monitor(MonitorId::new(99)), &desktop(), &[]),
            Err(CaptureError::NoSuchMonitor(_))
        ));
    }

    #[test]
    fn window_target_resolves_to_its_frame_and_owning_monitor() {
        let r = resolve_target(
            CaptureTarget::Window(WindowId::new(11)),
            &desktop(),
            &windows(),
        )
        .unwrap();
        assert_eq!(r.bounds, Rect::from_xywh(-2400.0, 200.0, 600.0, 400.0));
        assert_eq!(r.monitor, Some(MonitorId::new(2)));
        assert_eq!(r.scale_factor, 1.5);
    }

    #[test]
    fn window_target_is_clipped_to_the_desktop() {
        let r = resolve_target(
            CaptureTarget::Window(WindowId::new(13)),
            &desktop(),
            &windows(),
        )
        .unwrap();
        // The window is 400 wide starting at x=1800; the desktop ends at 1920.
        assert_eq!(r.bounds, Rect::from_xywh(1800.0, 0.0, 120.0, 200.0));
        assert_eq!(r.monitor, Some(MonitorId::new(1)));
    }

    #[test]
    fn stale_window_ids_are_rejected() {
        assert!(matches!(
            resolve_target(
                CaptureTarget::Window(WindowId::new(404)),
                &desktop(),
                &windows()
            ),
            Err(CaptureError::NoSuchWindow(_))
        ));
    }

    #[test]
    fn minimized_windows_cannot_be_captured() {
        let err = resolve_target(
            CaptureTarget::Window(WindowId::new(12)),
            &desktop(),
            &windows(),
        )
        .unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
        assert!(err.to_string().contains("minimized"));
    }

    #[test]
    fn desktop_relative_regions_are_clipped_to_the_desktop() {
        let r = resolve_target(
            CaptureTarget::region(Rect::from_xywh(-3000.0, -100.0, 1000.0, 500.0)),
            &desktop(),
            &[],
        )
        .unwrap();
        // Requested x -3000..-2000, y -100..400; the desktop starts at x=-2560
        // and y=0, so 560x400 survives.
        assert_eq!(r.bounds, Rect::from_xywh(-2560.0, 0.0, 560.0, 400.0));
        assert_eq!(r.monitor, Some(MonitorId::new(2)));
    }

    #[test]
    fn monitor_relative_regions_are_translated_then_clipped() {
        // Local (10,20) on the left monitor is virtual (-2550, 20).
        let r = resolve_target(
            CaptureTarget::region_on(MonitorId::new(2), Rect::from_xywh(10.0, 20.0, 100.0, 50.0)),
            &desktop(),
            &[],
        )
        .unwrap();
        assert_eq!(r.bounds, Rect::from_xywh(-2550.0, 20.0, 100.0, 50.0));
        assert_eq!(r.monitor, Some(MonitorId::new(2)));
        assert_eq!(r.scale_factor, 1.5);
    }

    #[test]
    fn the_same_rect_means_different_things_with_and_without_a_monitor() {
        let rect = Rect::from_xywh(10.0, 20.0, 100.0, 50.0);
        let global = resolve_target(CaptureTarget::region(rect), &desktop(), &[]).unwrap();
        let local = resolve_target(
            CaptureTarget::region_on(MonitorId::new(2), rect),
            &desktop(),
            &[],
        )
        .unwrap();
        assert_eq!(global.bounds, Rect::from_xywh(10.0, 20.0, 100.0, 50.0));
        assert_eq!(local.bounds, Rect::from_xywh(-2550.0, 20.0, 100.0, 50.0));
    }

    #[test]
    fn monitor_relative_regions_running_off_the_monitor_are_clipped_to_it() {
        // Local x 2500..2700 on a 2560-wide monitor keeps only 60 columns, even
        // though the neighbouring monitor covers the rest of the desktop there.
        let r = resolve_target(
            CaptureTarget::region_on(
                MonitorId::new(2),
                Rect::from_xywh(2500.0, 0.0, 200.0, 100.0),
            ),
            &desktop(),
            &[],
        )
        .unwrap();
        assert_eq!(r.bounds, Rect::from_xywh(-60.0, 0.0, 60.0, 100.0));
    }

    #[test]
    fn empty_and_off_desktop_regions_are_rejected() {
        assert!(matches!(
            resolve_target(
                CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 0.0, 10.0)),
                &desktop(),
                &[]
            ),
            Err(CaptureError::EmptyRegion)
        ));
        assert!(matches!(
            resolve_target(
                CaptureTarget::region(Rect::from_xywh(9000.0, 9000.0, 10.0, 10.0)),
                &desktop(),
                &[]
            ),
            Err(CaptureError::EmptyRegion)
        ));
        assert!(matches!(
            resolve_target(
                CaptureTarget::region_on(
                    MonitorId::new(1),
                    Rect::from_xywh(5000.0, 0.0, 10.0, 10.0)
                ),
                &desktop(),
                &[]
            ),
            Err(CaptureError::EmptyRegion)
        ));
    }

    #[test]
    fn monitor_relative_regions_reject_stale_monitor_ids() {
        assert!(matches!(
            resolve_target(
                CaptureTarget::region_on(MonitorId::new(77), Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
                &desktop(),
                &[]
            ),
            Err(CaptureError::NoSuchMonitor(_))
        ));
    }

    #[test]
    fn resolved_targets_snap_to_the_pixel_grid() {
        let r = resolve_target(
            CaptureTarget::region(Rect::from_xywh(10.4, 20.6, 100.0, 50.0)),
            &desktop(),
            &[],
        )
        .unwrap();
        assert_eq!(r.pixel_bounds().unwrap(), PixelRect::new(10, 21, 100, 50));
    }

    #[test]
    fn capabilities_gate_targets() {
        let portal = Capabilities {
            full_desktop: true,
            region: true,
            may_prompt_for_permission: true,
            ..Capabilities::NONE
        };
        assert!(portal.supports(&CaptureTarget::FullDesktop));
        assert!(portal.supports(&CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 1.0, 1.0))));
        assert!(!portal.supports(&CaptureTarget::Monitor(MonitorId::new(1))));
        assert!(!portal.supports(&CaptureTarget::Window(WindowId::new(1))));

        assert!(Capabilities::FULL.supports(&CaptureTarget::Window(WindowId::new(1))));
        assert!(!Capabilities::NONE.supports(&CaptureTarget::FullDesktop));
    }

    #[test]
    fn ensure_supports_names_the_backend_and_the_target_kind() {
        let err = Capabilities::NONE
            .ensure_supports("portal", &CaptureTarget::Window(WindowId::new(1)))
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("portal"), "{text}");
        assert!(text.contains("window"), "{text}");
        assert!(
            Capabilities::FULL
                .ensure_supports("x11", &CaptureTarget::FullDesktop)
                .is_ok()
        );
    }

    #[test]
    fn target_kinds_are_stable_strings() {
        assert_eq!(CaptureTarget::FullDesktop.kind(), "full-desktop");
        assert_eq!(CaptureTarget::Monitor(MonitorId::new(1)).kind(), "monitor");
        assert_eq!(CaptureTarget::Window(WindowId::new(1)).kind(), "window");
        assert_eq!(
            CaptureTarget::region(Rect::new(Vec2D::ZERO, Vec2D::splat(1.0))).kind(),
            "region"
        );
    }
}
