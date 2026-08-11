//! Acquiring an image to annotate.
//!
//! Extracted from `main` so a capture can be started two ways: once at startup
//! from the command line, and repeatedly from a hotkey or the tray while the
//! process stays resident.
//!
//! The screen is always grabbed **before** any selection UI appears. That is
//! what stops the overlay from appearing in its own screenshot, and it freezes
//! the scene so nothing shifts under the pointer mid-drag.

use anyhow::{Context, Result, bail};
use bettershot_capture::{CaptureBackend, CaptureTarget, RawFrame, WindowInfo};
use bettershot_core::config::{CaptureMode, Config};
use bettershot_core::math::Vec2D;
use image::RgbaImage;

/// A frozen frame plus what is needed to narrow it down.
pub struct Acquired {
    pub image: RgbaImage,
    /// Windows to snap to, empty when snapping is off or enumeration failed.
    pub windows: Vec<WindowInfo>,
    /// Where the frame sits in the virtual desktop, so window rectangles can
    /// be rebased onto it.
    pub origin: Vec2D,
    /// Whether the user still has to choose a region or a window.
    pub needs_selection: bool,
    pub mode: CaptureMode,
}

/// Capture the screen for `mode`.
pub fn acquire(mode: CaptureMode, config: &Config) -> Result<Acquired> {
    let backend =
        bettershot_capture::default_backend().context("no screen capture backend is available")?;
    log::info!("capturing with the {} backend", backend.name());

    if config.capture.delay_seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(config.capture.delay_seconds));
    }

    let target = match mode {
        // Region and window selection both need the whole desktop to choose
        // from; the overlay narrows it down afterwards.
        CaptureMode::All | CaptureMode::Region | CaptureMode::Window => CaptureTarget::FullDesktop,
        CaptureMode::Monitor => primary_monitor_target(backend.as_ref()),
    };

    let mut frame = backend.capture(target).context("capturing the screen")?;

    // The pointer is never in the captured pixels — compositors draw it on a
    // separate plane — so it has to be fetched and blended in separately.
    if config.capture.include_cursor {
        if backend.capabilities().cursor {
            bettershot_capture::draw_cursor_into(backend.as_ref(), &mut frame);
        } else {
            log::warn!(
                "the {} backend cannot supply the cursor, so --include-cursor has no effect",
                backend.name()
            );
        }
    }

    let image = non_empty(frame_to_image(&frame)?)?;

    let needs_selection = matches!(mode, CaptureMode::Region | CaptureMode::Window);
    let windows =
        if needs_selection && (config.capture.snap_to_windows || mode == CaptureMode::Window) {
            backend.windows().unwrap_or_else(|e| {
                // Window enumeration is a nicety; losing it must not stop a
                // region capture.
                log::warn!("could not enumerate windows ({e}); snapping disabled");
                Vec::new()
            })
        } else {
            Vec::new()
        };

    Ok(Acquired {
        image,
        windows,
        origin: frame.origin,
        needs_selection,
        mode,
    })
}

/// Whether this session's capture backend can supply a cursor bitmap, so the
/// UI can offer `include-cursor` only where it does something.
///
/// Cached for the life of the process: the answer depends on which backend the
/// session selected and, on X11, whether the server has XFixes — neither of
/// which changes while bettershot is running. Constructing a backend to ask is
/// cheap (the portal's is a unit struct; X11's opens a local socket) and
/// involves no capture and no permission prompt.
pub fn cursor_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        bettershot_capture::default_backend()
            .map(|backend| backend.capabilities().cursor)
            .unwrap_or(false)
    })
}

fn primary_monitor_target(backend: &dyn CaptureBackend) -> CaptureTarget {
    match backend.monitors() {
        Ok(monitors) => monitors
            .iter()
            .find(|m| m.is_primary)
            .or_else(|| monitors.first())
            .map(|m| CaptureTarget::Monitor(m.id))
            .unwrap_or(CaptureTarget::FullDesktop),
        Err(e) => {
            log::warn!("could not enumerate monitors ({e}); falling back to the whole desktop");
            CaptureTarget::FullDesktop
        }
    }
}

pub fn frame_to_image(frame: &RawFrame) -> Result<RgbaImage> {
    RgbaImage::from_raw(frame.width, frame.height, frame.data.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "the capture backend returned {} bytes for a {}×{} frame",
            frame.data.len(),
            frame.width,
            frame.height
        )
    })
}

pub fn non_empty(image: RgbaImage) -> Result<RgbaImage> {
    if image.width() == 0 || image.height() == 0 {
        bail!("the image is empty");
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_image_is_rejected() {
        assert!(non_empty(RgbaImage::new(0, 0)).is_err());
        assert!(non_empty(RgbaImage::new(10, 0)).is_err());
        assert!(non_empty(RgbaImage::new(1, 1)).is_ok());
    }

    #[test]
    fn a_frame_with_a_mismatched_buffer_is_reported_not_silently_accepted() {
        let frame = RawFrame {
            width: 10,
            height: 10,
            // One pixel short of 10x10 RGBA.
            data: vec![0; 10 * 10 * 4 - 4],
            origin: Vec2D::ZERO,
            scale_factor: 1.0,
        };
        let err = frame_to_image(&frame).unwrap_err().to_string();
        assert!(err.contains("396"), "{err}");
        assert!(err.contains("10×10"), "{err}");
    }

    #[test]
    fn a_well_formed_frame_converts() {
        let frame = RawFrame {
            width: 4,
            height: 3,
            data: vec![7; 4 * 3 * 4],
            origin: Vec2D::new(-1920.0, 0.0),
            scale_factor: 2.0,
        };
        let image = frame_to_image(&frame).expect("should convert");
        assert_eq!(image.dimensions(), (4, 3));
        assert_eq!(image.get_pixel(0, 0).0, [7, 7, 7, 7]);
    }
}
