//! Per-platform backend implementations and the factories that build them.
//!
//! Everything OS-specific in bettershot lives under this module, behind
//! `#[cfg(target_os = ...)]`. The factories below exist for every target so
//! [`crate::backend_for`] compiles unconditionally; asking for a backend that
//! does not exist on this OS is a normal [`CaptureError::Unsupported`], not a
//! compile error.

use crate::{CaptureBackend, CaptureError};

pub(crate) mod stub;

// Not `cfg`-gated: the macOS module's platform-independent half (geometry,
// stride unpacking, z-ordering, error classification) compiles and is unit
// tested on every host. Only its `sck` submodule is macOS-only.
pub(crate) mod macos;

#[cfg(target_os = "linux")]
pub(crate) mod portal;
#[cfg(target_os = "linux")]
pub(crate) mod x11;

#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "windows")]
pub(crate) mod windows_cursor;

/// The Wayland `xdg-desktop-portal` Screenshot backend.
pub(crate) fn new_wayland_portal() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(portal::PortalBackend::new()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(CaptureError::unsupported(
            "the Wayland screenshot portal backend is only available on Linux",
        ))
    }
}

/// The direct X11 backend.
pub(crate) fn new_x11() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(x11::X11Backend::connect()?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(CaptureError::unsupported(
            "the X11 backend is only available on Linux",
        ))
    }
}

/// The Windows Graphics Capture / DXGI backend.
pub(crate) fn new_windows() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsBackend::new()))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CaptureError::unsupported(
            "the Windows Graphics Capture backend is only available on Windows",
        ))
    }
}

/// The macOS ScreenCaptureKit backend.
///
/// Always returns *some* backend, on every platform: on macOS the real thing,
/// and otherwise (or on a macOS too old for ScreenCaptureKit) a placeholder
/// that refuses with the reason. That keeps the app shell able to construct a
/// backend and show its name in every case, and keeps the refusal path
/// unit-testable from the Linux and Windows build machines.
pub(crate) fn new_macos() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    #[cfg(target_os = "macos")]
    {
        match macos::sck::MacOsBackend::new() {
            Ok(backend) => Ok(Box::new(backend)),
            Err(reason) => Ok(Box::new(stub::macos_unavailable(reason.to_string()))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Box::new(stub::macos_unavailable(
            "the ScreenCaptureKit backend is only available on macOS",
        )))
    }
}
