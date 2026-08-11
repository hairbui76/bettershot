//! Windows: Windows Graphics Capture / DXGI, via the `xcap` crate.
//!
//! `xcap` wraps `Graphics.Capture` (with a GDI/DXGI path for older builds) plus
//! the Win32 monitor and window enumeration APIs, and cross-compiles cleanly
//! for `x86_64-pc-windows-msvc` from Linux because its bindings are pure Rust.
//! Using it here keeps bettershot out of the business of maintaining WinRT
//! interop by hand.
//!
//! Caveats specific to Windows:
//!
//! * **Per-monitor DPI awareness is the host process's job.** Win32 reports
//!   monitor rectangles in *virtual-screen* coordinates that are only physical
//!   pixels when the process is per-monitor-DPI-aware (v2). The bettershot app
//!   shell declares that in its manifest; a host that does not will get
//!   system-DPI-scaled geometry from this backend and mixed-DPI layouts will be
//!   wrong. Nothing this crate can detect or fix.
//! * **Monitors left of / above the primary have negative coordinates**, which
//!   is exactly what the virtual-desktop helpers in this crate are built for.
//! * **`xcap` orders windows with larger `z` nearer the viewer**; bettershot
//!   uses 0 = frontmost, so the list is re-indexed on the way through.
//! * **There is no whole-virtual-desktop grab.** Full-desktop and cross-monitor
//!   region captures are assembled from per-monitor grabs with
//!   [`crate::stitch`], which also fills the gaps of a non-rectangular layout
//!   with transparency.
//! * Capture needs no permission prompt on Windows.

use bettershot_core::Rect;
use xcap::{Monitor as XMonitor, Window as XWindow, XCapError};

use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureTarget, CursorImage, MonitorId, MonitorInfo,
    RawFrame, VirtualDesktop, WindowId, WindowInfo, stitch, target::resolve_target,
};

/// Windows Graphics Capture backend.
pub(crate) struct WindowsBackend;

impl WindowsBackend {
    pub(crate) fn new() -> Self {
        Self
    }

    fn describe_monitor(monitor: &XMonitor) -> Result<MonitorInfo, CaptureError> {
        Ok(MonitorInfo::new(
            MonitorId::new(u64::from(monitor.id().map_err(map_xcap)?)),
            monitor.name().map_err(map_xcap)?,
            Rect::from_xywh(
                monitor.x().map_err(map_xcap)? as f32,
                monitor.y().map_err(map_xcap)? as f32,
                monitor.width().map_err(map_xcap)? as f32,
                monitor.height().map_err(map_xcap)? as f32,
            ),
            monitor.scale_factor().map_err(map_xcap)?,
            monitor.is_primary().map_err(map_xcap)?,
        ))
    }

    fn find_monitor(id: MonitorId) -> Result<XMonitor, CaptureError> {
        XMonitor::all()
            .map_err(map_xcap)?
            .into_iter()
            .find(|m| {
                m.id()
                    .map(|raw| u64::from(raw) == id.get())
                    .unwrap_or(false)
            })
            .ok_or(CaptureError::NoSuchMonitor(id))
    }

    fn find_window(id: WindowId) -> Result<XWindow, CaptureError> {
        XWindow::all()
            .map_err(map_xcap)?
            .into_iter()
            .find(|w| {
                w.id()
                    .map(|raw| u64::from(raw) == id.get())
                    .unwrap_or(false)
            })
            .ok_or(CaptureError::NoSuchWindow(id))
    }

    /// Grab one monitor in full.
    fn capture_monitor(monitor: &XMonitor, info: &MonitorInfo) -> Result<RawFrame, CaptureError> {
        let image = monitor.capture_image().map_err(map_xcap)?;
        let (width, height) = (image.width(), image.height());
        RawFrame::new(
            width,
            height,
            image.into_raw(),
            info.origin(),
            info.scale_factor,
        )
    }

    /// Grab every monitor overlapping `bounds` and stitch them together. The
    /// result covers the union of those monitors, which may be larger than
    /// `bounds`; callers crop afterwards.
    fn capture_covering(desktop: &VirtualDesktop, bounds: Rect) -> Result<RawFrame, CaptureError> {
        let wanted: Vec<&MonitorInfo> = desktop
            .monitors()
            .iter()
            .filter(|m| !m.bounds.clamped_to(bounds).is_empty())
            .collect();
        if wanted.is_empty() {
            return Err(CaptureError::EmptyRegion);
        }

        let handles = XMonitor::all().map_err(map_xcap)?;
        let mut frames = Vec::with_capacity(wanted.len());
        for info in wanted {
            let handle = handles
                .iter()
                .find(|m| {
                    m.id()
                        .map(|raw| u64::from(raw) == info.id.get())
                        .unwrap_or(false)
                })
                .ok_or(CaptureError::NoSuchMonitor(info.id))?;
            frames.push(Self::capture_monitor(handle, info)?);
        }
        stitch(&frames)
    }
}

impl CaptureBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows-graphics-capture"
    }

    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        XMonitor::all()
            .map_err(map_xcap)?
            .iter()
            .map(Self::describe_monitor)
            .collect()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        let mut described = Vec::new();
        for window in XWindow::all().map_err(map_xcap)? {
            // Windows come and go while we enumerate; skip the ones that
            // vanish rather than failing the whole call.
            let Ok(id) = window.id() else { continue };
            let Ok(z) = window.z() else { continue };
            let (Ok(x), Ok(y), Ok(width), Ok(height)) =
                (window.x(), window.y(), window.width(), window.height())
            else {
                continue;
            };
            described.push((
                z,
                WindowInfo::new(
                    WindowId::new(u64::from(id)),
                    window.title().unwrap_or_default(),
                    window.app_name().unwrap_or_default(),
                    Rect::from_xywh(x as f32, y as f32, width as f32, height as f32),
                    window.is_minimized().unwrap_or(false),
                    0,
                ),
            ));
        }

        // xcap: larger z is nearer the viewer. bettershot: 0 is frontmost.
        described.sort_by_key(|(z, _)| std::cmp::Reverse(*z));
        Ok(described
            .into_iter()
            .enumerate()
            .map(|(index, (_, mut info))| {
                info.z_order = index as u32;
                info
            })
            .collect())
    }

    fn capture(&self, target: CaptureTarget) -> Result<RawFrame, CaptureError> {
        let desktop = VirtualDesktop::new(self.monitors()?);

        if let CaptureTarget::Window(id) = target {
            // Grab the window itself rather than cropping the desktop: Windows
            // Graphics Capture can reach content that is partly occluded.
            let windows = self.windows()?;
            let info = crate::window::require(&windows, id)?;
            let resolved = resolve_target(target, &desktop, &windows)?;
            let handle = Self::find_window(id)?;
            let image = handle.capture_image().map_err(map_xcap)?;
            let (width, height) = (image.width(), image.height());
            return RawFrame::new(
                width,
                height,
                image.into_raw(),
                info.bounds.pos,
                resolved.scale_factor,
            );
        }

        let resolved = resolve_target(target, &desktop, &[])?;
        match target {
            CaptureTarget::Monitor(id) => {
                let info = desktop.require(id)?;
                Self::capture_monitor(&Self::find_monitor(id)?, info)
            }
            _ => {
                let covering = Self::capture_covering(&desktop, resolved.bounds)?;
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
        Capabilities::FULL
    }

    fn cursor(&self) -> Result<Option<CursorImage>, CaptureError> {
        // `xcap` calls `SetIsCursorCaptureEnabled(false)` and will not hand the
        // bitmap back, so this goes to Win32 directly. Like the rest of the
        // cursor work, only the handle management is platform-specific: the
        // decoding lives in `crate::cursor` and is tested everywhere.
        super::windows_cursor::current_cursor()
    }
}

fn map_xcap(err: XCapError) -> CaptureError {
    match err {
        XCapError::NotSupported => CaptureError::unsupported(
            "this Windows build does not support the requested capture operation",
        ),
        XCapError::InvalidCaptureRegion(message) => {
            CaptureError::invalid_frame(format!("Windows rejected the capture region: {message}"))
        }
        other => CaptureError::backend(format!("Windows capture: {other}")),
    }
}
