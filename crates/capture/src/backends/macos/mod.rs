//! macOS: ScreenCaptureKit, and the platform-independent half of it.
//!
//! # Status: compile-verified, never run
//!
//! **This backend has never been executed on a Mac.** It is type-checked
//! against the real Apple API surface (`cargo check -p bettershot-capture
//! --target aarch64-apple-darwin`, which works from a Linux box because the
//! `objc2-*` bindings are pure Rust and `check` does not link), and every piece
//! of logic that could be lifted out of the Objective-C boundary lives in this
//! module and is unit tested on the build host. Everything else — the actual
//! ScreenCaptureKit calls in [`sck`] — is unverified beyond the type checker.
//!
//! A first run on real hardware should specifically check:
//!
//! 1. **The permission prompt appears.** A never-asked app must get the TCC
//!    dialog from `CGRequestScreenCaptureAccess`, and a previously-denied app
//!    must get [`CaptureError::PermissionDenied`] naming *System Settings →
//!    Privacy & Security → Screen & System Audio Recording* instead of a
//!    silent hang or a generic backend error.
//! 2. **Mixed-DPI displays report correct physical sizes.** A Retina laptop
//!    panel next to a 1x external monitor must produce `MonitorInfo::bounds`
//!    whose *sizes* are the true pixel counts. They are read straight off the
//!    current `CGDisplayMode` (`CGDisplayModeGetPixelWidth` /
//!    `CGDisplayModeGetPixelHeight`) rather than derived from the point size, so
//!    a display in a scaled HiDPI mode — where the height does *not* come out
//!    as `point_height x width_ratio` — is still exact. See
//!    [`points_to_physical`] for the known weakness in the *origins* of such a
//!    layout.
//! 3. **Window capture excludes shadows.** `SCStreamConfiguration`'s
//!    `ignoreShadowsSingleWindow` / `ignoreGlobalClipSingleWindow` are set; the
//!    resulting image should be the window frame with no drop shadow border and
//!    no alpha halo.
//! 4. **Stitched multi-display geometry.** A `FullDesktop` grab across two
//!    displays must land both images at the right offsets with no seam, no
//!    overlap and no doubled content.
//! 5. Windows behind others still capture their own content (ScreenCaptureKit
//!    composites per-window rather than reading the framebuffer), and the
//!    front-to-back `z_order` assignment matches what the user sees.
//! 6. **Off-screen windows are listed and refused.** The enumeration asks for
//!    them (`onScreenWindowsOnly: false`), so a window minimised to the Dock
//!    must appear in `windows()` with `is_minimized: true`, must never win a
//!    hit test, and must fail a `Window` capture with the "no on-screen pixels"
//!    message rather than a ScreenCaptureKit error. Check too that the list is
//!    not swamped by windows on other Spaces.
//! 7. **Captured images really are 32-bit little-endian BGRA.** `decode_image`
//!    now refuses anything else instead of silently producing wrong colours;
//!    a working capture on real hardware is the confirmation that
//!    `kCGImageByteOrder32Little` is what ScreenCaptureKit actually reports.
//!
//! # Why the logic lives here rather than in [`sck`]
//!
//! Nothing in this file mentions Objective-C, so it compiles and is tested on
//! Linux and Windows build machines too. [`sck`] is a thin adapter that turns
//! Apple types into the plain data these functions take.
//!
//! # Coordinate spaces
//!
//! ScreenCaptureKit reports **points** with a top-left origin (the CoreGraphics
//! global display space, *not* AppKit's bottom-left one), and bettershot's
//! canonical space is **physical pixels**, also top-left. The conversion is
//! [`points_to_physical`], driven by a backing scale factor derived by
//! [`backing_scale`] from the display's pixel width over its point width.

// On non-macOS hosts only the unit tests below call these helpers. They are
// still compiled and tested everywhere on purpose: that is the whole point of
// keeping them out of the `cfg`-gated Objective-C module.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::time::Duration;

use bettershot_core::Rect;

use crate::{
    BYTES_PER_PIXEL, Capabilities, CaptureError, CaptureTarget, MonitorId, MonitorInfo,
    VirtualDesktop, WindowId, WindowInfo, pixels,
};

#[cfg(target_os = "macos")]
pub(crate) mod sck;

/// The backend identifier, shared by the real backend and by the placeholder
/// used when ScreenCaptureKit is missing. It must match
/// [`crate::BackendChoice::MacOsScreenCaptureKit`]'s name.
pub(crate) const NAME: &str = "macos-screencapturekit";

/// ScreenCaptureKit itself arrived in this release.
pub(crate) const MIN_SCREEN_CAPTURE_KIT: &str = "12.3";

/// `SCScreenshotManager`, the one-shot screenshot API this backend captures
/// with, arrived in this release.
pub(crate) const MIN_SCREENSHOT_MANAGER: &str = "14.0";

/// Where the user has to go to grant Screen Recording, spelled exactly as macOS
/// spells it in current releases.
pub(crate) const SETTINGS_PATH: &str =
    "System Settings -> Privacy & Security -> Screen & System Audio Recording";

/// Screen Recording is a TCC permission: it cannot be granted from inside the
/// process, the prompt only ever appears once, and a freshly granted permission
/// does not reach an already-running process.
pub(crate) fn permission_denied(context: &str) -> CaptureError {
    CaptureError::permission_denied(format!(
        "macOS has not granted bettershot Screen Recording access ({context}). \
         Enable bettershot under {SETTINGS_PATH}, then quit and relaunch \
         bettershot -- macOS only hands the permission to a process at launch, \
         so granting it while bettershot is running is not enough."
    ))
}

/// The "granted but stale" case: TCC says yes, ScreenCaptureKit says there is
/// nothing to capture. That combination means this process started before the
/// permission was granted (or the TCC record was reset), and only a relaunch
/// fixes it.
pub(crate) fn stale_permission() -> CaptureError {
    CaptureError::permission_denied(format!(
        "macOS reports that Screen Recording is allowed, but ScreenCaptureKit \
         returned no displays. That happens when the permission was granted \
         after this process started, or was reset. Quit and relaunch \
         bettershot; if it persists, remove and re-add bettershot under \
         {SETTINGS_PATH}."
    ))
}

/// The API this backend needs is newer than the macOS it is running on.
pub(crate) fn too_old(api: &str, min_version: &str) -> CaptureError {
    CaptureError::unsupported(format!(
        "{api} is missing, so this macOS is older than {min_version}; \
         bettershot needs macOS {min_version} or newer to capture the screen"
    ))
}

/// What the backend can do on a macOS where `SCScreenshotManager` may be
/// missing.
///
/// ScreenCaptureKit itself (12.3+) is enough to *enumerate* displays and
/// windows; grabbing pixels needs `SCScreenshotManager` (14.0+). On 12.3–13.x
/// the honest answer is therefore "every enumeration, no capture", which is
/// what an app shell needs to grey out the capture modes instead of offering
/// something that will fail.
pub(crate) fn capabilities_for(can_screenshot: bool) -> Capabilities {
    Capabilities {
        full_desktop: can_screenshot,
        monitor: can_screenshot,
        window: can_screenshot,
        region: can_screenshot,
        // Enumeration only needs ScreenCaptureKit itself (12.3+), so it is
        // available even when the screenshot API is not.
        monitor_enumeration: true,
        window_enumeration: true,
        // No picker: ScreenCaptureKit captures without a dialog once the TCC
        // permission is granted.
        interactive_only: false,
        // The first capture on a fresh install raises the Screen Recording
        // prompt, and a denied permission is a hard failure.
        may_prompt_for_permission: true,
        // ScreenCaptureKit can composite the pointer itself, via
        // `SCStreamConfiguration.showsCursor`, so this is a matter of wiring
        // rather than a platform limit — but it belongs with the rest of the
        // macOS capture work in Phase 5, which needs a Mac to verify.
        cursor: false,
    }
}

/// Refuse a capture that this macOS cannot serve, with the *most specific*
/// reason first.
///
/// Order matters. [`capabilities_for`] turns a missing `SCScreenshotManager`
/// into "no target kind is capturable", so a plain
/// [`Capabilities::ensure_supports`] on macOS 12.3–13.x would answer every
/// request with "the macos-screencapturekit backend cannot capture a monitor
/// target" — true, but it hides the only thing the user can act on, which is
/// that bettershot needs macOS 14.0. The version check therefore runs first;
/// the capability check stays behind it as the general answer for any target
/// kind a future revision of this backend cannot serve.
pub(crate) fn ensure_capture_supported(
    can_screenshot: bool,
    target: &CaptureTarget,
) -> Result<(), CaptureError> {
    if !can_screenshot {
        return Err(too_old("SCScreenshotManager", MIN_SCREENSHOT_MANAGER));
    }
    capabilities_for(can_screenshot).ensure_supports(NAME, target)
}

/// A completion handler never fired. Returning this beats blocking forever:
/// bettershot's capture path has no window of its own to cancel from, so a hung
/// grab is unrecoverable for the user.
pub(crate) fn timed_out(operation: &str, waited: Duration) -> CaptureError {
    CaptureError::backend(format!(
        "ScreenCaptureKit did not call back within {:.0?} while {operation}; \
         giving up rather than blocking forever. This usually means the \
         Screen Recording permission is in a stale state -- quit and relaunch \
         bettershot, and check {SETTINGS_PATH}.",
        waited
    ))
}

/// Map an `NSError` from the `SCStreamErrorDomain` onto a [`CaptureError`].
///
/// Kept pure — domain string, code and message in, error out — so the mapping
/// is testable without a Mac, exactly like [`crate::classify_portal_error`] is
/// testable without a portal. Codes are `SCStreamErrorCode` from
/// `objc2_screen_capture_kit::SCStreamErrorCode`.
pub(crate) fn classify_sc_error(domain: &str, code: i64, message: &str) -> CaptureError {
    // Only the ScreenCaptureKit domain uses these numbers; anything else (an
    // OSStatus or a Foundation error) must not be misread as a denial.
    if domain != "SCStreamErrorDomain" {
        return CaptureError::backend(format!("{domain} {code}: {message}"));
    }
    match code {
        // SCStreamErrorUserDeclined / SCStreamErrorMissingEntitlements.
        -3801 | -3803 => permission_denied(message),
        // SCStreamErrorUserStopped: the user ended the capture themselves.
        -3817 => CaptureError::Cancelled,
        // SCStreamErrorNoWindowList (-3813), SCStreamErrorNoDisplayList
        // (-3814) and SCStreamErrorNoCaptureSource (-3815): the classic symptom
        // of a permission that was granted after launch.
        -3815..=-3813 => stale_permission(),
        _ => CaptureError::backend(format!("ScreenCaptureKit error {code}: {message}")),
    }
}

/// The backing scale factor of a display: its width in physical pixels over its
/// width in points.
///
/// Derived rather than read from a `backingScaleFactor` property because
/// `SCDisplay` has none, and because a display running a scaled ("looks like
/// 1680x1050") mode has a non-integer factor that only this ratio captures.
/// Nonsense inputs fall back to `1.0` rather than poisoning later arithmetic.
pub(crate) fn backing_scale(pixel_width: u32, point_width: f32) -> f32 {
    if pixel_width == 0 || !point_width.is_finite() || point_width <= 0.0 {
        return 1.0;
    }
    let scale = pixel_width as f32 / point_width;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Convert a point-space rectangle to physical pixels.
///
/// The left edge, the top edge, the width and the height are each scaled and
/// then rounded *independently*; the right and bottom edges are whatever those
/// four imply. A display that is already integral in both spaces survives
/// untouched, and a size is never distorted by where the rectangle happens to
/// sit.
///
/// # Known weakness on mixed-DPI desktops
///
/// macOS lays displays out in *points*, so scaling each display's origin by its
/// own factor is only exact when every display shares that factor. A 2x Retina
/// panel at points `(0,0,1512,982)` beside a 1x display at points
/// `(1512,0,1920,1080)` becomes physical `(0,0,3024,1964)` and
/// `(1512,0,1920,1080)` — which overlap. Sizes stay correct; origins do not.
/// There is no correct answer here without inventing a physical layout macOS
/// does not define, so this backend keeps the direct conversion, and
/// [`crate::stitch`] at least degrades predictably (later frames win, gaps stay
/// transparent). Verifying this on real mixed-DPI hardware is item 2 on the
/// module's first-run checklist.
///
/// This is a property of the *coordinate space*, not of stitching: it is
/// already baked into [`MonitorInfo::bounds`] before any grab happens. See
/// [`monitors_covering`] for why `SCScreenshotManager`'s multi-display
/// `captureImageInRect:` does not remove it either.
pub(crate) fn points_to_physical(rect: Rect, scale: f32) -> Rect {
    let rect = rect.normalized();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    Rect::from_xywh(
        (rect.left() * scale).round(),
        (rect.top() * scale).round(),
        (rect.width() * scale).round(),
        (rect.height() * scale).round(),
    )
}

/// One display's point-space footprint and its scale, enough to place anything
/// else reported in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DisplayScale {
    /// The display's frame in points, top-left origin.
    pub(crate) frame_points: Rect,
    /// Physical pixels per point on this display.
    pub(crate) scale: f32,
}

/// Convert a window frame from points to physical pixels.
///
/// A window belongs to the display its centre sits on, which is also how macOS
/// decides which display's backing store it renders into. A window straddling a
/// seam therefore follows its centre like any other — there is no special case,
/// because "which display is it mostly on" is exactly the question the centre
/// answers.
///
/// `fallback_scale` is used only when the centre lands on *no* display at all
/// (a fully off-screen window, or one hanging into the gap of a staggered
/// layout). It should be the desktop's densest factor, so that guess never
/// *under*-sizes the grab.
pub(crate) fn window_points_to_physical(
    frame_points: Rect,
    displays: &[DisplayScale],
    fallback_scale: f32,
) -> Rect {
    let frame = frame_points.normalized();
    let centre = frame.pos + frame.size * 0.5;
    let scale = displays
        .iter()
        .find(|d| crate::geometry::contains_half_open(d.frame_points, centre))
        .map(|d| d.scale)
        .unwrap_or(fallback_scale);
    points_to_physical(frame, scale)
}

/// A window as ScreenCaptureKit describes it, already converted into
/// bettershot's coordinate space.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowRecord {
    /// `CGWindowID`.
    pub(crate) id: u64,
    /// `SCWindow.title`, empty when the window has none.
    pub(crate) title: String,
    /// `SCWindow.owningApplication.applicationName`.
    pub(crate) app_name: String,
    /// Frame in physical pixels on the virtual desktop.
    pub(crate) bounds: Rect,
    /// `SCWindow.windowLayer`: the CoreGraphics window level. Larger is nearer
    /// the viewer (the Dock and menu bar sit above normal windows).
    pub(crate) layer: i64,
    /// `SCWindow.isOnScreen`.
    ///
    /// Only meaningful because `sck::shareable_content` asks for off-screen
    /// windows too (`onScreenWindowsOnly: false`). With `true` every returned
    /// window is on screen by construction and this field is a constant.
    pub(crate) on_screen: bool,
}

/// Turn ScreenCaptureKit's window list into bettershot's `0 = frontmost`
/// convention.
///
/// ScreenCaptureKit hands back windows front-to-back *within* a window level,
/// but the levels themselves are not interleaved into that order, so a
/// floating panel can be listed after a normal window it visually covers.
/// Sorting by descending `layer` first and keeping ScreenCaptureKit's order
/// inside each layer (a stable sort) reproduces the on-screen stacking, and
/// the resulting index becomes `z_order`.
///
/// `is_minimized` is `!on_screen`. ScreenCaptureKit has no minimised flag —
/// `SCWindow.isOnScreen` is the closest thing, and it is `false` for a window
/// that is minimised to the Dock, hidden with its application, or parked on
/// another Space. bettershot's flag therefore means "has no on-screen pixels
/// right now", which is a superset of "minimised" and is exactly the set
/// [`crate::resolve_target`] must refuse and [`crate::window_at`] must skip.
///
/// This only carries information because the enumeration asks for off-screen
/// windows as well; see `sck::shareable_content`.
pub(crate) fn windows_front_to_back(records: Vec<WindowRecord>) -> Vec<WindowInfo> {
    let mut ordered: Vec<(usize, WindowRecord)> = records.into_iter().enumerate().collect();
    // Stable: ties keep ScreenCaptureKit's own front-to-back order.
    ordered.sort_by_key(|(index, record)| (std::cmp::Reverse(record.layer), *index));
    ordered
        .into_iter()
        .enumerate()
        .map(|(z, (_, record))| {
            WindowInfo::new(
                WindowId::new(record.id),
                record.title,
                record.app_name,
                record.bounds,
                !record.on_screen,
                z as u32,
            )
        })
        .collect()
}

/// The monitors whose pixels overlap `bounds`, in enumeration order.
///
/// Every `SCContentFilter` initialiser that takes a display takes exactly *one*
/// (`initWithDisplay:excludingWindows:` and friends), so a full-desktop or
/// cross-monitor region capture is assembled from one
/// `captureImageWithFilter:configuration:` per display and then
/// [`crate::stitch`]ed — the same shape the Windows backend uses, for the same
/// reason.
///
/// # Why not `captureImageInRect:completionHandler:`
///
/// `SCScreenshotManager` does have a filter-free variant whose rect is, in
/// Apple's words, "display agnostic and supports multiple displays". It is
/// deliberately *not* used, and it would not fix the mixed-DPI origin problem
/// documented on [`points_to_physical`]:
///
/// * **Its rect is in points**, "specified in display space". bettershot's
///   canonical space is physical pixels, so asking for the whole desktop means
///   converting the desktop's physical bounding box back into points — the same
///   per-display scale ambiguity as [`points_to_physical`], merely inverted.
///   The skew lives in the point-to-pixel mapping itself, not in the stitch.
/// * **It takes no `SCStreamConfiguration`**, so there is no `setWidth:` /
///   `setHeight:` and the output resolution is whatever the system picks. The
///   per-display path sizes each grab from its own filter's `contentRect` and
///   `pointPixelScale`, which is what guarantees each display is rendered at
///   its native resolution rather than up- or down-sampled to some single
///   whole-desktop scale. The returned image's scale could only be recovered
///   after the fact, by dividing its pixel width by the requested point width.
/// * A single uniformly-scaled image would also **disagree with
///   [`MonitorInfo::bounds`]**, which is built per display. `capture` crops a
///   `Region` out of the same frame it grabs, so the two spaces have to match.
/// * It serves neither [`crate::CaptureTarget::Monitor`] nor
///   [`crate::CaptureTarget::Window`], so the per-display path and
///   [`crate::stitch`] would stay regardless.
pub(crate) fn monitors_covering(desktop: &VirtualDesktop, bounds: Rect) -> Vec<MonitorId> {
    desktop
        .monitors()
        .iter()
        .filter(|m| !m.bounds.clamped_to(bounds).is_empty())
        .map(|m| m.id)
        .collect()
}

/// Whether a `CGImage`'s alpha channel carries real information.
///
/// `kCGImageAlphaNone` (0), `kCGImageAlphaNoneSkipLast` (5) and
/// `kCGImageAlphaNoneSkipFirst` (6) mean the fourth byte is padding whose
/// contents are undefined — copying it through is the classic "the screenshot
/// is entirely transparent" bug. Every other layout has a meaningful alpha,
/// which window captures need for rounded corners.
pub(crate) fn force_opaque_for(alpha_info: u32) -> bool {
    matches!(alpha_info, 0 | 5 | 6)
}

/// Unpack a `CGImage`'s BGRA8 pixels into the tightly packed RGBA8 a
/// [`crate::RawFrame`] wants.
///
/// `bytes_per_row` is `CGImageGetBytesPerRow`, which is essentially always
/// *larger* than `width * 4`: CoreGraphics pads scanlines out to a cache-line
/// or tile boundary. Reading the buffer as if it were tightly packed produces
/// the familiar diagonally-skewed screenshot, so the padding is skipped here
/// row by row and the channel swap is delegated to [`pixels::bgra_to_rgba`].
///
/// The BGRA part of that is *checked*, not assumed: see [`ensure_bgra8`], which
/// the caller runs against `CGImageGetBitsPerPixel` and
/// `CGImageGetByteOrderInfo` before any of these bytes are touched.
///
/// ScreenCaptureKit's SDR output is BGRA with *premultiplied* alpha. That is
/// left as-is: `RawFrame` is documented as RGBA8 and every consumer in
/// bettershot composites premultiplied, so un-premultiplying here would only
/// lose precision on translucent window edges.
pub(crate) fn bgra_rows_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
    force_opaque: bool,
) -> Result<Vec<u8>, CaptureError> {
    let row_bytes = (width as usize)
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| CaptureError::invalid_frame("CGImage row does not fit in memory"))?;
    if width == 0 || height == 0 {
        return Err(CaptureError::EmptyRegion);
    }
    if bytes_per_row < row_bytes {
        return Err(CaptureError::invalid_frame(format!(
            "CGImage claims {bytes_per_row} bytes per row, which is less than \
             the {row_bytes} bytes {width} BGRA pixels need"
        )));
    }
    // Only the last row is guaranteed to be present in full; trailing padding
    // after it may legitimately be absent.
    let needed = bytes_per_row
        .checked_mul(height as usize - 1)
        .and_then(|full| full.checked_add(row_bytes))
        .ok_or_else(|| CaptureError::invalid_frame("CGImage does not fit in memory"))?;
    if data.len() < needed {
        return Err(CaptureError::invalid_frame(format!(
            "CGImage is {} bytes, expected at least {needed} for {width}x{height} \
             at {bytes_per_row} bytes per row",
            data.len()
        )));
    }

    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for y in 0..height as usize {
        let start = y * bytes_per_row;
        out.extend_from_slice(&data[start..start + row_bytes]);
    }
    pixels::bgra_to_rgba(&mut out, force_opaque)?;
    Ok(out)
}

/// Build a [`MonitorInfo`] from what `SCDisplay` and CoreGraphics report.
///
/// `frame_points` is `SCDisplay.frame`, `pixel_size` is
/// (`CGDisplayModeGetPixelWidth`, `CGDisplayModeGetPixelHeight`) for the same
/// display, and `is_primary` is `displayID == CGMainDisplayID()`.
///
/// The *origin* has to be derived — macOS only publishes it in points — but the
/// *size* does not: `pixel_size` is the real framebuffer, so it is used
/// verbatim rather than as `point_size x scale`. That matters in a scaled HiDPI
/// mode, where the width ratio is not an exact description of the height: a
/// 3456x2234 panel driven as 1800x1169 points has a width ratio of 1.92, and
/// `round(1169 x 1.92)` is 2244, ten rows taller than the panel actually is.
///
/// `pixel_size` of `(0, 0)` means "the display mode could not be read"; the
/// point size then stands in, which also yields a scale of 1.0.
pub(crate) fn monitor_from_display(
    display_id: u32,
    name: impl Into<String>,
    frame_points: Rect,
    pixel_size: (u32, u32),
    is_primary: bool,
) -> MonitorInfo {
    let (pixel_width, pixel_height) = pixel_size;
    let frame_points = frame_points.normalized();
    let scale = backing_scale(pixel_width, frame_points.width());
    let mut bounds = points_to_physical(frame_points, scale);
    if pixel_width > 0 && pixel_height > 0 {
        bounds = Rect::from_xywh(
            bounds.left(),
            bounds.top(),
            pixel_width as f32,
            pixel_height as f32,
        );
    }
    MonitorInfo::new(
        MonitorId::new(u64::from(display_id)),
        name,
        bounds,
        scale,
        is_primary,
    )
}

/// The pixel size to ask an `SCStreamConfiguration` for, from what the content
/// filter reports about itself.
///
/// `point_pixel_scale` is `SCContentFilter.pointPixelScale` and `width_points` /
/// `height_points` are `SCContentFilter.contentRect`'s size. Asking the filter
/// rather than computing it from [`MonitorInfo`] means the request always
/// matches what ScreenCaptureKit believes it is about to render, which is the
/// difference between a sharp Retina grab and a 2x-upscaled blurry one.
///
/// # Why nonsense is an error rather than a default
///
/// `SCStreamConfiguration.width` and `.height` are documented by Apple as
/// defaulting to **1920** and **1080**. Leaving them unset when the filter
/// reports nonsense would therefore not "let ScreenCaptureKit choose": it would
/// silently produce a 1920x1080 image, which the caller then wraps in a
/// [`crate::RawFrame`] carrying the display's real origin and scale factor. The
/// result is a mis-sized, mis-placed screenshot with no error anywhere. A
/// refusal the user can read is strictly better.
pub(crate) fn stream_pixel_size(
    point_pixel_scale: f64,
    width_points: f64,
    height_points: f64,
) -> Result<(usize, usize), CaptureError> {
    let width = width_points * point_pixel_scale;
    let height = height_points * point_pixel_scale;
    if point_pixel_scale.is_finite()
        && point_pixel_scale > 0.0
        && width.is_finite()
        && height.is_finite()
        && width.round() >= 1.0
        && height.round() >= 1.0
        && width.round() <= usize::MAX as f64
        && height.round() <= usize::MAX as f64
    {
        return Ok((width.round() as usize, height.round() as usize));
    }
    Err(CaptureError::backend(format!(
        "ScreenCaptureKit's content filter reports a {width_points}x{height_points} pt \
         region at {point_pixel_scale} px/pt, which is not a usable pixel size. \
         Refusing rather than leaving SCStreamConfiguration at its documented \
         1920x1080 default, which would silently return a mis-sized screenshot."
    )))
}

/// `kCGImageByteOrder32Little`, from `CGImageByteOrderInfo`.
///
/// This is the one 32-bit layout [`bgra_rows_to_rgba`] decodes: a little-endian
/// 32-bit word with alpha in the high byte, which in memory is B, G, R, A.
/// ScreenCaptureKit documents its SDR `CGImage` output as BGRA, so this is what
/// a healthy capture reports.
pub(crate) const BYTE_ORDER_32_LITTLE: u32 = 2 << 12;

/// Is this `CGImage` really the 32-bit BGRA [`bgra_rows_to_rgba`] assumes?
///
/// `bits_per_pixel` alone does not answer it. `kCGImageByteOrder32Big` is also
/// 32 bits per pixel, but lays the same word out as A, R, G, B — feeding that
/// to a BGRA decoder yields G, R, A, B, i.e. plausible-looking pixels in the
/// wrong colours, with nothing to alert the user. Anything that is not
/// little-endian 32-bit is refused by name instead.
pub(crate) fn ensure_bgra8(bits_per_pixel: usize, byte_order: u32) -> Result<(), CaptureError> {
    if bits_per_pixel != 32 {
        return Err(CaptureError::unsupported(format!(
            "ScreenCaptureKit returned a {bits_per_pixel}-bit image; \
             bettershot only handles 32-bit BGRA"
        )));
    }
    if byte_order != BYTE_ORDER_32_LITTLE {
        return Err(CaptureError::unsupported(format!(
            "ScreenCaptureKit returned a 32-bit image with byte order {}; \
             bettershot only handles kCGImageByteOrder32Little, the \
             B, G, R, A memory layout ScreenCaptureKit documents for SDR \
             captures. Decoding any other order as BGRA would silently swap \
             the colour channels.",
            byte_order_name(byte_order)
        )));
    }
    Ok(())
}

/// Spell a `CGImageByteOrderInfo` the way Apple's headers do, so an error names
/// something the reader can look up.
fn byte_order_name(byte_order: u32) -> String {
    let name = match byte_order {
        0 => "kCGImageByteOrderDefault",
        0x1000 => "kCGImageByteOrder16Little",
        0x2000 => "kCGImageByteOrder32Little",
        0x3000 => "kCGImageByteOrder16Big",
        0x4000 => "kCGImageByteOrder32Big",
        other => return format!("{other:#x}"),
    };
    format!("{name} ({byte_order:#x})")
}

/// The human-readable name for a display id, matching what other tools show.
pub(crate) fn display_name(display_id: u32, is_primary: bool) -> String {
    if is_primary {
        format!("Display {display_id} (primary)")
    } else {
        format!("Display {display_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::Vec2D;

    #[test]
    fn the_backend_name_is_stable() {
        assert_eq!(NAME, "macos-screencapturekit");
    }

    // ---------------------------------------------------------------- scale

    #[test]
    fn a_non_retina_display_has_a_scale_of_one() {
        assert_eq!(backing_scale(1920, 1920.0), 1.0);
    }

    #[test]
    fn a_retina_display_has_a_scale_of_two() {
        assert_eq!(backing_scale(3024, 1512.0), 2.0);
    }

    #[test]
    fn a_scaled_retina_mode_has_a_fractional_scale() {
        // "Looks like 1680x1050" on a 3360x2100 panel.
        assert_eq!(backing_scale(3360, 1680.0), 2.0);
        // A genuinely non-integer case: 2560 physical shown as 1707 points.
        let scale = backing_scale(2560, 1707.0);
        assert!((scale - 1.4997).abs() < 0.001, "{scale}");
    }

    #[test]
    fn nonsense_scale_inputs_fall_back_to_one() {
        assert_eq!(backing_scale(0, 1920.0), 1.0);
        assert_eq!(backing_scale(1920, 0.0), 1.0);
        assert_eq!(backing_scale(1920, -5.0), 1.0);
        assert_eq!(backing_scale(1920, f32::NAN), 1.0);
    }

    // ------------------------------------------------- points -> pixels

    #[test]
    fn at_one_x_points_are_already_physical_pixels() {
        let points = Rect::from_xywh(1920.0, -120.0, 1920.0, 1080.0);
        assert_eq!(points_to_physical(points, 1.0), points);
    }

    #[test]
    fn at_two_x_every_edge_doubles() {
        let points = Rect::from_xywh(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(
            points_to_physical(points, 2.0),
            Rect::from_xywh(0.0, 0.0, 3024.0, 1964.0)
        );
    }

    #[test]
    fn negative_origins_scale_too() {
        // A display above and left of the primary, at 2x.
        let points = Rect::from_xywh(-1440.0, -900.0, 1440.0, 900.0);
        assert_eq!(
            points_to_physical(points, 2.0),
            Rect::from_xywh(-2880.0, -1800.0, 2880.0, 1800.0)
        );
    }

    #[test]
    fn fractional_scales_round_to_whole_pixels() {
        let points = Rect::from_xywh(0.0, 0.0, 1707.0, 1067.0);
        let physical = points_to_physical(points, 1.4997);
        assert_eq!(physical, Rect::from_xywh(0.0, 0.0, 2560.0, 1600.0));
    }

    #[test]
    fn a_broken_scale_leaves_the_rect_alone() {
        let points = Rect::from_xywh(10.0, 20.0, 30.0, 40.0);
        assert_eq!(points_to_physical(points, 0.0), points);
        assert_eq!(points_to_physical(points, f32::NAN), points);
    }

    #[test]
    fn monitor_from_display_reports_physical_bounds_and_the_derived_scale() {
        let monitor = monitor_from_display(
            1,
            "Built-in",
            Rect::from_xywh(0.0, 0.0, 1512.0, 982.0),
            (3024, 1964),
            true,
        );
        assert_eq!(monitor.id, MonitorId::new(1));
        assert_eq!(monitor.scale_factor, 2.0);
        assert_eq!(monitor.bounds, Rect::from_xywh(0.0, 0.0, 3024.0, 1964.0));
        assert!(monitor.is_primary);
        // The size in the monitor's own logical space is the point size again.
        assert_eq!(monitor.logical_size(), Vec2D::new(1512.0, 982.0));
    }

    #[test]
    fn a_monitors_size_is_the_framebuffer_not_the_scaled_point_size() {
        // A 3456x2234 panel driven in the scaled "looks like 1800x1169" mode.
        // The width ratio is 1.92, and 1169 x 1.92 rounds to 2244 -- ten rows
        // more than the panel has. The reported height must be the real 2234.
        let monitor = monitor_from_display(
            1,
            "Built-in",
            Rect::from_xywh(0.0, 0.0, 1800.0, 1169.0),
            (3456, 2234),
            true,
        );
        assert_eq!(monitor.bounds, Rect::from_xywh(0.0, 0.0, 3456.0, 2234.0));
        assert_eq!(monitor.scale_factor, 1.92);
        // The derived height, which the old code used, would have been wrong.
        assert_eq!((1169.0_f32 * 1.92).round(), 2244.0);
    }

    #[test]
    fn a_monitors_origin_is_still_derived_from_points() {
        // Only the size comes from the framebuffer; macOS publishes no physical
        // origin, so the point origin is scaled as before.
        let monitor = monitor_from_display(
            2,
            "External",
            Rect::from_xywh(-1440.0, -900.0, 1440.0, 900.0),
            (2880, 1800),
            false,
        );
        assert_eq!(
            monitor.bounds,
            Rect::from_xywh(-2880.0, -1800.0, 2880.0, 1800.0)
        );
    }

    #[test]
    fn an_unreadable_display_mode_falls_back_to_the_point_size() {
        let monitor = monitor_from_display(
            3,
            "Unknown",
            Rect::from_xywh(10.0, 20.0, 1920.0, 1080.0),
            (0, 0),
            false,
        );
        assert_eq!(monitor.scale_factor, 1.0);
        assert_eq!(monitor.bounds, Rect::from_xywh(10.0, 20.0, 1920.0, 1080.0));
    }

    #[test]
    fn display_names_mark_the_primary() {
        assert_eq!(display_name(7, true), "Display 7 (primary)");
        assert_eq!(display_name(7, false), "Display 7");
    }

    // ------------------------------------------------- window geometry

    fn two_displays() -> Vec<DisplayScale> {
        vec![
            DisplayScale {
                frame_points: Rect::from_xywh(0.0, 0.0, 1512.0, 982.0),
                scale: 2.0,
            },
            DisplayScale {
                frame_points: Rect::from_xywh(1512.0, 0.0, 1920.0, 1080.0),
                scale: 1.0,
            },
        ]
    }

    #[test]
    fn a_window_takes_the_scale_of_the_display_its_centre_is_on() {
        // Entirely on the 2x built-in panel.
        let on_retina = window_points_to_physical(
            Rect::from_xywh(100.0, 100.0, 800.0, 600.0),
            &two_displays(),
            2.0,
        );
        assert_eq!(on_retina, Rect::from_xywh(200.0, 200.0, 1600.0, 1200.0));

        // Entirely on the 1x external.
        let on_external = window_points_to_physical(
            Rect::from_xywh(1600.0, 100.0, 800.0, 600.0),
            &two_displays(),
            2.0,
        );
        assert_eq!(on_external, Rect::from_xywh(1600.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_straddling_the_seam_follows_its_centre() {
        // 1400..1800 in points: centre at 1600, which is on the external.
        let straddling = window_points_to_physical(
            Rect::from_xywh(1400.0, 0.0, 400.0, 300.0),
            &two_displays(),
            2.0,
        );
        assert_eq!(straddling, Rect::from_xywh(1400.0, 0.0, 400.0, 300.0));
    }

    #[test]
    fn a_window_on_no_display_uses_the_fallback_scale() {
        let off_screen = window_points_to_physical(
            Rect::from_xywh(9000.0, 9000.0, 100.0, 100.0),
            &two_displays(),
            2.0,
        );
        assert_eq!(off_screen, Rect::from_xywh(18000.0, 18000.0, 200.0, 200.0));
    }

    // ------------------------------------------------------- z-ordering

    fn record(id: u64, layer: i64, on_screen: bool) -> WindowRecord {
        WindowRecord {
            id,
            title: format!("Window {id}"),
            app_name: "testapp".into(),
            bounds: Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            layer,
            on_screen,
        }
    }

    #[test]
    fn screencapturekit_order_becomes_zero_is_frontmost() {
        let windows = windows_front_to_back(vec![
            record(1, 0, true),
            record(2, 0, true),
            record(3, 0, true),
        ]);
        let ids: Vec<u64> = windows.iter().map(|w| w.id.get()).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        let zs: Vec<u32> = windows.iter().map(|w| w.z_order).collect();
        assert_eq!(zs, vec![0, 1, 2]);
        // ...and the crate's own hit-testing agrees on who is in front.
        assert_eq!(
            crate::window::topmost(&windows).unwrap().id,
            WindowId::new(1)
        );
    }

    #[test]
    fn higher_window_levels_are_pulled_in_front() {
        // The Dock (layer 20) listed *after* two normal windows must still end
        // up frontmost.
        let windows = windows_front_to_back(vec![
            record(1, 0, true),
            record(2, 0, true),
            record(3, 20, true),
        ]);
        let ids: Vec<u64> = windows.iter().map(|w| w.id.get()).collect();
        assert_eq!(ids, vec![3, 1, 2]);
        assert_eq!(windows[0].z_order, 0);
    }

    #[test]
    fn windows_below_the_normal_level_sink_to_the_back() {
        // Desktop icons sit at a negative level.
        let windows = windows_front_to_back(vec![record(1, -2147483623, true), record(2, 0, true)]);
        let ids: Vec<u64> = windows.iter().map(|w| w.id.get()).collect();
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn off_screen_windows_are_reported_as_minimized() {
        // These records reach `read_windows` only because the enumeration asks
        // for off-screen windows too; with `onScreenWindowsOnly: true` every
        // `on_screen` would be `true` and this whole branch would be dead.
        let windows = windows_front_to_back(vec![record(1, 0, false), record(2, 0, true)]);
        assert!(windows[0].is_minimized);
        assert!(!windows[1].is_minimized);
        // Minimised windows never win a hit test.
        assert_eq!(
            crate::window::topmost(&windows).unwrap().id,
            WindowId::new(2)
        );
    }

    #[test]
    fn an_off_screen_window_is_refused_with_the_reason_rather_than_grabbed() {
        // The other half of the same contract: the flag is only worth setting
        // because `resolve_target` acts on it.
        let windows = windows_front_to_back(vec![record(1, 0, false)]);
        let err = crate::resolve_target(
            crate::CaptureTarget::Window(WindowId::new(1)),
            &desktop(),
            &windows,
        )
        .unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
        assert!(err.to_string().contains("no on-screen pixels"), "{err}");
    }

    #[test]
    fn an_empty_window_list_stays_empty() {
        assert!(windows_front_to_back(Vec::new()).is_empty());
    }

    // ------------------------------------------------- row padding

    /// `width` BGRA pixels per row, `bytes_per_row` apart, with the padding
    /// filled with a value that must never appear in the output.
    fn padded(width: u32, height: u32, bytes_per_row: usize) -> Vec<u8> {
        let mut data = vec![0xEE; bytes_per_row * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let at = y * bytes_per_row + x * 4;
                // B, G, R, A — distinguishable per pixel.
                data[at] = 0x10 + x as u8;
                data[at + 1] = 0x20 + y as u8;
                data[at + 2] = 0x30;
                data[at + 3] = 0x80;
            }
        }
        data
    }

    #[test]
    fn row_padding_is_skipped_and_channels_are_swapped() {
        // 3 pixels = 12 bytes of content, padded out to a 16-byte stride.
        let data = padded(3, 2, 16);
        let rgba = bgra_rows_to_rgba(&data, 3, 2, 16, false).unwrap();
        assert_eq!(rgba.len(), 3 * 2 * 4);
        // Row 0, pixel 0: BGRA 10 20 30 80 -> RGBA 30 20 10 80.
        assert_eq!(&rgba[0..4], &[0x30, 0x20, 0x10, 0x80]);
        // Row 0, pixel 2.
        assert_eq!(&rgba[8..12], &[0x30, 0x20, 0x12, 0x80]);
        // Row 1, pixel 0 — this is the one that lands on padding if the stride
        // is ignored.
        assert_eq!(&rgba[12..16], &[0x30, 0x21, 0x10, 0x80]);
        assert!(!rgba.contains(&0xEE), "padding leaked into the output");
    }

    #[test]
    fn a_tightly_packed_image_needs_no_special_case() {
        let data = padded(4, 3, 16);
        let rgba = bgra_rows_to_rgba(&data, 4, 3, 16, false).unwrap();
        assert_eq!(rgba.len(), 4 * 3 * 4);
        assert_eq!(&rgba[0..4], &[0x30, 0x20, 0x10, 0x80]);
    }

    #[test]
    fn a_realistic_retina_stride_unpacks_cleanly() {
        // 3024 physical pixels is 12096 bytes; CoreGraphics rounds up to 12160.
        let width = 3024;
        let stride = 12160;
        let data = vec![7u8; stride * 2];
        let rgba = bgra_rows_to_rgba(&data, width, 2, stride, true).unwrap();
        assert_eq!(rgba.len() as usize, width as usize * 2 * 4);
        // Every pixel is 07 07 07 with alpha forced opaque.
        assert_eq!(&rgba[0..4], &[7, 7, 7, 255]);
    }

    #[test]
    fn opaque_forcing_overwrites_a_junk_alpha_channel() {
        let mut data = padded(2, 1, 8);
        data[3] = 0x00;
        data[7] = 0x00;
        let rgba = bgra_rows_to_rgba(&data, 2, 1, 8, true).unwrap();
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 255, 0x30, 0x20, 0x11, 255]);
    }

    #[test]
    fn a_meaningful_alpha_channel_survives() {
        let data = padded(1, 1, 4);
        let rgba = bgra_rows_to_rgba(&data, 1, 1, 4, false).unwrap();
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 0x80]);
    }

    #[test]
    fn only_the_last_row_may_be_short() {
        // Exactly enough bytes for two full rows minus the trailing padding.
        let data = vec![0u8; 16 + 12];
        assert!(bgra_rows_to_rgba(&data, 3, 2, 16, true).is_ok());
        let short = vec![0u8; 16 + 11];
        assert!(matches!(
            bgra_rows_to_rgba(&short, 3, 2, 16, true),
            Err(CaptureError::InvalidFrame(_))
        ));
    }

    #[test]
    fn an_impossible_stride_is_rejected_rather_than_read_out_of_bounds() {
        let data = vec![0u8; 64];
        let err = bgra_rows_to_rgba(&data, 4, 2, 8, true).unwrap_err();
        assert!(matches!(err, CaptureError::InvalidFrame(_)));
        assert!(err.to_string().contains("bytes per row"), "{err}");
    }

    #[test]
    fn a_zero_sized_image_is_an_empty_region() {
        assert!(matches!(
            bgra_rows_to_rgba(&[], 0, 4, 0, true),
            Err(CaptureError::EmptyRegion)
        ));
        assert!(matches!(
            bgra_rows_to_rgba(&[], 4, 0, 16, true),
            Err(CaptureError::EmptyRegion)
        ));
    }

    #[test]
    fn alpha_layouts_without_information_force_opacity() {
        // kCGImageAlphaNone / NoneSkipLast / NoneSkipFirst.
        for none in [0, 5, 6] {
            assert!(force_opaque_for(none), "alpha info {none}");
        }
        // Premultiplied and straight alpha both carry information.
        for real in [1, 2, 3, 4] {
            assert!(!force_opaque_for(real), "alpha info {real}");
        }
    }

    // ------------------------------------------- region -> display(s)

    fn desktop() -> VirtualDesktop {
        VirtualDesktop::new(vec![
            monitor_from_display(
                1,
                "built-in",
                Rect::from_xywh(0.0, 0.0, 1512.0, 982.0),
                (3024, 1964),
                true,
            ),
            monitor_from_display(
                2,
                "external",
                Rect::from_xywh(1512.0, 0.0, 1920.0, 1080.0),
                (1920, 1080),
                false,
            ),
        ])
    }

    #[test]
    fn a_region_inside_one_display_only_grabs_that_display() {
        let desktop = desktop();
        let covering = monitors_covering(&desktop, Rect::from_xywh(100.0, 100.0, 200.0, 200.0));
        assert_eq!(covering, vec![MonitorId::new(1)]);
    }

    #[test]
    fn a_region_across_the_seam_grabs_both_displays() {
        // The built-in is physical 0..3024 wide; the external's *physical*
        // origin is its point origin, 1512, so they overlap in this space.
        // Either way both are needed.
        let desktop = desktop();
        let covering = monitors_covering(&desktop, Rect::from_xywh(1600.0, 0.0, 800.0, 400.0));
        assert_eq!(covering, vec![MonitorId::new(1), MonitorId::new(2)]);
    }

    #[test]
    fn a_region_off_every_display_grabs_nothing() {
        let desktop = desktop();
        assert!(
            monitors_covering(&desktop, Rect::from_xywh(-5000.0, -5000.0, 10.0, 10.0)).is_empty()
        );
    }

    #[test]
    fn a_full_desktop_grab_covers_every_display() {
        let desktop = desktop();
        let covering = monitors_covering(&desktop, desktop.bounds());
        assert_eq!(covering.len(), desktop.len());
    }

    #[test]
    fn resolving_a_region_then_cropping_the_display_grab_lands_on_the_same_pixels() {
        // The whole Region path in miniature: resolve, grab the covering
        // display, crop back to what was asked for.
        let desktop = desktop();
        let resolved = crate::resolve_target(
            crate::CaptureTarget::region(Rect::from_xywh(100.0, 50.0, 200.0, 100.0)),
            &desktop,
            &[],
        )
        .unwrap();
        assert_eq!(resolved.monitor, Some(MonitorId::new(1)));
        assert_eq!(resolved.scale_factor, 2.0);

        let display = desktop.require(MonitorId::new(1)).unwrap();
        let grab = crate::RawFrame::transparent(
            display.bounds.width() as u32,
            display.bounds.height() as u32,
            display.origin(),
            display.scale_factor,
        )
        .unwrap();
        let cropped = grab.crop(resolved.bounds).unwrap();
        assert_eq!((cropped.width, cropped.height), (200, 100));
        assert_eq!(cropped.origin, Vec2D::new(100.0, 50.0));
    }

    // ------------------------------------------ capabilities / version

    #[test]
    fn a_macos_without_the_screenshot_api_can_still_enumerate() {
        let old = capabilities_for(false);
        assert!(old.monitor_enumeration);
        assert!(old.window_enumeration);
        assert!(!old.full_desktop);
        assert!(!old.monitor);
        assert!(!old.window);
        assert!(!old.region);
        assert!(old.may_prompt_for_permission);
        assert!(!old.interactive_only);
    }

    #[test]
    fn a_modern_macos_can_capture_every_target() {
        let new = capabilities_for(true);
        for target in [
            CaptureTarget::FullDesktop,
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(1)),
            CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 1.0, 1.0)),
        ] {
            assert!(new.supports(&target), "{target:?}");
        }
    }

    #[test]
    fn an_old_macos_is_told_the_version_it_needs_not_that_the_target_is_odd() {
        // The regression this guards: `Capabilities::ensure_supports` running
        // first turned "you need macOS 14" into "this backend cannot capture a
        // monitor target", which names nothing the user can act on.
        for target in [
            CaptureTarget::FullDesktop,
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(1)),
            CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 1.0, 1.0)),
        ] {
            let err = ensure_capture_supported(false, &target).unwrap_err();
            assert!(matches!(err, CaptureError::Unsupported(_)), "{target:?}");
            let text = err.to_string();
            assert!(text.contains(MIN_SCREENSHOT_MANAGER), "{text}");
            assert!(text.contains("SCScreenshotManager"), "{text}");
            assert!(!text.contains("cannot capture a"), "{text}");
        }
    }

    #[test]
    fn a_modern_macos_passes_the_version_gate() {
        for target in [
            CaptureTarget::FullDesktop,
            CaptureTarget::Monitor(MonitorId::new(1)),
            CaptureTarget::Window(WindowId::new(1)),
            CaptureTarget::region(Rect::from_xywh(0.0, 0.0, 1.0, 1.0)),
        ] {
            assert!(
                ensure_capture_supported(true, &target).is_ok(),
                "{target:?}"
            );
        }
    }

    // ------------------------------------------- stream configuration

    #[test]
    fn a_stream_is_sized_in_physical_pixels() {
        assert_eq!(stream_pixel_size(2.0, 1512.0, 982.0).unwrap(), (3024, 1964));
        assert_eq!(
            stream_pixel_size(1.0, 1920.0, 1080.0).unwrap(),
            (1920, 1080)
        );
        // Fractional scales round to whole pixels.
        assert_eq!(
            stream_pixel_size(1.5, 1707.0, 1067.0).unwrap(),
            (2561, 1601)
        );
    }

    #[test]
    fn a_nonsense_filter_size_is_an_error_not_a_1920x1080_default() {
        // Every one of these used to leave `SCStreamConfiguration` untouched,
        // and Apple documents that default as 1920x1080 -- a silently
        // mis-sized grab stamped with the display's real origin and scale.
        for (scale, width, height) in [
            (0.0, 1512.0, 982.0),
            (-2.0, 1512.0, 982.0),
            (f64::NAN, 1512.0, 982.0),
            (f64::INFINITY, 1512.0, 982.0),
            (2.0, 0.0, 982.0),
            (2.0, 1512.0, 0.0),
            (2.0, f64::NAN, 982.0),
            (2.0, 1512.0, f64::INFINITY),
            (2.0, -1512.0, 982.0),
        ] {
            let result = stream_pixel_size(scale, width, height);
            assert!(result.is_err(), "{scale} x {width} x {height} was accepted");
            let err = result.unwrap_err();
            assert!(matches!(err, CaptureError::Backend(_)));
            let text = err.to_string();
            assert!(text.contains("1920x1080"), "{text}");
        }
    }

    #[test]
    fn a_sub_pixel_region_is_refused_rather_than_rounded_away() {
        // 0.2 pt at 1x rounds to zero, which is not a capturable size.
        assert!(stream_pixel_size(1.0, 0.2, 100.0).is_err());
        // ...but half a pixel up still rounds to a real one.
        assert_eq!(stream_pixel_size(1.0, 0.6, 100.0).unwrap(), (1, 100));
    }

    // ------------------------------------------------ CGImage layout

    #[test]
    fn little_endian_32_bit_images_are_the_accepted_layout() {
        assert!(ensure_bgra8(32, BYTE_ORDER_32_LITTLE).is_ok());
        assert_eq!(BYTE_ORDER_32_LITTLE, 0x2000);
    }

    #[test]
    fn a_big_endian_32_bit_image_is_refused_rather_than_silently_recoloured() {
        // kCGImageByteOrder32Big is A,R,G,B in memory; the BGRA decoder would
        // turn that into G,R,A,B and nobody would be told.
        let err = ensure_bgra8(32, 0x4000).unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
        let text = err.to_string();
        assert!(text.contains("kCGImageByteOrder32Big"), "{text}");
        assert!(text.contains("kCGImageByteOrder32Little"), "{text}");
    }

    #[test]
    fn the_default_byte_order_is_not_assumed_to_be_bgra() {
        // kCGImageByteOrderDefault on a 32-bit image is the big-endian
        // component order, not BGRA, so it must not slip through either.
        let err = ensure_bgra8(32, 0).unwrap_err();
        assert!(
            err.to_string().contains("kCGImageByteOrderDefault"),
            "{err}"
        );
    }

    #[test]
    fn a_non_32_bit_image_is_still_refused_first() {
        let err = ensure_bgra8(64, BYTE_ORDER_32_LITTLE).unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
        assert!(err.to_string().contains("64-bit"), "{err}");
    }

    #[test]
    fn an_unrecognised_byte_order_is_reported_by_value() {
        let err = ensure_bgra8(32, 0x7000).unwrap_err();
        assert!(err.to_string().contains("0x7000"), "{err}");
    }

    // -------------------------------------------------- error mapping

    #[test]
    fn a_declined_permission_maps_to_permission_denied_with_the_settings_path() {
        let err = classify_sc_error("SCStreamErrorDomain", -3801, "user declined");
        assert!(err.is_permission_denied());
        let text = err.to_string();
        assert!(text.contains("Screen & System Audio Recording"), "{text}");
        assert!(text.contains("System Settings"), "{text}");
        assert!(text.contains("relaunch"), "{text}");
    }

    #[test]
    fn missing_entitlements_are_also_a_permission_problem() {
        assert!(
            classify_sc_error("SCStreamErrorDomain", -3803, "no entitlement")
                .is_permission_denied()
        );
    }

    #[test]
    fn a_user_stopped_capture_is_a_cancellation_not_a_failure() {
        let err = classify_sc_error("SCStreamErrorDomain", -3817, "stopped");
        assert!(err.is_cancelled());
        assert!(err.guidance().is_none());
    }

    #[test]
    fn an_empty_display_list_is_reported_as_a_stale_permission() {
        for code in [-3813, -3814, -3815] {
            let err = classify_sc_error("SCStreamErrorDomain", code, "");
            assert!(err.is_permission_denied(), "code {code}");
            assert!(err.to_string().contains("relaunch"), "code {code}");
        }
    }

    #[test]
    fn unknown_screencapturekit_codes_fall_back_to_backend() {
        let err = classify_sc_error("SCStreamErrorDomain", -3811, "internal");
        assert!(matches!(err, CaptureError::Backend(_)));
        assert!(err.to_string().contains("-3811"));
        assert!(err.to_string().contains("internal"));
    }

    #[test]
    fn errors_from_other_domains_are_never_misread_as_denials() {
        // NSOSStatusErrorDomain -3801 means something else entirely.
        let err = classify_sc_error("NSOSStatusErrorDomain", -3801, "boom");
        assert!(matches!(err, CaptureError::Backend(_)));
        assert!(!err.is_permission_denied());
        assert!(err.to_string().contains("NSOSStatusErrorDomain"));
    }

    #[test]
    fn the_permission_message_tells_the_user_exactly_where_to_go() {
        let err = permission_denied("checked before capture");
        assert!(err.is_permission_denied());
        let text = err.to_string();
        assert!(text.contains(SETTINGS_PATH), "{text}");
        assert!(text.contains("checked before capture"), "{text}");
        // And the shared guidance line still points at the same place.
        assert!(err.guidance().unwrap().contains("Screen"));
    }

    #[test]
    fn the_stale_permission_message_distinguishes_itself_from_a_denial() {
        let stale = stale_permission().to_string();
        assert!(stale.contains("returned no displays"), "{stale}");
        assert!(stale.contains("relaunch"), "{stale}");
    }

    #[test]
    fn an_old_macos_is_unsupported_and_names_the_version_it_needs() {
        let err = too_old("SCScreenshotManager", MIN_SCREENSHOT_MANAGER);
        assert!(matches!(err, CaptureError::Unsupported(_)));
        let text = err.to_string();
        assert!(text.contains("14.0"), "{text}");
        assert!(text.contains("SCScreenshotManager"), "{text}");

        let older = too_old("ScreenCaptureKit", MIN_SCREEN_CAPTURE_KIT).to_string();
        assert!(older.contains("12.3"), "{older}");
    }

    #[test]
    fn a_timeout_says_what_it_was_waiting_for_and_how_long() {
        let err = timed_out("capturing a monitor", Duration::from_secs(15));
        assert!(matches!(err, CaptureError::Backend(_)));
        let text = err.to_string();
        assert!(text.contains("capturing a monitor"), "{text}");
        assert!(text.contains("15"), "{text}");
        assert!(text.contains(SETTINGS_PATH), "{text}");
    }
}
