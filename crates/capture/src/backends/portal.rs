//! Linux / Wayland: `xdg-desktop-portal`'s `org.freedesktop.portal.Screenshot`.
//!
//! Wayland compositors do not let a client read other clients' pixels, so the
//! only portable route is the portal: a D-Bus request that the compositor
//! services, possibly after asking the user. This backend therefore behaves
//! quite differently from the X11 and Windows ones, and the differences are
//! reflected honestly in [`Capabilities`]:
//!
//! * **No enumeration.** The Screenshot portal exposes neither monitors nor
//!   windows, so [`CaptureBackend::monitors`] and [`CaptureBackend::windows`]
//!   return [`CaptureError::Unsupported`] rather than inventing geometry. The
//!   app shell is expected to fall back to a single full-desktop frame and do
//!   its region selection inside that image.
//! * **Whole desktop only.** `Screenshot` returns one image of everything.
//!   Region targets are served by cropping that image; monitor and window
//!   targets are refused.
//! * **The origin is assumed to be `(0, 0)`.** The portal reports no geometry
//!   at all. Every mainstream compositor lays its outputs out in non-negative
//!   coordinates, so this holds in practice, but it is an assumption.
//! * **The scale factor is unknown** and reported as `1.0`; the image itself is
//!   in physical pixels, which is what matters for annotation.
//! * **Cancellation is normal.** Dismissing the portal dialog yields
//!   [`CaptureError::Cancelled`], which callers must not treat as a failure.
//!
//! Compositor quirks worth knowing: GNOME services the request itself with no
//! prompt for `interactive: false`; KDE and the wlroots portal may show a
//! picker; some older wlroots builds ignore `interactive` entirely.

use std::path::{Path, PathBuf};

use ashpd::desktop::screenshot::Screenshot;
use bettershot_core::Vec2D;

use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureTarget, MonitorInfo, RawFrame, WindowInfo,
    error::classify_portal_error,
};

/// Screenshot-portal backend. Holds no connection: each capture is a one-shot
/// D-Bus round trip, which is also what keeps it `Send`.
pub(crate) struct PortalBackend;

/// Whether to ask the portal for its own interactive picker.
///
/// Always `false`: bettershot draws its own region-selection overlay on top of
/// a frozen frame, so a second compositor-provided picker would be confusing
/// and, on the compositors that honour it, would make hotkey captures block on
/// a dialog.
const INTERACTIVE: bool = false;

impl PortalBackend {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl CaptureBackend for PortalBackend {
    fn name(&self) -> &'static str {
        "wayland-portal"
    }

    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Err(CaptureError::unsupported(
            "the Wayland screenshot portal does not expose monitor geometry; \
             capture the full desktop and select a region inside it",
        ))
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        Err(CaptureError::unsupported(
            "the Wayland screenshot portal does not expose the window list; \
             window capture is not available on Wayland",
        ))
    }

    fn capture(&self, target: CaptureTarget) -> Result<RawFrame, CaptureError> {
        // Decide what is possible *before* touching D-Bus, so an impossible
        // request fails instantly instead of after a portal round trip (and,
        // with `interactive`, after bothering the user with a dialog).
        let crop_to = match target {
            CaptureTarget::FullDesktop => None,
            CaptureTarget::Region {
                monitor: None,
                rect,
            } => Some(rect),
            CaptureTarget::Monitor(_)
            | CaptureTarget::Window(_)
            | CaptureTarget::Region {
                monitor: Some(_), ..
            } => {
                return Err(CaptureError::unsupported(format!(
                    "the Wayland screenshot portal can only capture the whole desktop, \
                     so a {} target cannot be served; it exposes no monitor or window geometry",
                    target.kind()
                )));
            }
        };

        let frame = self.screenshot()?;
        match crop_to {
            Some(rect) => frame.crop(rect),
            None => Ok(frame),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            full_desktop: true,
            region: true,
            may_prompt_for_permission: true,
            interactive_only: INTERACTIVE,
            ..Capabilities::NONE
        }
    }
}

impl PortalBackend {
    /// One portal round trip: request, read the file it wrote, decode, delete.
    fn screenshot(&self) -> Result<RawFrame, CaptureError> {
        let path = self.request_screenshot_file()?;
        let result = decode_screenshot(&path);
        // The portal writes into our own cache directory and expects us to
        // clean up; a leftover file per capture would be a slow disk leak.
        if let Err(err) = std::fs::remove_file(&path) {
            log::debug!(
                "could not remove portal screenshot {}: {err}",
                path.display()
            );
        }
        result
    }

    fn request_screenshot_file(&self) -> Result<PathBuf, CaptureError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CaptureError::backend(format!("could not start a D-Bus runtime: {e}")))?;

        let uri = runtime.block_on(async move {
            // Check the portal is actually there before asking it for
            // anything. A request to a name nobody owns does not fail — zbus
            // waits for the name to appear, which on a machine with no portal
            // installed means waiting forever. And a plain timeout is the
            // wrong fix: a portal that *is* present legitimately takes as long
            // as the user needs to answer its dialog.
            ensure_portal_present().await?;

            let response = Screenshot::request()
                .interactive(INTERACTIVE)
                .modal(true)
                .send()
                .await
                .map_err(map_ashpd_error)?
                .response()
                .map_err(map_ashpd_error)?;
            Ok::<String, CaptureError>(response.uri().as_str().to_owned())
        })?;

        file_uri_to_path(&uri)
    }
}

/// The bus name every xdg-desktop-portal backend claims.
///
/// Presence has to be tested two ways. The name is D-Bus **activatable**: on an
/// idle desktop nobody owns it until the first request starts the service, so
/// an owner check alone reports "no portal" on a machine that has a perfectly
/// good one. This was caught by running bettershot against a real session where
/// the portal was installed but not yet started.
const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";

/// How long to wait for the bus itself to answer a question about who owns a
/// name. This is a local round trip, so anything beyond a second means the bus
/// is wedged rather than busy.
const BUS_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The error for "there is no portal here", with the guidance a user needs to
/// fix it. Separated out so the wording is testable.
pub(crate) fn portal_missing() -> CaptureError {
    CaptureError::Unsupported(format!(
        "no desktop portal is available ({PORTAL_BUS_NAME} is neither running nor \
         startable on the session bus), so the compositor cannot be asked for a \
         screenshot. Install xdg-desktop-portal plus the backend for your \
         compositor (xdg-desktop-portal-gnome, -kde or -wlr), or annotate an \
         existing image with --filename"
    ))
}

/// Fail fast, and usefully, when no portal is running.
async fn ensure_portal_present() -> Result<(), CaptureError> {
    use ashpd::zbus;

    let query = async {
        let connection = zbus::Connection::session().await.ok()?;
        let dbus = zbus::fdo::DBusProxy::new(&connection).await.ok()?;

        // Already running?
        let name = zbus::names::BusName::try_from(PORTAL_BUS_NAME).ok()?;
        if dbus.name_has_owner(name).await.ok()? {
            return Some(true);
        }

        // Not running is not the same as not installed. The portal is
        // D-Bus-activatable and is normally started on demand by the first
        // request, so a machine with a perfectly good portal reports no owner
        // until something asks for one. Checking only the owner would refuse
        // to capture on most idle desktops.
        let activatable = dbus.list_activatable_names().await.ok()?;
        Some(activatable.iter().any(|n| n.as_str() == PORTAL_BUS_NAME))
    };

    match tokio::time::timeout(BUS_QUERY_TIMEOUT, query).await {
        Ok(Some(true)) => Ok(()),
        Ok(Some(false)) => Err(portal_missing()),
        // No session bus at all, or it did not answer: either way there is no
        // portal to talk to.
        Ok(None) => Err(portal_missing()),
        Err(_) => Err(CaptureError::backend(
            "the session bus did not answer within 2s when asked whether a \
             desktop portal is running",
        )),
    }
}

fn decode_screenshot(path: &Path) -> Result<RawFrame, CaptureError> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| {
            CaptureError::backend(format!(
                "could not open the portal screenshot {}: {e}",
                path.display()
            ))
        })?
        .with_guessed_format()
        .map_err(|e| CaptureError::backend(format!("could not identify the portal image: {e}")))?;
    let image = reader
        .decode()
        .map_err(|e| CaptureError::backend(format!("could not decode the portal image: {e}")))?
        .into_rgba8();
    let (width, height) = (image.width(), image.height());
    // The portal reports no geometry: assume the compositor's outputs start at
    // the origin (see the module docs).
    RawFrame::new(width, height, image.into_raw(), Vec2D::ZERO, 1.0)
}

/// Turn the `file://` URI a portal returns into a path.
///
/// Portals hand back a URI rather than a path, percent-encoded, occasionally
/// with a `localhost` authority. Parsing it is pure string work, so it is
/// separated out and tested rather than buried in the async call.
fn file_uri_to_path(uri: &str) -> Result<PathBuf, CaptureError> {
    let rest = uri.strip_prefix("file://").ok_or_else(|| {
        CaptureError::backend(format!("the portal returned a non-file URI: {uri}"))
    })?;
    // Everything before the first '/' is the authority; only empty and
    // "localhost" refer to this machine.
    let (authority, path) = match rest.find('/') {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return Err(CaptureError::backend(format!(
            "the portal returned a remote file URI: {uri}"
        )));
    }
    let decoded = percent_decode(path)?;
    if decoded.is_empty() || decoded == "/" {
        return Err(CaptureError::backend(format!(
            "the portal returned an empty file URI: {uri}"
        )));
    }
    Ok(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> Result<String, CaptureError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            let value = hex
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| {
                    CaptureError::backend(format!(
                        "malformed percent-escape in portal URI: {input}"
                    ))
                })?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|_| CaptureError::backend(format!("portal URI is not valid UTF-8: {input}")))
}

/// Find something that looks like a D-Bus error name in an error message, so
/// zbus failures can be classified with the same table as portal responses.
fn dbus_error_name(text: &str) -> Option<&str> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.'))
        .find(|token| token.matches('.').count() >= 2 && !token.ends_with('.'))
}

fn map_ashpd_error(err: ashpd::Error) -> CaptureError {
    use ashpd::desktop::ResponseError;

    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureError::Cancelled,
        ashpd::Error::Response(ResponseError::Other) => {
            CaptureError::backend("the screenshot portal reported an unspecified failure")
        }
        ashpd::Error::Portal(portal) => map_portal_error(portal),
        ashpd::Error::PortalNotFound(name) => CaptureError::unsupported(format!(
            "no xdg-desktop-portal implementation provides {name}; \
             install xdg-desktop-portal and a backend for your compositor"
        )),
        ashpd::Error::NoResponse => {
            CaptureError::backend("the screenshot portal closed without responding")
        }
        ashpd::Error::Zbus(inner) => {
            let text = inner.to_string();
            match dbus_error_name(&text) {
                Some(name) => classify_portal_error(name, &text),
                None => CaptureError::backend(format!("D-Bus error talking to the portal: {text}")),
            }
        }
        other => CaptureError::backend(other.to_string()),
    }
}

fn map_portal_error(err: ashpd::PortalError) -> CaptureError {
    use ashpd::PortalError;
    match err {
        PortalError::Cancelled(_) => CaptureError::Cancelled,
        PortalError::NotAllowed(message) => CaptureError::permission_denied(format!(
            "the screenshot portal refused access: {message}"
        )),
        PortalError::WindowDestroyed(message) => CaptureError::backend(format!(
            "the portal dialog was destroyed before responding: {message}"
        )),
        PortalError::Failed(message)
        | PortalError::InvalidArgument(message)
        | PortalError::NotFound(message)
        | PortalError::Exist(message) => CaptureError::backend(message),
        PortalError::ZBus(inner) => CaptureError::backend(inner.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_missing_portal_error_tells_the_user_what_to_install() {
        // Found by running bettershot against a session with no portal: it
        // used to hang forever instead of saying this.
        let message = super::portal_missing().to_string();
        assert!(message.contains("xdg-desktop-portal"), "{message}");
        assert!(
            message.contains("--filename"),
            "offer the fallback: {message}"
        );
        // Must not claim the portal is merely "not running": it is also
        // checked for being startable, and saying otherwise would send people
        // hunting for a service that is supposed to be inactive.
        assert!(message.contains("startable"), "{message}");
    }

    #[test]
    fn the_bus_query_timeout_is_short() {
        // This waits only for the local bus to answer a question about name
        // ownership, never for a user to answer a dialog, so it must not be
        // generous enough to look like a hang.
        assert!(super::BUS_QUERY_TIMEOUT <= std::time::Duration::from_secs(3));
    }

    use super::*;
    use crate::{MonitorId, WindowId};
    use bettershot_core::Rect;

    #[test]
    fn plain_file_uris_become_paths() {
        assert_eq!(
            file_uri_to_path("file:///tmp/shot.png").unwrap(),
            PathBuf::from("/tmp/shot.png")
        );
    }

    #[test]
    fn a_localhost_authority_is_accepted() {
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/shot.png").unwrap(),
            PathBuf::from("/tmp/shot.png")
        );
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            file_uri_to_path("file:///home/me/My%20Pictures/a%2Bb.png").unwrap(),
            PathBuf::from("/home/me/My Pictures/a+b.png")
        );
        // Non-ASCII survives as UTF-8.
        assert_eq!(
            file_uri_to_path("file:///tmp/%C3%A9.png").unwrap(),
            PathBuf::from("/tmp/\u{e9}.png")
        );
    }

    #[test]
    fn non_file_and_remote_uris_are_rejected() {
        assert!(file_uri_to_path("https://example.com/shot.png").is_err());
        assert!(file_uri_to_path("file://otherhost/tmp/shot.png").is_err());
    }

    #[test]
    fn empty_and_malformed_uris_are_rejected() {
        assert!(file_uri_to_path("file://").is_err());
        assert!(file_uri_to_path("file:///").is_err());
        assert!(file_uri_to_path("file:///tmp/%ZZ.png").is_err());
        assert!(file_uri_to_path("file:///tmp/%4").is_err());
    }

    #[test]
    fn dbus_error_names_are_extracted_from_messages() {
        assert_eq!(
            dbus_error_name("org.freedesktop.DBus.Error.ServiceUnknown: no such name"),
            Some("org.freedesktop.DBus.Error.ServiceUnknown")
        );
        assert_eq!(dbus_error_name("connection refused"), None);
    }

    #[test]
    fn extracted_dbus_names_classify_the_same_way_as_portal_responses() {
        let text = "org.freedesktop.portal.Error.Cancelled: user dismissed";
        let name = dbus_error_name(text).unwrap();
        assert!(classify_portal_error(name, text).is_cancelled());
    }

    #[test]
    fn the_portal_backend_refuses_enumeration_with_a_useful_message() {
        let backend = PortalBackend::new();
        assert_eq!(backend.name(), "wayland-portal");
        let err = backend.monitors().unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
        assert!(err.to_string().contains("monitor geometry"));
        assert!(matches!(
            backend.windows(),
            Err(CaptureError::Unsupported(_))
        ));
    }

    #[test]
    fn the_portal_backend_advertises_only_what_it_can_do() {
        let caps = PortalBackend::new().capabilities();
        assert!(caps.full_desktop);
        assert!(caps.region);
        assert!(caps.may_prompt_for_permission);
        assert!(!caps.monitor);
        assert!(!caps.window);
        assert!(!caps.monitor_enumeration);
        assert!(!caps.window_enumeration);
    }

    #[test]
    fn unsupported_targets_are_refused_before_any_dbus_traffic() {
        // No portal is running on the build machine, so reaching D-Bus would
        // fail with a different error; getting `Unsupported` proves the
        // capability check short-circuits first.
        let backend = PortalBackend::new();
        for target in [
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(1)),
            CaptureTarget::region_on(MonitorId::new(1), Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
        ] {
            assert!(
                matches!(backend.capture(target), Err(CaptureError::Unsupported(_))),
                "{target:?} should be refused up front"
            );
        }
    }
}
