//! The one error type every capture backend speaks.
//!
//! The variants exist so the app shell can react differently rather than just
//! print a string: [`CaptureError::Cancelled`] is a normal user action and must
//! not surface as a failure, [`CaptureError::PermissionDenied`] needs guidance
//! ("allow screen capture in ..."), and [`CaptureError::NoDisplay`] means the
//! process is not attached to a graphical session at all.

use crate::{MonitorId, WindowId};

/// Everything that can go wrong while enumerating or capturing.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The platform, session or backend cannot do what was asked, and no amount
    /// of retrying or permission granting will change that.
    #[error("screen capture is not supported here: {0}")]
    Unsupported(String),

    /// The OS or compositor refused access. On Wayland this is a denied portal
    /// permission; on macOS it will be the Screen Recording TCC entitlement.
    #[error("screen capture permission was denied: {0}")]
    PermissionDenied(String),

    /// The user dismissed the portal / picker dialog. Not a failure: callers
    /// should exit quietly rather than show an error.
    #[error("the capture was cancelled")]
    Cancelled,

    /// A [`MonitorId`] that no longer (or never did) exist. Monitor ids are
    /// only valid until the display configuration changes.
    #[error("no such monitor: {0}")]
    NoSuchMonitor(MonitorId),

    /// A [`WindowId`] that no longer (or never did) exist. Windows disappear
    /// between enumeration and capture all the time.
    #[error("no such window: {0}")]
    NoSuchWindow(WindowId),

    /// The requested region was degenerate, or fell entirely outside the
    /// monitor / virtual desktop it was clamped against.
    #[error("the requested region is empty or lies outside the capture area")]
    EmptyRegion,

    /// No graphical session was found: neither `WAYLAND_DISPLAY` nor `DISPLAY`
    /// is usable. Typical for ssh sessions, TTYs and CI containers.
    #[error(
        "no display server found (neither WAYLAND_DISPLAY nor DISPLAY is set); \
         bettershot needs a graphical session to capture the screen"
    )]
    NoDisplay,

    /// A frame's dimensions and buffer length disagree, or the frame is too
    /// large to address. Indicates a backend bug or a hostile input.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// Anything the underlying platform API reported that does not map onto a
    /// more specific variant.
    #[error("capture backend error: {0}")]
    Backend(String),
}

impl CaptureError {
    /// Shorthand for [`CaptureError::Unsupported`].
    pub fn unsupported(what: impl Into<String>) -> Self {
        Self::Unsupported(what.into())
    }

    /// Shorthand for [`CaptureError::Backend`].
    pub fn backend(what: impl Into<String>) -> Self {
        Self::Backend(what.into())
    }

    /// Shorthand for [`CaptureError::PermissionDenied`].
    pub fn permission_denied(what: impl Into<String>) -> Self {
        Self::PermissionDenied(what.into())
    }

    /// Shorthand for [`CaptureError::InvalidFrame`].
    pub fn invalid_frame(what: impl Into<String>) -> Self {
        Self::InvalidFrame(what.into())
    }

    /// The user pressed "Cancel". Callers should exit with status 0 and stay
    /// silent instead of reporting an error.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Access was refused and the user has to change a system setting.
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::PermissionDenied(_))
    }

    /// Whether a retry could plausibly succeed without the user changing
    /// anything. Enumeration races (a window closing mid-capture) are retryable;
    /// a missing display server or a denied permission is not.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::NoSuchMonitor(_) | Self::NoSuchWindow(_))
    }

    /// A single actionable sentence for the UI, or `None` when the message
    /// itself is all there is to say.
    pub fn guidance(&self) -> Option<&'static str> {
        match self {
            Self::PermissionDenied(_) => Some(
                "Allow screen capture for bettershot in your system's privacy settings \
                 (GNOME/KDE: Settings -> Privacy -> Screen Sharing; macOS: System Settings -> \
                 Privacy & Security -> Screen & System Audio Recording, then quit and \
                 relaunch bettershot), and try again.",
            ),
            Self::NoDisplay => Some(
                "Start bettershot from a graphical session, or forward a display \
                 (WAYLAND_DISPLAY / DISPLAY) into this environment.",
            ),
            Self::EmptyRegion => {
                Some("Select a region that overlaps a monitor and is at least one pixel wide.")
            }
            Self::NoSuchMonitor(_) | Self::NoSuchWindow(_) => Some(
                "The display configuration changed since the last enumeration; \
                 re-run the capture.",
            ),
            Self::Unsupported(_) => Some(
                "Try a different capture mode, or see the bettershot roadmap for platform support.",
            ),
            Self::Cancelled | Self::InvalidFrame(_) | Self::Backend(_) => None,
        }
    }
}

/// Map a D-Bus error name (and message) from `xdg-desktop-portal` onto a
/// [`CaptureError`].
///
/// Kept as a pure string-in/error-out function so the mapping can be unit
/// tested on machines with no portal — constructing real `ashpd` errors is not
/// practical.
pub fn classify_portal_error(error_name: &str, message: &str) -> CaptureError {
    // Portal backends are inconsistent about which name they use, so match on
    // the last dot-separated component rather than the full interface path.
    let short = error_name.rsplit('.').next().unwrap_or(error_name);
    match short {
        "Cancelled" | "Canceled" => CaptureError::Cancelled,
        "NotAllowed" | "AccessDenied" | "PermissionDenied" => CaptureError::PermissionDenied(
            format!("the screenshot portal refused access: {message}"),
        ),
        "ServiceUnknown" | "PortalNotFound" | "UnknownMethod" | "NameHasNoOwner" => {
            CaptureError::Unsupported(format!(
                "no xdg-desktop-portal screenshot backend is running: {message}"
            ))
        }
        _ => CaptureError::Backend(format!("{error_name}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_is_recognised_and_carries_no_guidance() {
        let err = CaptureError::Cancelled;
        assert!(err.is_cancelled());
        assert!(!err.is_permission_denied());
        assert!(err.guidance().is_none());
    }

    #[test]
    fn permission_denied_offers_guidance() {
        let err = CaptureError::permission_denied("portal said no");
        assert!(err.is_permission_denied());
        assert!(err.guidance().unwrap().contains("Screen"));
        assert!(err.to_string().contains("portal said no"));
    }

    #[test]
    fn no_display_message_names_both_env_vars() {
        let text = CaptureError::NoDisplay.to_string();
        assert!(text.contains("WAYLAND_DISPLAY"));
        assert!(text.contains("DISPLAY"));
    }

    #[test]
    fn stale_ids_are_retryable_but_permission_is_not() {
        assert!(CaptureError::NoSuchMonitor(MonitorId::new(7)).is_retryable());
        assert!(CaptureError::NoSuchWindow(WindowId::new(7)).is_retryable());
        assert!(!CaptureError::NoDisplay.is_retryable());
        assert!(!CaptureError::permission_denied("x").is_retryable());
    }

    #[test]
    fn missing_ids_are_named_in_the_message() {
        assert_eq!(
            CaptureError::NoSuchMonitor(MonitorId::new(42)).to_string(),
            "no such monitor: monitor#42"
        );
        assert_eq!(
            CaptureError::NoSuchWindow(WindowId::new(3)).to_string(),
            "no such window: window#3"
        );
    }

    #[test]
    fn portal_cancellation_maps_to_cancelled() {
        assert!(
            classify_portal_error(
                "org.freedesktop.portal.Error.Cancelled",
                "user closed dialog"
            )
            .is_cancelled()
        );
        assert!(classify_portal_error("Canceled", "").is_cancelled());
    }

    #[test]
    fn portal_denial_maps_to_permission_denied() {
        for name in [
            "org.freedesktop.portal.Error.NotAllowed",
            "org.freedesktop.DBus.Error.AccessDenied",
            "PermissionDenied",
        ] {
            assert!(
                classify_portal_error(name, "nope").is_permission_denied(),
                "{name} should map to PermissionDenied"
            );
        }
    }

    #[test]
    fn missing_portal_service_maps_to_unsupported() {
        let err =
            classify_portal_error("org.freedesktop.DBus.Error.ServiceUnknown", "no such name");
        assert!(matches!(err, CaptureError::Unsupported(_)));
    }

    #[test]
    fn unknown_portal_errors_fall_back_to_backend() {
        let err = classify_portal_error("org.example.Weird", "boom");
        assert!(matches!(err, CaptureError::Backend(_)));
        assert!(err.to_string().contains("org.example.Weird"));
        assert!(err.to_string().contains("boom"));
    }
}
