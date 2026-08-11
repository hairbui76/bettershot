//! Reading the Windows mouse cursor, for `--include-cursor`.
//!
//! # Status: compile-verified, never run
//!
//! Like [`crate::backends::macos`], this has never executed on the OS it
//! targets. It is type-checked for `x86_64-pc-windows-msvc` and linked by the
//! `windows-latest` release build, but nobody has looked at the cursor it
//! produces. A first run should check the pointer lands exactly on what it is
//! pointing at (the hotspot arithmetic), that the classic text I-beam is
//! visible rather than invisible (the alpha fallback below), and that repeated
//! captures do not leak GDI handles — `Task Manager`'s *GDI objects* column for
//! the bettershot process should be flat across many captures.
//!
//! # Why this is a separate module
//!
//! It is the only place in the crate besides the macOS bindings that needs
//! `unsafe`, so it is confined here and opts in explicitly. The crate-level
//! lint is `deny`, not `forbid`, precisely so a module like this can exist
//! while unsafe stays rejected everywhere else.
//!
//! Every pixel decision — the AND/XOR mask table, the zero-alpha fallback, the
//! MSB-first bit order, the padded stride — lives in [`crate::cursor`] instead,
//! where it is unit tested on machines that are not Windows. What is left here
//! is handle management and two `GetDIBits` calls.
#![allow(unsafe_code)]

use std::mem::{size_of, zeroed};

use bettershot_core::Vec2D;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HDC, RGBQUAD, ReleaseDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, CopyIcon, DestroyIcon, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
};

use crate::{CaptureError, CursorAnchor, CursorImage};

/// An `HICON` that is destroyed when it goes out of scope.
///
/// `CopyIcon` hands over ownership, and every early return below would
/// otherwise leak it. A guard makes that structurally impossible rather than
/// something to remember at each `?`.
struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `CopyIcon`, is non-null (checked at
        // construction), and is not destroyed anywhere else.
        unsafe { DestroyIcon(self.0) };
    }
}

/// An `HBITMAP` that is deleted when it goes out of scope.
///
/// `GetIconInfo` creates two of these and the caller owns both. Leaking them is
/// the classic bug with this API: it is silent, and only shows up as a process
/// that slowly exhausts its GDI handle quota after a few thousand captures.
struct OwnedBitmap(HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is an `HBITMAP` from `GetIconInfo` that this
            // type owns, and nothing else deletes it.
            unsafe { DeleteObject(self.0 as _) };
        }
    }
}

/// A screen device context, released when it goes out of scope.
struct ScreenDc(HDC);

impl ScreenDc {
    fn get() -> Result<Self, CaptureError> {
        // SAFETY: a null `HWND` asks for the whole screen, which is always a
        // valid request.
        let dc = unsafe { GetDC(std::ptr::null_mut()) };
        if dc.is_null() {
            return Err(CaptureError::backend(
                "could not get a screen device context to read the cursor",
            ));
        }
        Ok(Self(dc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: paired with the `GetDC(null)` above, same thread.
        unsafe { ReleaseDC(std::ptr::null_mut(), self.0) };
    }
}

/// A `BITMAPINFO` with room for the two-entry palette a 1bpp DIB requires.
///
/// `BITMAPINFO` declares `bmiColors` as a single-element array, so passing one
/// to `GetDIBits` for a monochrome bitmap lets it write a second palette entry
/// past the end. This gives it the space it expects.
#[repr(C)]
struct MonochromeBitmapInfo {
    header: BITMAPINFOHEADER,
    colors: [RGBQUAD; 2],
}

/// The current cursor, or `None` when the pointer is hidden.
pub(crate) fn current_cursor() -> Result<Option<CursorImage>, CaptureError> {
    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        // SAFETY: `CURSORINFO` is a plain C struct of integers and handles, so
        // an all-zero value is valid; `cbSize` is set immediately above and is
        // the only field `GetCursorInfo` reads.
        ..unsafe { zeroed() }
    };
    // SAFETY: `info` is a live, correctly sized `CURSORINFO`.
    if unsafe { GetCursorInfo(&mut info) } == 0 {
        return Err(CaptureError::backend("GetCursorInfo failed"));
    }
    if info.flags != CURSOR_SHOWING || info.hCursor.is_null() {
        return Ok(None);
    }

    // Copy it: the shared `HCURSOR` can be swapped out from under us at any
    // moment by whichever window owns the pointer, and `GetIconInfo` on a
    // freed handle is a use-after-free.
    //
    // SAFETY: `info.hCursor` is non-null and valid for the duration of this
    // call.
    let icon = unsafe { CopyIcon(info.hCursor) };
    if icon.is_null() {
        return Err(CaptureError::backend("CopyIcon failed for the cursor"));
    }
    let icon = OwnedIcon(icon);

    // SAFETY: `icon.0` is a live icon this function owns; `ICONINFO` is
    // all-integers-and-handles so a zeroed value is a valid out-parameter.
    let mut icon_info: ICONINFO = unsafe { zeroed() };
    if unsafe { GetIconInfo(icon.0, &mut icon_info) } == 0 {
        return Err(CaptureError::backend("GetIconInfo failed for the cursor"));
    }
    // Take ownership of both bitmaps immediately, before anything can fail.
    let mask = OwnedBitmap(icon_info.hbmMask);
    let color = OwnedBitmap(icon_info.hbmColor);

    let anchor = CursorAnchor::Hotspot {
        position: Vec2D::new(info.ptScreenPos.x as f32, info.ptScreenPos.y as f32),
        xhot: icon_info.xHotspot,
        yhot: icon_info.yHotspot,
    };

    let dc = ScreenDc::get()?;
    let mask_bitmap = describe(mask.0)?;

    if color.0.is_null() {
        // Monochrome: no colour bitmap, and the mask holds the AND rows
        // followed by the XOR rows, so it is twice the cursor's height.
        let height = u32::try_from(mask_bitmap.bmHeight)
            .map_err(|_| CaptureError::invalid_frame("cursor mask has a negative height"))?
            / 2;
        let width = u32::try_from(mask_bitmap.bmWidth)
            .map_err(|_| CaptureError::invalid_frame("cursor mask has a negative width"))?;
        let (bits, stride) = read_mask(&dc, mask.0, width, height * 2)?;
        CursorImage::from_win32_monochrome(width, height, &bits, stride, anchor).map(Some)
    } else {
        let color_bitmap = describe(color.0)?;
        let width = u32::try_from(color_bitmap.bmWidth)
            .map_err(|_| CaptureError::invalid_frame("cursor has a negative width"))?;
        let height = u32::try_from(color_bitmap.bmHeight)
            .map_err(|_| CaptureError::invalid_frame("cursor has a negative height"))?;
        let bgra = read_color(&dc, color.0, width, height)?;
        let (bits, stride) = read_mask(&dc, mask.0, width, height)?;
        CursorImage::from_win32_color(width, height, &bgra, &bits, stride, anchor).map(Some)
    }
}

/// `GetObject` for a bitmap's dimensions.
fn describe(bitmap: HBITMAP) -> Result<BITMAP, CaptureError> {
    // SAFETY: `bitmap` is a live `HBITMAP`, and the buffer is exactly the
    // `BITMAP` that `GetObjectW` documents for one.
    let mut described: BITMAP = unsafe { zeroed() };
    let written = unsafe {
        GetObjectW(
            bitmap as _,
            size_of::<BITMAP>() as i32,
            (&mut described as *mut BITMAP).cast(),
        )
    };
    if written == 0 {
        return Err(CaptureError::backend(
            "GetObject failed for a cursor bitmap",
        ));
    }
    Ok(described)
}

/// Read a 32-bit colour bitmap as top-down BGRA.
fn read_color(
    dc: &ScreenDc,
    bitmap: HBITMAP,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CaptureError> {
    let mut info: BITMAPINFO = unsafe { zeroed() };
    info.bmiHeader = header(width, height, 32);
    let mut out = vec![0u8; width as usize * height as usize * 4];

    // SAFETY: `out` is exactly `width * height * 4` bytes, which is what a
    // 32bpp top-down DIB of these dimensions occupies, and `info` describes
    // precisely that.
    let rows = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            height,
            out.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if rows == 0 {
        return Err(CaptureError::backend(
            "GetDIBits failed for the cursor's colour bitmap",
        ));
    }
    Ok(out)
}

/// Read a 1-bit mask, returning the bits and their padded row stride.
fn read_mask(
    dc: &ScreenDc,
    bitmap: HBITMAP,
    width: u32,
    rows: u32,
) -> Result<(Vec<u8>, usize), CaptureError> {
    // DIB rows are padded to a 4-byte boundary, which for 1bpp is every 32
    // pixels. `crate::cursor` is told the stride rather than inferring it.
    let stride = ((width as usize).div_ceil(32)) * 4;
    let mut info = MonochromeBitmapInfo {
        header: header(width, rows, 1),
        colors: unsafe { zeroed() },
    };
    let mut out = vec![0u8; stride * rows as usize];

    // SAFETY: `out` holds `rows` rows of `stride` bytes, the size a 1bpp
    // top-down DIB of these dimensions occupies, and `info` carries the
    // two-entry palette `GetDIBits` writes for a monochrome bitmap.
    let read = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            rows,
            out.as_mut_ptr().cast(),
            (&mut info as *mut MonochromeBitmapInfo).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
        )
    };
    if read == 0 {
        return Err(CaptureError::backend(
            "GetDIBits failed for the cursor's mask",
        ));
    }
    Ok((out, stride))
}

/// A `BITMAPINFOHEADER` for a top-down, uncompressed DIB.
///
/// The negative height is what asks for top-down. Without it the rows arrive
/// bottom-up and the cursor is drawn upside down.
fn header(width: u32, height: u32, bits_per_pixel: u16) -> BITMAPINFOHEADER {
    BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: bits_per_pixel,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    }
}
