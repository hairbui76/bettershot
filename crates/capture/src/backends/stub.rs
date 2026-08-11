//! A backend that politely refuses everything.
//!
//! Stands in for the macOS ScreenCaptureKit backend wherever that backend
//! cannot exist: on a non-macOS build, and on a macOS older than 12.3 where the
//! framework is missing. Having a real object rather than an early `Err` means
//! the app shell can still construct a backend, show its name and render a
//! "not supported here" panel with the same code path it uses for every other
//! failure — and it means that refusal behaviour stays testable on Linux and
//! Windows build machines.

use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureTarget, MonitorInfo, RawFrame, WindowInfo,
};

/// A [`CaptureBackend`] whose every operation fails with
/// [`CaptureError::Unsupported`].
pub(crate) struct UnsupportedBackend {
    name: &'static str,
    reason: String,
}

impl UnsupportedBackend {
    pub(crate) fn new(name: &'static str, reason: impl Into<String>) -> Self {
        Self {
            name,
            reason: reason.into(),
        }
    }

    fn refuse<T>(&self) -> Result<T, CaptureError> {
        Err(CaptureError::unsupported(self.reason.clone()))
    }
}

impl CaptureBackend for UnsupportedBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        self.refuse()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        self.refuse()
    }

    fn capture(&self, _target: CaptureTarget) -> Result<RawFrame, CaptureError> {
        self.refuse()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::NONE
    }
}

/// The stand-in for the macOS backend, reporting the same name so the app shell
/// and [`crate::BackendChoice`] agree on the backend's identity whether or not
/// ScreenCaptureKit is actually reachable.
pub(crate) fn macos_unavailable(reason: impl Into<String>) -> UnsupportedBackend {
    UnsupportedBackend::new(crate::backends::macos::NAME, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MonitorId, WindowId, backends};
    use bettershot_core::Rect;

    fn macos_stub() -> UnsupportedBackend {
        macos_unavailable("the ScreenCaptureKit backend is only available on macOS 12.3 or newer")
    }

    #[test]
    fn the_macos_stub_refuses_every_operation() {
        let backend = macos_stub();
        assert_eq!(backend.name(), "macos-screencapturekit");
        assert!(matches!(
            backend.monitors(),
            Err(CaptureError::Unsupported(_))
        ));
        assert!(matches!(
            backend.windows(),
            Err(CaptureError::Unsupported(_))
        ));
        for target in [
            CaptureTarget::FullDesktop,
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(1)),
            CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
        ] {
            assert!(matches!(
                backend.capture(target),
                Err(CaptureError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn the_macos_stub_explains_why_it_is_standing_in() {
        let err = macos_stub()
            .capture(CaptureTarget::FullDesktop)
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("ScreenCaptureKit"), "{text}");
        assert!(text.contains("12.3"), "{text}");
    }

    #[test]
    fn the_macos_stub_advertises_no_capabilities() {
        let caps = macos_stub().capabilities();
        assert_eq!(caps, Capabilities::NONE);
        assert!(!caps.supports(&CaptureTarget::FullDesktop));
    }

    #[test]
    fn the_macos_backend_is_constructible_on_any_host() {
        // Not cfg-gated: `new_macos` always yields *something* that reports the
        // macOS backend's name, whether that is the real ScreenCaptureKit
        // backend or this placeholder.
        let boxed = backends::new_macos().unwrap();
        assert_eq!(boxed.name(), "macos-screencapturekit");
    }

    #[test]
    fn backends_absent_from_this_target_report_unsupported() {
        #[cfg(not(target_os = "linux"))]
        {
            assert!(matches!(
                backends::new_x11().err(),
                Some(CaptureError::Unsupported(_))
            ));
            assert!(matches!(
                backends::new_wayland_portal().err(),
                Some(CaptureError::Unsupported(_))
            ));
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(matches!(
                backends::new_windows().err(),
                Some(CaptureError::Unsupported(_))
            ));
        }
    }
}
