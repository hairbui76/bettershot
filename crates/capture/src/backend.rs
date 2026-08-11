//! The backend trait, the factory that picks one, and the delayed-capture
//! helper.

use std::time::Duration;

use crate::{
    BackendChoice, BackendSelection, Capabilities, CaptureError, CaptureTarget, Environment,
    MonitorInfo, RawFrame, VirtualDesktop, WindowInfo, backends, select_backend,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use bettershot_core::{Rect, Vec2D};

    use super::*;
    use crate::MonitorId;

    /// A backend that records how often it was asked to capture, so the delay
    /// helper can be tested without a display.
    struct FakeBackend {
        captures: AtomicUsize,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                captures: AtomicUsize::new(0),
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
            RawFrame::filled(2, 2, [1, 2, 3, 4], Vec2D::ZERO, 1.0)
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::FULL
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
    fn backends_are_object_safe_and_boxable() {
        let boxed: Box<dyn CaptureBackend> = Box::new(FakeBackend::new());
        assert_eq!(boxed.name(), "fake");
        assert!(boxed.capabilities().window);
    }
}
