//! Screen capture for bettershot: enumerating monitors and windows, and
//! grabbing pixels.
//!
//! This is the **only** crate in the workspace allowed to contain OS-specific
//! code, and all of it lives under `backends/` behind `#[cfg(target_os = ...)]`.
//! Everything above that boundary — geometry, layout maths, target resolution,
//! stitching, backend selection — is platform-independent and unit tested,
//! because capture itself cannot be exercised on a headless build machine.
//!
//! # Quick start
//!
//! ```no_run
//! use std::time::Duration;
//! use bettershot_capture::{CaptureTarget, capture_after, default_backend};
//!
//! let backend = default_backend()?;
//! println!("using the {} backend", backend.name());
//!
//! for monitor in backend.monitors()? {
//!     println!("{} at {:?} @{}x", monitor.name, monitor.bounds, monitor.scale_factor);
//! }
//!
//! let frame = capture_after(
//!     backend.as_ref(),
//!     CaptureTarget::FullDesktop,
//!     Duration::from_secs(3),
//! )?;
//! println!("{}x{} RGBA", frame.width, frame.height);
//! # Ok::<(), bettershot_capture::CaptureError>(())
//! ```
//!
//! # Physical pixels are the canonical space
//!
//! Every rectangle, origin and size in this crate is in **physical device
//! pixels on the virtual desktop**. [`MonitorInfo::scale_factor`] and
//! [`RawFrame::scale_factor`] ride along as *metadata*; they are never applied
//! to the geometry.
//!
//! The reason is mixed DPI. A capture is physical pixels by definition, so any
//! other canonical space means converting on every grab — and every conversion
//! on a 125% or 150% display is a chance to be a pixel out. Worse, the logical
//! layout is not a uniform scaling of the physical one: two monitors at 100% and
//! 150% sit at physical x = 0 and x = 1920, and the second monitor's logical
//! coordinates restart at its own origin. There is no single divisor mapping the
//! virtual-physical plane onto a "virtual-logical" plane, so such a plane would
//! be a fiction.
//!
//! Scale conversions are therefore **monitor-local only**:
//! [`MonitorInfo::local_to_logical`] / [`MonitorInfo::logical_to_local`], and
//! [`VirtualDesktop::virtual_to_logical`], which finds the owning monitor first
//! and hands it back so a logical coordinate is never separated from the display
//! that defines it.
//!
//! Origins can be negative: a monitor left of or above the primary sits at a
//! negative x or y on Windows and X11 alike. Nothing here assumes otherwise.
//!
//! # Backend selection matrix
//!
//! [`default_backend`] reads the environment once ([`Environment::detect`]) and
//! then runs the pure [`select_backend`] over it:
//!
//! | OS      | Session evidence                                   | Backend                             |
//! | ------- | -------------------------------------------------- | ----------------------------------- |
//! | Windows | (any)                                              | `windows-graphics-capture`          |
//! | macOS   | (any)                                              | `macos-screencapturekit`            |
//! | Linux   | `XDG_SESSION_TYPE=wayland` + `WAYLAND_DISPLAY`      | `wayland-portal`                    |
//! | Linux   | `XDG_SESSION_TYPE=x11` + `DISPLAY`                  | `x11`                               |
//! | Linux   | `WAYLAND_DISPLAY` set (session type absent/wrong)   | `wayland-portal`                    |
//! | Linux   | `DISPLAY` set only                                  | `x11`                               |
//! | Linux   | `XDG_SESSION_TYPE=wayland` alone                    | `wayland-portal` (D-Bus only)       |
//! | Linux   | none of the above                                   | [`CaptureError::NoDisplay`]         |
//! | other   | (any)                                              | [`CaptureError::Unsupported`]       |
//!
//! An explicit `XDG_SESSION_TYPE=x11` outranks a stray `WAYLAND_DISPLAY`, which
//! some display managers leak into X sessions; otherwise a reachable Wayland
//! compositor wins over XWayland's `DISPLAY`, because the portal produces the
//! better image there. Empty environment variables count as unset.
//!
//! # Platform caveats
//!
//! **Linux / Wayland (`wayland-portal`, real).** Uses
//! `org.freedesktop.portal.Screenshot` over D-Bus via `ashpd`. The portal
//! returns a `file://` URI that must be read and then deleted. It exposes
//! *neither* monitor nor window geometry, so `monitors()` and `windows()`
//! return [`CaptureError::Unsupported`] instead of inventing numbers; only
//! full-desktop and desktop-relative region targets work, the latter served by
//! cropping the full-desktop image. The frame origin is assumed to be `(0, 0)`
//! and the scale factor is unknown (reported as `1.0`). Dismissing the portal
//! dialog is [`CaptureError::Cancelled`] — a normal outcome, not a failure —
//! and a refusal is [`CaptureError::PermissionDenied`].
//!
//! **Linux / X11 (`x11`, real).** A direct RandR + `GetImage` grab on `x11rb`.
//! `xcap` is deliberately *not* used on Linux: it pulls
//! `libwayshot`/`gbm`/`wayland-client`, which need Wayland and DRM development
//! libraries at build time even for an X11-only build, and that breaks headless
//! build machines. `x11rb` speaks the wire protocol directly and needs no system
//! libraries. X11 has no per-monitor DPI, so every monitor reports
//! `scale_factor: 1.0`; HiDPI on X11 is a toolkit convention and belongs to the
//! app shell. Grabs read the root window, so occluded window content is whatever
//! is painted over it, and no permission is ever required.
//!
//! **Windows (`windows-graphics-capture`, real).** Windows Graphics Capture /
//! DXGI through `xcap`, which cross-compiles cleanly to
//! `x86_64-pc-windows-msvc`. Monitor rectangles are only true physical pixels
//! when the host process is per-monitor-DPI-aware (v2) — the app shell's
//! manifest decides that, and this crate cannot detect it. There is no
//! whole-virtual-desktop grab, so full-desktop and cross-monitor regions are
//! assembled from per-monitor grabs with [`stitch`].
//!
//! **macOS (`macos-screencapturekit`, real but never run).** ScreenCaptureKit:
//! `SCShareableContent` for enumeration and `SCScreenshotManager`'s
//! `captureImageWithFilter:configuration:completionHandler:` for the grab, with
//! the asynchronous completion handlers bridged to synchronous calls by a
//! channel and a bounded wait. **This backend is compile-verified against the
//! real ScreenCaptureKit API but has never been executed on a Mac** — see
//! `backends::macos` for the first-real-hardware checklist. ScreenCaptureKit
//! needs macOS 12.3+ and `SCScreenshotManager` needs 14.0+; below those the
//! backend degrades to [`CaptureError::Unsupported`] naming the version rather
//! than crashing. Screen Recording is a TCC permission: it is preflighted with
//! `CGPreflightScreenCaptureAccess`, requested once with
//! `CGRequestScreenCaptureAccess`, and a refusal is
//! [`CaptureError::PermissionDenied`] naming *System Settings → Privacy &
//! Security → Screen & System Audio Recording* and the required relaunch.
//! ScreenCaptureKit reports geometry in **points**, which this backend
//! multiplies by each display's backing scale factor to reach the crate's
//! canonical physical-pixel space. Every `SCContentFilter` display initialiser
//! names exactly one display, so full-desktop and cross-display regions are
//! assembled from per-display grabs with [`stitch`]. `SCScreenshotManager` does
//! offer a multi-display `captureImageInRect:`, but its rect is in points and
//! it accepts no `SCStreamConfiguration` to size the output, so it would neither
//! remove the point-space origin skew nor let each display be grabbed at its own
//! native resolution; `backends::macos::monitors_covering` documents the full
//! argument.
//!
//! # What is testable without a display
//!
//! [`stitch`], [`RawFrame::crop`], [`resolve_target`], [`select_backend`],
//! [`VirtualDesktop`], [`window_at`], [`geometry::clamp_region`],
//! [`pixels::zpixmap_to_rgba`] and the whole platform-independent half of the
//! macOS backend (points→pixels conversion, `CGImage` row-padding unpacking,
//! front-to-back z-ordering, `SCStreamErrorDomain` classification) are all pure
//! functions over plain data. Backends are thin adapters that convert OS types
//! into those inputs, which is where nearly all of this crate's tests live.

pub mod backend;
mod backends;
pub mod env;
pub mod error;
pub mod frame;
pub mod geometry;
pub mod monitor;
pub mod pixels;
pub mod target;
pub mod window;

pub use backend::{CaptureBackend, backend_for, capture_after, default_backend};
pub use env::{BackendChoice, BackendSelection, Environment, TargetOs, select_backend};
pub use error::{CaptureError, classify_portal_error};
pub use frame::{BYTES_PER_PIXEL, RawFrame, stitch};
pub use geometry::PixelRect;
pub use monitor::{MonitorId, MonitorInfo, VirtualDesktop};
pub use target::{Capabilities, CaptureTarget, ResolvedTarget, resolve_target};
pub use window::{WindowId, WindowInfo, window_at};

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::{Rect, Vec2D};

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
                "hidpi",
                Rect::from_xywh(1920.0, -360.0, 2560.0, 1440.0),
                1.5,
                false,
            ),
        ])
    }

    /// The full journey a mixed-DPI `--capture all` takes: enumerate, resolve,
    /// grab each monitor, stitch, and land on the expected pixel grid.
    #[test]
    fn a_mixed_dpi_desktop_stitches_into_one_correctly_placed_frame() {
        let desktop = desktop();
        let resolved = resolve_target(CaptureTarget::FullDesktop, &desktop, &[]).unwrap();
        assert_eq!(
            resolved.bounds,
            Rect::from_xywh(0.0, -360.0, 4480.0, 1440.0)
        );

        // Two synthetic "grabs", one per monitor, at 1/40th scale so the test
        // stays cheap; the geometry relationships are what matter.
        let scale = 40.0;
        let frames: Vec<RawFrame> = desktop
            .monitors()
            .iter()
            .map(|m| {
                RawFrame::filled(
                    (m.bounds.width() / scale) as u32,
                    (m.bounds.height() / scale) as u32,
                    [m.id.get() as u8, 0, 0, 255],
                    m.origin() * (1.0 / scale),
                    m.scale_factor,
                )
                .unwrap()
            })
            .collect();

        let stitched = stitch(&frames).unwrap();
        assert_eq!(stitched.width, 4480 / 40);
        assert_eq!(stitched.height, 1440 / 40);
        assert_eq!(stitched.origin, Vec2D::new(0.0, -9.0));
        assert_eq!(stitched.scale_factor, 1.5);

        // Monitor 1 starts 9 rows down (it sits below the taller HiDPI one).
        assert_eq!(stitched.pixel(0, 9), Some([1, 0, 0, 255]));
        // Monitor 2 occupies the top-right.
        assert_eq!(stitched.pixel(48, 0), Some([2, 0, 0, 255]));
        // The top-left corner is desktop nobody covers: transparent.
        assert_eq!(stitched.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_region_dragged_across_the_seam_resolves_and_crops_consistently() {
        let desktop = desktop();
        let target = CaptureTarget::region(Rect::from_xywh(1900.0, 0.0, 40.0, 20.0));
        let resolved = resolve_target(target, &desktop, &[]).unwrap();
        assert_eq!(resolved.bounds, Rect::from_xywh(1900.0, 0.0, 40.0, 20.0));

        // Fake a full-desktop frame and cut the same region out of it.
        let full = RawFrame::transparent(4480, 1440, Vec2D::new(0.0, -360.0), 1.5).unwrap();
        let cropped = full.crop(resolved.bounds).unwrap();
        assert_eq!((cropped.width, cropped.height), (40, 20));
        assert_eq!(cropped.origin, Vec2D::new(1900.0, 0.0));
    }

    #[test]
    fn window_snapping_uses_the_same_coordinate_space_as_capture() {
        let desktop = desktop();
        let windows = vec![WindowInfo::new(
            WindowId::new(1),
            "Terminal",
            "kitty",
            Rect::from_xywh(2000.0, -200.0, 800.0, 600.0),
            false,
            0,
        )];
        // A hover on the HiDPI monitor snaps to the window...
        let hovered = window_at(&windows, Vec2D::new(2100.0, -100.0)).unwrap();
        // ...and capturing it resolves to the same rect, on that monitor.
        let resolved =
            resolve_target(CaptureTarget::Window(hovered.id), &desktop, &windows).unwrap();
        assert_eq!(resolved.bounds, hovered.bounds);
        assert_eq!(resolved.monitor, Some(MonitorId::new(2)));
        assert_eq!(resolved.scale_factor, 1.5);
    }

    #[test]
    fn backend_selection_and_construction_agree_on_names() {
        for (choice, expected) in [
            (BackendChoice::WaylandPortal, "wayland-portal"),
            (BackendChoice::X11, "x11"),
            (
                BackendChoice::WindowsGraphicsCapture,
                "windows-graphics-capture",
            ),
            (
                BackendChoice::MacOsScreenCaptureKit,
                "macos-screencapturekit",
            ),
        ] {
            assert_eq!(choice.name(), expected);
        }
        // The one backend constructible on every host must report the name its
        // choice advertises — on a non-Mac that is the placeholder standing in
        // for ScreenCaptureKit, which deliberately shares the name.
        let selection = BackendSelection {
            choice: BackendChoice::MacOsScreenCaptureKit,
            reason: "test".into(),
        };
        assert_eq!(
            backend_for(&selection).unwrap().name(),
            BackendChoice::MacOsScreenCaptureKit.name()
        );
    }

    #[test]
    fn default_backend_never_panics_on_a_headless_machine() {
        // On the headless CI/build box this is `Err(NoDisplay)`; on a developer
        // desktop it is a real backend. Either is fine — the contract is that
        // it resolves rather than panicking or hanging.
        match default_backend() {
            Ok(backend) => assert!(!backend.name().is_empty()),
            Err(err) => assert!(
                matches!(
                    err,
                    CaptureError::NoDisplay
                        | CaptureError::Unsupported(_)
                        | CaptureError::Backend(_)
                ),
                "unexpected error from default_backend: {err}"
            ),
        }
    }
}
