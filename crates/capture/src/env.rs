//! Deciding which backend to use, without touching the environment.
//!
//! Session sniffing on Linux is genuinely messy: `XDG_SESSION_TYPE` lies under
//! XWayland, `DISPLAY` is set inside a Wayland session, and both can be empty
//! strings rather than absent. So the sniffing is split in two:
//! [`Environment::detect`] reads the process environment once, and
//! [`select_backend`] is a pure function over the result — which is the part
//! that can be tested on a headless machine.

use std::env;
use std::fmt;

use crate::CaptureError;

/// The operating system a backend would run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetOs {
    /// Linux (Wayland or X11).
    Linux,
    /// Windows 10 1903+ / 11.
    Windows,
    /// macOS 12.3+ (ScreenCaptureKit); screenshots need 14.0+.
    MacOs,
    /// Anything else: BSDs, unknown targets.
    Other,
}

impl TargetOs {
    /// The OS this binary was compiled for.
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            Self::Other
        }
    }
}

/// The bits of the process environment that decide the backend.
///
/// Empty strings are normalised to `None` on construction: a shell that exports
/// `DISPLAY=` has no display, and treating `Some("")` as "X11 is available" is a
/// classic source of confusing failures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// Which OS the capture would happen on.
    pub target_os: TargetOs,
    /// `XDG_SESSION_TYPE`: `wayland`, `x11`, `tty`, ...
    pub session_type: Option<String>,
    /// `WAYLAND_DISPLAY`, e.g. `wayland-0`.
    pub wayland_display: Option<String>,
    /// `DISPLAY`, e.g. `:0`.
    pub display: Option<String>,
}

impl Default for TargetOs {
    fn default() -> Self {
        Self::current()
    }
}

impl Environment {
    /// Read the real process environment.
    pub fn detect() -> Self {
        Self {
            target_os: TargetOs::current(),
            session_type: non_empty(env::var("XDG_SESSION_TYPE").ok()),
            wayland_display: non_empty(env::var("WAYLAND_DISPLAY").ok()),
            display: non_empty(env::var("DISPLAY").ok()),
        }
    }

    /// Build an environment explicitly. Empty strings become `None`.
    pub fn new(
        target_os: TargetOs,
        session_type: Option<&str>,
        wayland_display: Option<&str>,
        display: Option<&str>,
    ) -> Self {
        Self {
            target_os,
            session_type: non_empty(session_type.map(str::to_owned)),
            wayland_display: non_empty(wayland_display.map(str::to_owned)),
            display: non_empty(display.map(str::to_owned)),
        }
    }

    /// `XDG_SESSION_TYPE` lowercased and trimmed, for matching.
    fn session(&self) -> Option<String> {
        self.session_type
            .as_ref()
            .map(|s| s.trim().to_ascii_lowercase())
    }

    fn says_wayland(&self) -> bool {
        self.session().as_deref() == Some("wayland")
    }

    fn says_x11(&self) -> bool {
        matches!(self.session().as_deref(), Some("x11") | Some("xorg"))
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Which backend implementation to instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendChoice {
    /// `xdg-desktop-portal`'s Screenshot interface over D-Bus (Wayland).
    WaylandPortal,
    /// Direct X11 grab via RandR + `GetImage`.
    X11,
    /// Windows Graphics Capture / DXGI, via `xcap`.
    WindowsGraphicsCapture,
    /// macOS ScreenCaptureKit: `SCShareableContent` for enumeration and
    /// `SCScreenshotManager` for the grab.
    MacOsScreenCaptureKit,
}

impl BackendChoice {
    /// Stable identifier, matching [`crate::CaptureBackend::name`].
    pub const fn name(self) -> &'static str {
        match self {
            Self::WaylandPortal => "wayland-portal",
            Self::X11 => "x11",
            Self::WindowsGraphicsCapture => "windows-graphics-capture",
            Self::MacOsScreenCaptureKit => "macos-screencapturekit",
        }
    }
}

impl fmt::Display for BackendChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A backend choice plus the reasoning, so `--verbose` can explain itself and
/// bug reports arrive with the session details already in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSelection {
    /// The chosen backend.
    pub choice: BackendChoice,
    /// Why, in one human-readable sentence.
    pub reason: String,
}

impl BackendSelection {
    fn new(choice: BackendChoice, reason: impl Into<String>) -> Self {
        Self {
            choice,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for BackendSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.choice, self.reason)
    }
}

/// Pick a backend for `env`.
///
/// The Linux precedence, highest first:
///
/// 1. `XDG_SESSION_TYPE=wayland` **and** `WAYLAND_DISPLAY` set — a Wayland
///    session that agrees with itself.
/// 2. `XDG_SESSION_TYPE=x11` **and** `DISPLAY` set — an explicit X11 session.
///    This outranks a stray `WAYLAND_DISPLAY`, which some login managers leak
///    into X sessions.
/// 3. `WAYLAND_DISPLAY` set — a Wayland compositor is reachable even if the
///    session type is missing or wrong (common under XWayland, where `DISPLAY`
///    is also set; the portal gives the better result there).
/// 4. `DISPLAY` set — X11 or XWayland.
/// 5. `XDG_SESSION_TYPE=wayland` alone — the Screenshot portal is pure D-Bus,
///    so it can still work without a Wayland socket in this process.
/// 6. Otherwise [`CaptureError::NoDisplay`].
pub fn select_backend(env: &Environment) -> Result<BackendSelection, CaptureError> {
    match env.target_os {
        TargetOs::Windows => Ok(BackendSelection::new(
            BackendChoice::WindowsGraphicsCapture,
            "Windows uses Windows Graphics Capture / DXGI",
        )),
        TargetOs::MacOs => Ok(BackendSelection::new(
            BackendChoice::MacOsScreenCaptureKit,
            "macOS uses ScreenCaptureKit (needs macOS 12.3+, and 14.0+ to capture)",
        )),
        TargetOs::Other => Err(CaptureError::unsupported(
            "bettershot has no capture backend for this operating system",
        )),
        TargetOs::Linux => select_linux_backend(env),
    }
}

fn select_linux_backend(env: &Environment) -> Result<BackendSelection, CaptureError> {
    let has_wayland = env.wayland_display.is_some();
    let has_x11 = env.display.is_some();

    if env.says_wayland() && has_wayland {
        return Ok(BackendSelection::new(
            BackendChoice::WaylandPortal,
            "XDG_SESSION_TYPE=wayland and WAYLAND_DISPLAY is set",
        ));
    }
    if env.says_x11() && has_x11 {
        return Ok(BackendSelection::new(
            BackendChoice::X11,
            "XDG_SESSION_TYPE=x11 and DISPLAY is set",
        ));
    }
    if has_wayland {
        return Ok(BackendSelection::new(
            BackendChoice::WaylandPortal,
            "WAYLAND_DISPLAY is set, so a Wayland compositor is reachable",
        ));
    }
    if has_x11 {
        return Ok(BackendSelection::new(
            BackendChoice::X11,
            "DISPLAY is set and no Wayland compositor was found",
        ));
    }
    if env.says_wayland() {
        return Ok(BackendSelection::new(
            BackendChoice::WaylandPortal,
            "XDG_SESSION_TYPE=wayland; the screenshot portal is reachable over D-Bus alone",
        ));
    }
    Err(CaptureError::NoDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(session: Option<&str>, wayland: Option<&str>, x11: Option<&str>) -> Environment {
        Environment::new(TargetOs::Linux, session, wayland, x11)
    }

    #[test]
    fn windows_always_picks_graphics_capture() {
        let selection =
            select_backend(&Environment::new(TargetOs::Windows, None, None, None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WindowsGraphicsCapture);
        assert_eq!(selection.choice.name(), "windows-graphics-capture");
    }

    #[test]
    fn macos_always_picks_screencapturekit() {
        let selection =
            select_backend(&Environment::new(TargetOs::MacOs, None, None, None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::MacOsScreenCaptureKit);
        assert_eq!(selection.choice.name(), "macos-screencapturekit");
        assert!(selection.reason.contains("ScreenCaptureKit"));
        // The version floors belong in the reason so `--verbose` and bug
        // reports carry them.
        assert!(selection.reason.contains("12.3"), "{}", selection.reason);
        assert!(selection.reason.contains("14.0"), "{}", selection.reason);
    }

    #[test]
    fn macos_ignores_the_linux_session_variables_entirely() {
        // A Mac with DISPLAY set (XQuartz) must not fall through to X11.
        let selection = select_backend(&Environment::new(
            TargetOs::MacOs,
            Some("x11"),
            Some("wayland-0"),
            Some(":0"),
        ))
        .unwrap();
        assert_eq!(selection.choice, BackendChoice::MacOsScreenCaptureKit);
    }

    #[test]
    fn unknown_operating_systems_are_unsupported() {
        assert!(matches!(
            select_backend(&Environment::new(TargetOs::Other, None, None, None)),
            Err(CaptureError::Unsupported(_))
        ));
    }

    #[test]
    fn wayland_session_picks_the_portal() {
        let selection = select_backend(&linux(Some("wayland"), Some("wayland-0"), None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
    }

    #[test]
    fn x11_session_picks_the_x11_backend() {
        let selection = select_backend(&linux(Some("x11"), None, Some(":0"))).unwrap();
        assert_eq!(selection.choice, BackendChoice::X11);
    }

    #[test]
    fn xorg_is_accepted_as_a_spelling_of_x11() {
        let selection = select_backend(&linux(Some("Xorg"), None, Some(":0"))).unwrap();
        assert_eq!(selection.choice, BackendChoice::X11);
    }

    #[test]
    fn session_type_is_matched_case_insensitively_and_trimmed() {
        let selection =
            select_backend(&linux(Some("  WAYLAND \n"), Some("wayland-1"), None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
    }

    #[test]
    fn under_xwayland_the_portal_wins_over_display() {
        // A Wayland session with XWayland: both variables are set.
        let selection =
            select_backend(&linux(Some("wayland"), Some("wayland-0"), Some(":0"))).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
    }

    #[test]
    fn an_explicit_x11_session_outranks_a_leaked_wayland_display() {
        let selection = select_backend(&linux(Some("x11"), Some("wayland-0"), Some(":0"))).unwrap();
        assert_eq!(selection.choice, BackendChoice::X11);
        assert!(selection.reason.contains("XDG_SESSION_TYPE=x11"));
    }

    #[test]
    fn a_wayland_display_alone_is_enough_for_the_portal() {
        let selection = select_backend(&linux(None, Some("wayland-0"), None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
        let selection = select_backend(&linux(Some("tty"), Some("wayland-0"), None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
    }

    #[test]
    fn a_display_alone_is_enough_for_x11() {
        let selection = select_backend(&linux(None, None, Some(":1"))).unwrap();
        assert_eq!(selection.choice, BackendChoice::X11);
    }

    #[test]
    fn a_wayland_session_type_alone_still_reaches_the_portal_over_dbus() {
        let selection = select_backend(&linux(Some("wayland"), None, None)).unwrap();
        assert_eq!(selection.choice, BackendChoice::WaylandPortal);
        assert!(selection.reason.contains("D-Bus"));
    }

    #[test]
    fn an_x11_session_type_without_display_is_no_display() {
        // Nothing to connect to: claiming X11 here would only fail later with a
        // worse message.
        assert!(matches!(
            select_backend(&linux(Some("x11"), None, None)),
            Err(CaptureError::NoDisplay)
        ));
    }

    #[test]
    fn a_tty_session_with_nothing_set_is_no_display() {
        assert!(matches!(
            select_backend(&linux(Some("tty"), None, None)),
            Err(CaptureError::NoDisplay)
        ));
        assert!(matches!(
            select_backend(&linux(None, None, None)),
            Err(CaptureError::NoDisplay)
        ));
    }

    #[test]
    fn empty_env_vars_count_as_unset() {
        let env = linux(Some("  "), Some(""), Some(""));
        assert_eq!(env.session_type, None);
        assert_eq!(env.wayland_display, None);
        assert_eq!(env.display, None);
        assert!(matches!(select_backend(&env), Err(CaptureError::NoDisplay)));
    }

    #[test]
    fn selection_renders_the_reason_for_logs() {
        let selection = select_backend(&linux(None, None, Some(":0"))).unwrap();
        let text = selection.to_string();
        assert!(text.starts_with("x11 ("), "{text}");
        assert!(text.contains("DISPLAY"), "{text}");
    }

    #[test]
    fn detect_reads_the_real_environment_without_panicking() {
        // The build machine may be a bare TTY; all we assert is that detection
        // agrees with the compile target and does not blow up.
        let env = Environment::detect();
        assert_eq!(env.target_os, TargetOs::current());
    }
}
