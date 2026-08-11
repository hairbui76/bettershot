//! The backend trait, the factory that picks one, and the delayed-capture
//! helper.

use std::time::Duration;

use crate::{
    BackendChoice, BackendSelection, Capabilities, CaptureError, CaptureTarget, CursorImage,
    Environment, MonitorInfo, RawFrame, VirtualDesktop, WindowInfo, backends, composite_cursor,
    select_backend,
};

/// One way of getting pixels off the screen.
///
/// Implementations are expected to be cheap to hold and safe to keep across
/// captures; the display configuration may change underneath them, which is why
/// every method re-queries rather than caching.
///
/// `Send` (but deliberately not `Sync`): the app shell moves a backend onto a
/// worker thread for the capture, and several platform handles are not safe to
/// share between threads.
pub trait CaptureBackend: Send {
    /// Stable identifier for logs, config and error messages, e.g. `x11`.
    fn name(&self) -> &'static str;

    /// Every connected monitor, in physical pixels on the virtual desktop.
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;

    /// Every capturable top-level window, `z_order` 0 = frontmost.
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;

    /// Grab pixels. See [`CaptureTarget`] for how each target is interpreted.
    fn capture(&self, target: CaptureTarget) -> Result<RawFrame, CaptureError>;

    /// Which targets this backend can actually serve.
    fn capabilities(&self) -> Capabilities;

    /// The cursor as it looks right now, for `--include-cursor`.
    ///
    /// Separate from [`CaptureBackend::capture`] because the cursor is not in
    /// the captured pixels on any platform — compositors draw it on its own
    /// plane — so including it is always a second query plus a blend, and the
    /// blend ([`crate::composite_cursor`]) is platform-neutral.
    ///
    /// `Ok(None)` means "nothing to draw": the pointer is hidden, or has left
    /// the screen. Backends that cannot ask at all keep this default and
    /// report `cursor: false` in their [`Capabilities`].
    fn cursor(&self) -> Result<Option<CursorImage>, CaptureError> {
        Ok(None)
    }

    /// The monitors wrapped in the layout helper. Provided so callers do not
    /// re-implement it.
    fn virtual_desktop(&self) -> Result<VirtualDesktop, CaptureError> {
        Ok(VirtualDesktop::new(self.monitors()?))
    }
}

/// The backend for the current OS and session.
///
/// Equivalent to `backend_for(&select_backend(&Environment::detect())?)`.
///
/// Errors with [`CaptureError::NoDisplay`] when the process is not attached to
/// a graphical session, and [`CaptureError::Unsupported`] on platforms with no
/// implementation.
pub fn default_backend() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    let environment = Environment::detect();
    let selection = select_backend(&environment)?;
    log::debug!("capture backend: {selection}");
    backend_for(&selection)
}

/// Instantiate a specific backend.
///
/// Fails with [`CaptureError::Unsupported`] when the choice does not exist on
/// the OS this binary was built for — asking for the X11 backend in a Windows
/// build, for instance.
pub fn backend_for(selection: &BackendSelection) -> Result<Box<dyn CaptureBackend>, CaptureError> {
    match selection.choice {
        BackendChoice::WaylandPortal => backends::new_wayland_portal(),
        BackendChoice::X11 => backends::new_x11(),
        BackendChoice::WindowsGraphicsCapture => backends::new_windows(),
        BackendChoice::MacOsScreenCaptureKit => backends::new_macos(),
    }
}

/// Capture after waiting `delay`.
///
/// Deliberately a free function rather than a trait method: the sleep is policy
/// (the `--delay` flag), not a backend concern, and keeping it out of the trait
/// means backends stay synchronous and stubbable. A zero delay skips the sleep
/// entirely.
///
/// The wait is a plain blocking sleep — the caller decides whether that happens
/// on a worker thread or blocks the UI.
pub fn capture_after(
    backend: &dyn CaptureBackend,
    target: CaptureTarget,
    delay: Duration,
) -> Result<RawFrame, CaptureError> {
    if !delay.is_zero() {
        log::debug!("delaying capture by {delay:?}");
        std::thread::sleep(delay);
    }
    backend.capture(target)
}

/// [`capture_after`], then blend the pointer in when `include_cursor` is set.
///
/// The cursor is sampled *after* the delay and the grab, which is as close to
/// the captured instant as a two-call platform API allows.
pub fn capture_after_including_cursor(
    backend: &dyn CaptureBackend,
    target: CaptureTarget,
    delay: Duration,
    include_cursor: bool,
) -> Result<RawFrame, CaptureError> {
    let mut frame = capture_after(backend, target, delay)?;
    if include_cursor {
        draw_cursor_into(backend, &mut frame);
    }
    Ok(frame)
}

/// Blend the backend's current cursor into `frame`.
///
/// Deliberately infallible: the user already has their screenshot by this
/// point, and a cursor that could not be read is a cosmetic loss, not a reason
/// to throw the pixels away. Failures are logged and the frame is left alone.
pub fn draw_cursor_into(backend: &dyn CaptureBackend, frame: &mut RawFrame) {
    match backend.cursor() {
        Ok(Some(cursor)) => {
            log::debug!(
                "drawing the {}x{} cursor at {:?}",
                cursor.width,
                cursor.height,
                cursor.position
            );
            composite_cursor(frame, &cursor);
        }
        Ok(None) => log::debug!("the {} backend reports no visible cursor", backend.name()),
        Err(e) => {
            log::warn!("could not read the cursor ({e}); the screenshot will not show it");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use bettershot_core::{Rect, Vec2D};

    use super::*;
    use crate::MonitorId;

    /// What a [`FakeBackend`] should do when asked for the cursor.
    #[derive(Clone, Copy, PartialEq)]
    enum Cursor {
        /// No pointer on screen.
        Hidden,
        /// An opaque 1x1 red pointer at the frame's top-left.
        Red,
        /// The platform query failed.
        Fails,
    }

    /// A backend that records how often it was asked to capture, so the delay
    /// helper can be tested without a display.
    struct FakeBackend {
        captures: AtomicUsize,
        cursor: Cursor,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                captures: AtomicUsize::new(0),
                cursor: Cursor::Hidden,
            }
        }

        fn with_cursor(cursor: Cursor) -> Self {
            Self {
                captures: AtomicUsize::new(0),
                cursor,
            }
        }
    }

    impl CaptureBackend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
            Ok(vec![MonitorInfo::new(
                MonitorId::new(1),
                "fake-0",
                Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
                1.0,
                true,
            )])
        }

        fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
            Ok(Vec::new())
        }

        fn capture(&self, _target: CaptureTarget) -> Result<RawFrame, CaptureError> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            RawFrame::filled(2, 2, [0, 0, 0, 255], Vec2D::ZERO, 1.0)
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::FULL
        }

        fn cursor(&self) -> Result<Option<CursorImage>, CaptureError> {
            match self.cursor {
                Cursor::Hidden => Ok(None),
                Cursor::Red => Ok(Some(
                    CursorImage::from_premultiplied(
                        1,
                        1,
                        vec![255, 0, 0, 255],
                        crate::CursorAnchor::TopLeft(Vec2D::ZERO),
                    )
                    .unwrap(),
                )),
                Cursor::Fails => Err(CaptureError::unsupported("no cursor here")),
            }
        }
    }

    #[test]
    fn capture_after_zero_delay_captures_immediately() {
        let backend = FakeBackend::new();
        let started = Instant::now();
        let frame = capture_after(&backend, CaptureTarget::FullDesktop, Duration::ZERO).unwrap();
        assert_eq!(frame.width, 2);
        assert_eq!(backend.captures.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn capture_after_waits_before_capturing() {
        let backend = FakeBackend::new();
        let started = Instant::now();
        capture_after(
            &backend,
            CaptureTarget::FullDesktop,
            Duration::from_millis(30),
        )
        .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert_eq!(backend.captures.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn virtual_desktop_is_derived_from_monitors() {
        let desktop = FakeBackend::new().virtual_desktop().unwrap();
        assert_eq!(desktop.len(), 1);
        assert_eq!(desktop.bounds(), Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn the_cursor_is_only_drawn_when_it_is_asked_for() {
        let backend = FakeBackend::with_cursor(Cursor::Red);

        let without = capture_after_including_cursor(
            &backend,
            CaptureTarget::FullDesktop,
            Duration::ZERO,
            false,
        )
        .unwrap();
        assert_eq!(without.pixel(0, 0), Some([0, 0, 0, 255]));

        let with = capture_after_including_cursor(
            &backend,
            CaptureTarget::FullDesktop,
            Duration::ZERO,
            true,
        )
        .unwrap();
        assert_eq!(with.pixel(0, 0), Some([255, 0, 0, 255]));
        // Only the pixel under the cursor changes.
        assert_eq!(with.pixel(1, 1), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_hidden_cursor_leaves_the_frame_alone() {
        let frame = capture_after_including_cursor(
            &FakeBackend::with_cursor(Cursor::Hidden),
            CaptureTarget::FullDesktop,
            Duration::ZERO,
            true,
        )
        .unwrap();
        assert_eq!(frame.pixel(0, 0), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_cursor_query_that_fails_still_yields_the_screenshot() {
        // Losing the pointer is cosmetic; losing the capture is not.
        let frame = capture_after_including_cursor(
            &FakeBackend::with_cursor(Cursor::Fails),
            CaptureTarget::FullDesktop,
            Duration::ZERO,
            true,
        )
        .unwrap();
        assert_eq!(frame.width, 2);
        assert_eq!(frame.pixel(0, 0), Some([0, 0, 0, 255]));
    }

    #[test]
    fn backends_default_to_having_no_cursor_to_offer() {
        // A backend that does not override `cursor()` must not claim one.
        struct Bare;
        impl CaptureBackend for Bare {
            fn name(&self) -> &'static str {
                "bare"
            }
            fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
                Ok(Vec::new())
            }
            fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
                Ok(Vec::new())
            }
            fn capture(&self, _: CaptureTarget) -> Result<RawFrame, CaptureError> {
                Err(CaptureError::NoDisplay)
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::NONE
            }
        }
        assert!(Bare.cursor().unwrap().is_none());
        assert!(!Bare.capabilities().cursor);
    }

    #[test]
    fn backends_are_object_safe_and_boxable() {
        let boxed: Box<dyn CaptureBackend> = Box::new(FakeBackend::new());
        assert_eq!(boxed.name(), "fake");
        assert!(boxed.capabilities().window);
    }
}
