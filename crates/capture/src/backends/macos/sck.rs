//! The Objective-C half of the macOS backend: ScreenCaptureKit and
//! CoreGraphics.
//!
//! **Compile-verified against the real ScreenCaptureKit API, never run on a
//! Mac.** See the [parent module](super) for the first-run checklist. Every
//! decision that does not need Apple types was pushed up into that module so it
//! could be unit tested on a Linux build machine; what is left here is the
//! adapter.
//!
//! # Which API, and why
//!
//! * Enumeration is `SCShareableContent`
//!   (`getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:`).
//!   It is the only supported way to list displays and windows since
//!   `CGWindowListCopyWindowInfo` was deprecated, and — importantly — it is the
//!   call that fails when Screen Recording has not been granted, which makes it
//!   the natural place to detect the permission state for real rather than
//!   trusting the TCC preflight.
//! * Capture is `SCScreenshotManager`
//!   (`captureImageWithFilter:configuration:completionHandler:`), which returns
//!   a single `CGImage`. `SCStream` is the continuous-capture API: it needs a
//!   delegate object, a dispatch queue, start/stop handshakes and frame
//!   dropping logic, all to throw away every frame but one. For a screenshot
//!   tool `SCScreenshotManager` is simply the right call.
//! * Permission is `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`.
//!
//! # Bridging the completion handlers
//!
//! Both APIs are asynchronous and hand their result to a block, while
//! [`crate::CaptureBackend`] is synchronous. Each call therefore:
//!
//! 1. creates a `std::sync::mpsc::sync_channel(1)`,
//! 2. moves the sender into a `block2::RcBlock`,
//! 3. issues the ScreenCaptureKit call, and
//! 4. blocks on [`std::sync::mpsc::Receiver::recv_timeout`].
//!
//! The wait is **bounded** ([`CONTENT_TIMEOUT`], [`CAPTURE_TIMEOUT`]): a
//! screenshot backend has no window of its own to cancel from, so a handler
//! that never fires would hang the process with nothing the user could do about
//! it. A timeout becomes a [`CaptureError`] naming the operation instead.
//!
//! What crosses the channel is deliberately plain data wherever possible — the
//! capture handler decodes the `CGImage` into an RGBA `Vec<u8>` *inside* the
//! block, so only `Send` types travel. The one exception is
//! `SCShareableContent`, whose `SCDisplay`/`SCWindow` objects are needed later
//! to build content filters; it travels in [`SendShareableContent`], a wrapper
//! with a hand-written `Send` for that one class (see its safety comment).
//!
//! # Assumptions that only real hardware can confirm
//!
//! * ScreenCaptureKit invokes these handlers on its own internal dispatch
//!   queue, not the main queue. If it ever used the main queue, calling a
//!   capture from the main thread would deadlock until the timeout fires
//!   (which is exactly why the timeout exists). Callers should run captures on
//!   a worker thread regardless — `CaptureBackend` is `Send` for that reason.
//! * `SCDisplay.frame` and `SCWindow.frame` are in points in CoreGraphics'
//!   top-left-origin global display space, matching bettershot's axis
//!   convention. AppKit's `NSScreen.frame` is bottom-left origin; these are
//!   documented as `CGRect`s and are believed not to be.
//! * `SCShareableContent.windows` is front-to-back within a window level.

// The whole point of this module is calling Objective-C; see the crate's
// Cargo.toml for why the crate-wide lint is `deny` rather than `forbid`. No
// other file in `bettershot-capture` contains an `unsafe` block.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use bettershot_core::Rect;
use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{
    CGDataProvider, CGDisplayCopyDisplayMode, CGDisplayMode, CGImage, CGMainDisplayID,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration, SCWindow,
};

use super::{
    DisplayScale, MIN_SCREEN_CAPTURE_KIT, NAME, WindowRecord, bgra_rows_to_rgba, capabilities_for,
    display_name, ensure_bgra8, ensure_capture_supported, monitor_from_display, monitors_covering,
    permission_denied, stale_permission, stream_pixel_size, timed_out, too_old,
    window_points_to_physical, windows_front_to_back,
};
use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureTarget, MonitorId, MonitorInfo, RawFrame,
    VirtualDesktop, WindowId, WindowInfo, resolve_target, stitch,
};

/// How long to wait for `SCShareableContent`. Enumeration is fast when the
/// permission is settled; a long stall means it is not.
const CONTENT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait for a screenshot. Generous: a 6K display on a busy machine
/// is not instant, and a spurious timeout is worse than a slow capture.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

/// Move a retained `SCShareableContent` from ScreenCaptureKit's callback thread
/// to the thread waiting on the channel.
///
/// Deliberately monomorphic. `Send` is a property of a *specific* Objective-C
/// class, never of `Retained<T>` in general, so there is nothing here to reuse
/// for another type: a blanket `unsafe impl<T> Send` would be a claim about
/// classes nobody has examined.
///
/// # Safety of the `Send` impl
///
/// Moving a `Retained<T>` between threads needs three things, and "ownership is
/// transferred so only one thread touches it" only supplies the third:
///
/// 1. **The reference count is thread-safe.** Objective-C `retain`/`release`
///    are atomic on all Apple platforms, so the count itself cannot be torn by
///    the handoff.
/// 2. **`dealloc` has no thread affinity.** This is the requirement move-only
///    transfer does *not* address, and the reason so many Cocoa classes are
///    `!Send`: releasing the last reference to, say, an `NSView` off the main
///    thread is undefined behaviour no matter how carefully it was moved. The
///    receiving thread here is the one that drops the value, so it must be
///    legal to deallocate it anywhere. `SCShareableContent` is an immutable
///    container of `SCDisplay` / `SCWindow` / `SCRunningApplication` value
///    objects, none of which is a UI object or holds one; more decisively,
///    ScreenCaptureKit *constructs and hands it to the caller on its own
///    private dispatch queue* rather than the main queue, so a main-thread
///    deallocation requirement would make Apple's own documented usage
///    unsound.
/// 3. **No aliasing across threads.** The block builds the value, sends it, and
///    never touches it again; nothing else holds a reference. `SCShareableContent`
///    is also read-only after construction, so there is no interior mutation to
///    synchronise.
///
/// Point 2 rests on ScreenCaptureKit's documented delivery queue rather than on
/// anything the compiler can check, which is why this wrapper is private to
/// this module and why the receiving thread does all the work with the content
/// before dropping it.
struct SendShareableContent(Retained<SCShareableContent>);

// SAFETY: `SCShareableContent` is an immutable, non-UI container that
// ScreenCaptureKit itself creates and releases off the main thread, its
// reference count is atomic, and this wrapper only ever moves a uniquely-owned
// value. See the type's documentation for the full argument.
unsafe impl Send for SendShareableContent {}

/// macOS screen capture through ScreenCaptureKit.
pub(crate) struct MacOsBackend {
    /// Whether `SCScreenshotManager` (macOS 14.0+) exists. Enumeration works on
    /// 12.3+, so the two are checked separately and the backend degrades to
    /// "can list, cannot grab" rather than refusing outright.
    can_screenshot: bool,
}

impl MacOsBackend {
    /// Build the backend, or explain why ScreenCaptureKit is unusable here.
    ///
    /// The class lookups are the only available runtime version check: Rust has
    /// no equivalent of `@available`, and the framework's symbols are resolved
    /// eagerly. (A distribution build should link ScreenCaptureKit *weakly* so
    /// that a pre-12.3 macOS reaches this check instead of failing to launch;
    /// that is a link-flag decision for the app shell, not something this crate
    /// can express.)
    pub(crate) fn new() -> Result<Self, CaptureError> {
        if !class_exists(c"SCShareableContent") {
            return Err(too_old("ScreenCaptureKit", MIN_SCREEN_CAPTURE_KIT));
        }
        Ok(Self {
            can_screenshot: class_exists(c"SCScreenshotManager"),
        })
    }

    /// Everything ScreenCaptureKit knows, converted into bettershot's types.
    fn snapshot(&self) -> Result<Snapshot, CaptureError> {
        ensure_permission()?;
        let content = shareable_content()?;
        Snapshot::from_content(content)
    }

    /// Grab one whole display.
    fn grab_display(&self, entry: &DisplayEntry) -> Result<RawFrame, CaptureError> {
        let filter = display_filter(&entry.display);
        let image = capture_image(&filter, false)?;
        RawFrame::new(
            image.width,
            image.height,
            image.rgba,
            entry.info.origin(),
            entry.info.scale_factor,
        )
    }

    /// Grab every display overlapping `bounds` and compose them.
    ///
    /// Like the Windows backend, macOS has no whole-virtual-desktop grab that
    /// bettershot can use: every `SCContentFilter` display initialiser names
    /// exactly one `SCDisplay`, and `SCScreenshotManager`'s multi-display
    /// `captureImageInRect:completionHandler:` takes its rect in *points* with
    /// no `SCStreamConfiguration` to size the result. See [`monitors_covering`]
    /// for the full argument. [`stitch`] fills the gaps of an L-shaped layout
    /// with transparency.
    fn grab_covering(
        &self,
        snapshot: &Snapshot,
        desktop: &VirtualDesktop,
        bounds: Rect,
    ) -> Result<RawFrame, CaptureError> {
        let wanted = monitors_covering(desktop, bounds);
        if wanted.is_empty() {
            return Err(CaptureError::EmptyRegion);
        }
        let mut frames = Vec::with_capacity(wanted.len());
        for id in wanted {
            frames.push(self.grab_display(snapshot.display(id)?)?);
        }
        stitch(&frames)
    }

    /// Grab one window, shadow and all the surrounding desktop excluded.
    fn grab_window(
        &self,
        snapshot: &Snapshot,
        info: &WindowInfo,
        scale_factor: f32,
    ) -> Result<RawFrame, CaptureError> {
        let window = snapshot.window(info.id)?;
        let filter = window_filter(&window);
        let image = capture_image(&filter, true)?;
        RawFrame::new(
            image.width,
            image.height,
            image.rgba,
            // The *unclipped* window origin: ScreenCaptureKit renders the whole
            // window even where it hangs off the edge of a display, so cropping
            // the origin to the desktop would misplace the frame.
            info.bounds.pos,
            scale_factor,
        )
    }
}

impl CaptureBackend for MacOsBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(self.snapshot()?.monitors())
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        Ok(self.snapshot()?.windows)
    }

    fn capture(&self, target: CaptureTarget) -> Result<RawFrame, CaptureError> {
        // Version first, target kind second: see `ensure_capture_supported`.
        ensure_capture_supported(self.can_screenshot, &target)?;

        let snapshot = self.snapshot()?;
        let desktop = VirtualDesktop::new(snapshot.monitors());
        let resolved = resolve_target(target, &desktop, &snapshot.windows)?;

        match target {
            CaptureTarget::Window(id) => {
                let info = crate::window::require(&snapshot.windows, id)?;
                self.grab_window(&snapshot, info, resolved.scale_factor)
            }
            CaptureTarget::Monitor(id) => self.grab_display(snapshot.display(id)?),
            // FullDesktop and Region alike: grab the display(s) the request
            // touches, then cut the request out of the result.
            CaptureTarget::FullDesktop | CaptureTarget::Region { .. } => {
                let covering = self.grab_covering(&snapshot, &desktop, resolved.bounds)?;
                if covering.bounds() == resolved.bounds {
                    Ok(covering)
                } else {
                    let mut cropped = covering.crop(resolved.bounds)?;
                    cropped.scale_factor = resolved.scale_factor;
                    Ok(cropped)
                }
            }
        }
    }

    fn capabilities(&self) -> Capabilities {
        capabilities_for(self.can_screenshot)
    }
}

// ---------------------------------------------------------------- enumeration

/// One display, kept alongside the `SCDisplay` needed to build a filter for it.
struct DisplayEntry {
    display: Retained<SCDisplay>,
    info: MonitorInfo,
    frame_points: Rect,
}

/// A single consistent view of the displays and windows, so a capture never
/// mixes geometry from two different enumerations.
struct Snapshot {
    content: Retained<SCShareableContent>,
    displays: Vec<DisplayEntry>,
    windows: Vec<WindowInfo>,
}

impl Snapshot {
    fn from_content(content: Retained<SCShareableContent>) -> Result<Self, CaptureError> {
        let displays = read_displays(&content);
        if displays.is_empty() {
            // TCC said yes but ScreenCaptureKit has nothing: the permission was
            // granted after this process started.
            return Err(stale_permission());
        }
        let windows = read_windows(&content, &displays);
        Ok(Self {
            content,
            displays,
            windows,
        })
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        self.displays.iter().map(|d| d.info.clone()).collect()
    }

    fn display(&self, id: MonitorId) -> Result<&DisplayEntry, CaptureError> {
        self.displays
            .iter()
            .find(|d| d.info.id == id)
            .ok_or(CaptureError::NoSuchMonitor(id))
    }

    /// The `SCWindow` behind a [`WindowId`]. Re-read from the retained content
    /// rather than cached, because `SCWindow` is not `Clone`.
    fn window(&self, id: WindowId) -> Result<Retained<SCWindow>, CaptureError> {
        let windows = unsafe { self.content.windows() };
        for index in 0..windows.count() {
            let window = windows.objectAtIndex(index);
            if u64::from(unsafe { window.windowID() }) == id.get() {
                return Ok(window);
            }
        }
        Err(CaptureError::NoSuchWindow(id))
    }
}

fn read_displays(content: &SCShareableContent) -> Vec<DisplayEntry> {
    let main = CGMainDisplayID();
    let displays = unsafe { content.displays() };
    let mut out = Vec::with_capacity(displays.count());
    for index in 0..displays.count() {
        let display = displays.objectAtIndex(index);
        let display_id = unsafe { display.displayID() };
        let frame_points = cg_rect(unsafe { display.frame() });
        let is_primary = display_id == main;
        let info = monitor_from_display(
            display_id,
            display_name(display_id, is_primary),
            frame_points,
            display_pixel_size(display_id),
            is_primary,
        );
        out.push(DisplayEntry {
            display,
            info,
            frame_points,
        });
    }
    out
}

fn read_windows(content: &SCShareableContent, displays: &[DisplayEntry]) -> Vec<WindowInfo> {
    let scales: Vec<DisplayScale> = displays
        .iter()
        .map(|d| DisplayScale {
            frame_points: d.frame_points,
            scale: d.info.scale_factor,
        })
        .collect();
    let fallback = scales.iter().map(|s| s.scale).fold(1.0_f32, f32::max);

    let windows = unsafe { content.windows() };
    let mut records = Vec::with_capacity(windows.count());
    for index in 0..windows.count() {
        let window = windows.objectAtIndex(index);
        let frame_points = cg_rect(unsafe { window.frame() });
        let app_name = unsafe { window.owningApplication() }
            .map(|app| unsafe { app.applicationName() }.to_string())
            .unwrap_or_default();
        records.push(WindowRecord {
            id: u64::from(unsafe { window.windowID() }),
            title: unsafe { window.title() }
                .map(|t| t.to_string())
                .unwrap_or_default(),
            app_name,
            bounds: window_points_to_physical(frame_points, &scales, fallback),
            layer: unsafe { window.windowLayer() } as i64,
            on_screen: unsafe { window.isOnScreen() },
        });
    }
    windows_front_to_back(records)
}

/// The display's size in real pixels, or `(0, 0)` when the display mode cannot
/// be read.
///
/// `SCDisplay` reports points only, so both the backing scale and the true
/// pixel size have to come from the current `CGDisplayMode`:
/// `CGDisplayModeGetPixelWidth` / `GetPixelHeight` are the framebuffer's own
/// dimensions, `SCDisplay.frame` is the same display in points, and the width
/// ratio is the scale factor. [`monitor_from_display`] handles the `(0, 0)`
/// case by falling back to the point size, which yields a scale of 1.0 — wrong
/// on Retina, but never a division by zero or a wildly mis-sized grab.
fn display_pixel_size(display_id: u32) -> (u32, u32) {
    let Some(mode) = CGDisplayCopyDisplayMode(display_id) else {
        return (0, 0);
    };
    let width = CGDisplayMode::pixel_width(Some(&mode));
    let height = CGDisplayMode::pixel_height(Some(&mode));
    if width > 0 && height > 0 && width <= u32::MAX as usize && height <= u32::MAX as usize {
        (width as u32, height as u32)
    } else {
        (0, 0)
    }
}

// ------------------------------------------------------------------ capturing

/// The pixels a completion handler produced, in a form that can cross a thread
/// boundary.
struct CapturedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// A filter covering one whole display, dock and desktop picture included.
fn display_filter(display: &SCDisplay) -> Retained<SCContentFilter> {
    let nothing_excluded: Retained<NSArray<SCWindow>> = NSArray::new();
    unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            display,
            &nothing_excluded,
        )
    }
}

/// A filter covering one window and nothing else — no desktop behind it.
fn window_filter(window: &SCWindow) -> Retained<SCContentFilter> {
    unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window) }
}

/// Build the stream configuration for a filter.
///
/// The width and height are set in **physical pixels**, from
/// [`stream_pixel_size`] over the filter's own `contentRect` (points) and
/// `pointPixelScale`. They are always set: `SCStreamConfiguration` defaults to
/// 1920x1080, so a filter that reports nonsense is an error rather than a
/// silently mis-sized grab.
fn configuration(
    filter: &SCContentFilter,
    single_window: bool,
) -> Result<Retained<SCStreamConfiguration>, CaptureError> {
    let (width, height) = filter_pixel_size(filter)?;
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(width);
        config.setHeight(height);
        // Screenshots of the pointer are almost never wanted, and the pointer
        // moves between the request and the grab anyway.
        config.setShowsCursor(false);
        // Render at the display's own resolution rather than downscaling.
        config.setCaptureResolution(SCCaptureResolutionType::Best);
        config.setScalesToFit(false);
        config.setPreservesAspectRatio(true);
        if single_window {
            // Window grabs should be the window, not the window plus the
            // system-drawn drop shadow and its rounded-corner clipping.
            config.setIgnoreShadowsSingleWindow(true);
            config.setIgnoreGlobalClipSingleWindow(true);
        } else {
            config.setIgnoreShadowsDisplay(false);
            config.setIgnoreGlobalClipDisplay(false);
        }
    }
    Ok(config)
}

/// The filter's content size in physical pixels, or the reason it cannot be
/// determined. The arithmetic lives in [`stream_pixel_size`] so it is testable
/// without a Mac.
fn filter_pixel_size(filter: &SCContentFilter) -> Result<(usize, usize), CaptureError> {
    let scale = f64::from(unsafe { filter.pointPixelScale() });
    let rect = unsafe { filter.contentRect() };
    stream_pixel_size(scale, rect.size.width, rect.size.height)
}

/// Run one `SCScreenshotManager` capture and block until it produces pixels.
fn capture_image(
    filter: &SCContentFilter,
    single_window: bool,
) -> Result<CapturedImage, CaptureError> {
    let config = configuration(filter, single_window)?;
    let (tx, rx) = mpsc::sync_channel::<Result<CapturedImage, CaptureError>>(1);
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // Decode inside the block so only plain bytes cross the channel.
        let _ = tx.send(decode_result(image, error));
    });

    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            &config,
            Some(&handler),
        );
    }

    wait(rx, CAPTURE_TIMEOUT, "capturing a screenshot")
}

/// Turn the `(CGImage, NSError)` pair a completion handler receives into a
/// result. Runs on ScreenCaptureKit's thread.
fn decode_result(image: *mut CGImage, error: *mut NSError) -> Result<CapturedImage, CaptureError> {
    // SAFETY: `error` is either null or a `+0` `NSError` owned by the caller
    // for the duration of the block; `retain` takes a reference of our own.
    if let Some(error) = unsafe { Retained::retain(error) } {
        return Err(ns_error(&error));
    }
    // SAFETY: as above, for the `CGImage`.
    let image = unsafe { Retained::retain(image) }.ok_or_else(|| {
        CaptureError::backend("ScreenCaptureKit returned neither image nor error")
    })?;
    decode_image(&image)
}

/// Copy a `CGImage`'s pixels out as tightly packed RGBA8.
fn decode_image(image: &CGImage) -> Result<CapturedImage, CaptureError> {
    let image = Some(image);
    let width = CGImage::width(image);
    let height = CGImage::height(image);
    let bytes_per_row = CGImage::bytes_per_row(image);
    // Both halves of "32-bit BGRA": the depth *and* the byte order, because
    // `kCGImageByteOrder32Big` is equally 32 bits and would decode to the
    // wrong colours without a word of warning.
    ensure_bgra8(
        CGImage::bits_per_pixel(image),
        CGImage::byte_order_info(image).0,
    )?;
    let width = u32::try_from(width)
        .map_err(|_| CaptureError::invalid_frame("captured image is wider than u32"))?;
    let height = u32::try_from(height)
        .map_err(|_| CaptureError::invalid_frame("captured image is taller than u32"))?;

    let provider = CGImage::data_provider(image)
        .ok_or_else(|| CaptureError::backend("the captured CGImage has no data provider"))?;
    let data = CGDataProvider::data(Some(&provider))
        .ok_or_else(|| CaptureError::backend("the captured CGImage's pixels could not be read"))?;

    let length = usize::try_from(data.length())
        .map_err(|_| CaptureError::invalid_frame("captured image length is negative"))?;
    let ptr = data.byte_ptr();
    if ptr.is_null() {
        return Err(CaptureError::backend(
            "the captured CGImage's pixel buffer is null",
        ));
    }
    // SAFETY: `ptr` and `length` come from the same live `CFData`, which is
    // retained by `data` for the whole of this borrow, and CoreFoundation
    // guarantees the buffer is `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, length) };

    // The alpha info decides two separate things — whether the fourth byte is
    // real, and whether the colours were scaled by it — so it is passed through
    // rather than collapsed into one flag here.
    let rgba = bgra_rows_to_rgba(
        bytes,
        width,
        height,
        bytes_per_row,
        CGImage::alpha_info(image).0,
    )?;
    Ok(CapturedImage {
        width,
        height,
        rgba,
    })
}

// ----------------------------------------------------------------- permission

/// Check, and if it has never been asked, prompt.
///
/// The three states, and what each does:
///
/// * **Granted** — `CGPreflightScreenCaptureAccess` returns true; nothing else
///   happens and no dialog appears.
/// * **Never asked** — preflight returns false, `CGRequestScreenCaptureAccess`
///   raises the system dialog and returns whether the user allowed it. macOS
///   only shows this dialog *once per app, ever*; after that the call returns
///   immediately.
/// * **Denied (or already asked and refused)** — both return false, and this
///   returns [`CaptureError::PermissionDenied`] naming
///   `System Settings -> Privacy & Security -> Screen & System Audio Recording`
///   and telling the user to relaunch afterwards, because macOS only hands the
///   permission to a process at launch.
fn ensure_permission() -> Result<(), CaptureError> {
    if CGPreflightScreenCaptureAccess() {
        return Ok(());
    }
    log::info!("macOS Screen Recording is not granted yet; requesting it");
    if CGRequestScreenCaptureAccess() {
        // Freshly granted. ScreenCaptureKit usually still needs a relaunch to
        // see real content; `Snapshot::from_content` catches that and says so.
        return Ok(());
    }
    Err(permission_denied(
        "CGPreflightScreenCaptureAccess reported no access and the system \
         dialog was declined or had already been answered",
    ))
}

// ------------------------------------------------------------------- plumbing

/// Fetch `SCShareableContent`, blocking until the completion handler fires.
///
/// Desktop windows (the wallpaper and the icon layer) are excluded:
/// `excludingDesktopWindows: true`.
///
/// Off-screen windows are **included**: `onScreenWindowsOnly: false`. The
/// tempting `true` would give a tidier picker list, but it also makes
/// `SCWindow.isOnScreen` a constant, and `WindowRecord::on_screen` — and so
/// [`WindowInfo::is_minimized`] and every guard built on it — dead code. macOS
/// has no minimised flag of its own, so this is the only signal there is. With
/// `false`, a window minimised to the Dock, hidden with its application, or
/// left on another Space is listed and marked as having no on-screen pixels;
/// [`crate::window_at`] then skips it, and [`resolve_target`] refuses to
/// capture it with a message saying why. That matches what the X11 backend
/// reports from `_NET_WM_STATE_HIDDEN` and keeps the platforms consistent —
/// and it is strictly more honest than silently omitting a window the user can
/// see in their Dock.
fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = mpsc::sync_channel::<Result<SendShareableContent, CaptureError>>(1);
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: both pointers are `+0` values owned by ScreenCaptureKit
            // for the duration of the block; `retain` takes our own reference.
            let outcome = match unsafe { Retained::retain(error) } {
                Some(error) => Err(ns_error(&error)),
                None => unsafe { Retained::retain(content) }
                    .map(SendShareableContent)
                    .ok_or_else(|| {
                        CaptureError::backend(
                            "ScreenCaptureKit returned neither shareable content nor an error",
                        )
                    }),
            };
            let _ = tx.send(outcome);
        },
    );

    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &handler,
        );
    }

    wait(rx, CONTENT_TIMEOUT, "listing displays and windows").map(|content| content.0)
}

/// Block on a completion handler's channel, with a bounded wait.
fn wait<T>(
    rx: mpsc::Receiver<Result<T, CaptureError>>,
    timeout: Duration,
    operation: &str,
) -> Result<T, CaptureError> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(timed_out(operation, timeout)),
        // The block was released without ever being called.
        Err(RecvTimeoutError::Disconnected) => Err(CaptureError::backend(format!(
            "ScreenCaptureKit dropped its completion handler while {operation}"
        ))),
    }
}

/// Convert an `NSError` into a [`CaptureError`] via the pure classifier, so the
/// mapping stays testable without a Mac.
fn ns_error(error: &NSError) -> CaptureError {
    super::classify_sc_error(
        &error.domain().to_string(),
        error.code() as i64,
        &error.localizedDescription().to_string(),
    )
}

/// Is an Objective-C class present in this process?
///
/// Used as the runtime macOS version check: `SCShareableContent` exists from
/// 12.3, `SCScreenshotManager` from 14.0.
fn class_exists(name: &CStr) -> bool {
    AnyClass::get(name).is_some()
}

/// `CGRect` (points, top-left origin) to bettershot's [`Rect`].
fn cg_rect(rect: CGRect) -> Rect {
    Rect::from_xywh(
        rect.origin.x as f32,
        rect.origin.y as f32,
        rect.size.width as f32,
        rect.size.height as f32,
    )
}
