//! The mouse cursor, and drawing it into a captured frame.
//!
//! Screenshots do not include the pointer: compositors draw it on a separate
//! hardware plane, so it never appears in the pixels a capture hands back. A
//! tool that wants `--include-cursor` has to fetch the cursor bitmap separately
//! and blend it in itself.
//!
//! Everything here is pure arithmetic on buffers, deliberately: the only
//! platform-specific part of the feature is *asking* for the cursor, which is
//! one call per backend. The blending, the clipping and the two different
//! conventions platforms use for "where the cursor is" all live here where they
//! can be tested on a machine with no display at all — the same split
//! [`crate::pixels`] uses.
//!
//! # Alpha
//!
//! Cursors are anti-aliased and genuinely translucent at the edges, so unlike
//! [`RawFrame`] they cannot be treated as opaque. [`CursorImage`] stores
//! **premultiplied** RGBA because that is what X11 hands over and what makes
//! the source-over blend exact; `RawFrame` stays straight (non-premultiplied)
//! as documented on the type. The conversion happens once, on the way in.

use bettershot_core::Vec2D;

use crate::{BYTES_PER_PIXEL, CaptureError, RawFrame};

/// Where a platform says the cursor is.
///
/// The two conventions differ by the hotspot — the pixel inside the bitmap that
/// is "the point of the arrow" — and getting them backwards shifts the cursor
/// by a few pixels in a way that looks almost right. Naming both makes the
/// distinction impossible to lose.
///
/// Both platforms bettershot can currently ask are
/// [`CursorAnchor::Hotspot`]-flavoured:
///
/// * X11's `XFixesGetCursorImage` reply is documented as "x and y are the
///   current cursor position", which reads ambiguously, but
///   `ProcXFixesGetCursorImage` in xorg-server assigns the sprite position
///   straight through (`rep->x = x`) and reports `xhot`/`yhot` beside it — so
///   the position is the hotspot's, not the bitmap's corner.
/// * Windows' `GetCursorInfo` reports `ptScreenPos`, the hotspot's screen
///   position, with the offsets coming from `GetIconInfo`.
///
/// [`CursorAnchor::TopLeft`] exists for platforms that pre-adjust, and because
/// it is the honest way to express a bitmap whose corner is already known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorAnchor {
    /// The position is the top-left corner of the bitmap already.
    TopLeft(Vec2D),
    /// The position is the hotspot, which sits at `(xhot, yhot)` inside the
    /// bitmap.
    Hotspot {
        /// Hotspot position on the virtual desktop, in physical pixels.
        position: Vec2D,
        /// Hotspot column within the bitmap.
        xhot: u32,
        /// Hotspot row within the bitmap.
        yhot: u32,
    },
}

impl CursorAnchor {
    /// The bitmap's top-left corner on the virtual desktop.
    fn top_left(self) -> Vec2D {
        match self {
            Self::TopLeft(p) => p,
            Self::Hotspot {
                position,
                xhot,
                yhot,
            } => position - Vec2D::new(xhot as f32, yhot as f32),
        }
    }
}

/// A cursor bitmap and where it belongs on the virtual desktop.
#[derive(Clone, PartialEq)]
pub struct CursorImage {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// RGBA8, row-major, **premultiplied**, `len == width * height * 4`.
    ///
    /// The premultiplied invariant `r, g, b <= a` is enforced by the
    /// constructors, so blending can add the source channel directly without
    /// risking an overflow past 255.
    pub data: Vec<u8>,
    /// Top-left corner on the virtual desktop, in physical pixels. Already
    /// hotspot-adjusted.
    pub position: Vec2D,
}

impl std::fmt::Debug for CursorImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("data", &format_args!("[{} bytes]", self.data.len()))
            .field("position", &self.position)
            .finish()
    }
}

impl CursorImage {
    /// Build from already-premultiplied RGBA8.
    ///
    /// Channels above the alpha value are clamped down to it: a cursor that
    /// violates the premultiplied invariant (some X servers ship one) would
    /// otherwise blend to more than full brightness.
    pub fn from_premultiplied(
        width: u32,
        height: u32,
        mut data: Vec<u8>,
        anchor: CursorAnchor,
    ) -> Result<Self, CaptureError> {
        check_len(width, height, data.len())?;
        for px in data.chunks_exact_mut(BYTES_PER_PIXEL) {
            let a = px[3];
            px[0] = px[0].min(a);
            px[1] = px[1].min(a);
            px[2] = px[2].min(a);
        }
        Ok(Self {
            width,
            height,
            data,
            position: anchor.top_left(),
        })
    }

    /// Build from straight (non-premultiplied) RGBA8, premultiplying on the way
    /// in.
    pub fn from_straight(
        width: u32,
        height: u32,
        mut data: Vec<u8>,
        anchor: CursorAnchor,
    ) -> Result<Self, CaptureError> {
        check_len(width, height, data.len())?;
        for px in data.chunks_exact_mut(BYTES_PER_PIXEL) {
            let a = u32::from(px[3]);
            px[0] = div255(u32::from(px[0]) * a) as u8;
            px[1] = div255(u32::from(px[1]) * a) as u8;
            px[2] = div255(u32::from(px[2]) * a) as u8;
        }
        Ok(Self {
            width,
            height,
            data,
            position: anchor.top_left(),
        })
    }

    /// Build from a buffer of 32-bit `ARGB` words in host byte order, which is
    /// how X11's XFixes extension returns a cursor (premultiplied, alpha in the
    /// high byte).
    ///
    /// Lives here rather than in the X11 backend so the bit-shuffling is
    /// covered by tests that run on every platform.
    pub fn from_argb_words(
        width: u32,
        height: u32,
        words: &[u32],
        anchor: CursorAnchor,
    ) -> Result<Self, CaptureError> {
        let expected = (width as usize).saturating_mul(height as usize);
        if words.len() < expected {
            return Err(CaptureError::invalid_frame(format!(
                "cursor is {}x{} but only {} pixels were returned",
                width,
                height,
                words.len()
            )));
        }
        let mut data = Vec::with_capacity(expected * BYTES_PER_PIXEL);
        for word in &words[..expected] {
            data.push((word >> 16) as u8); // R
            data.push((word >> 8) as u8); // G
            data.push(*word as u8); // B
            data.push((word >> 24) as u8); // A
        }
        Self::from_premultiplied(width, height, data, anchor)
    }

    /// No pixels at all — a hidden cursor, which several platforms report as a
    /// zero-sized bitmap rather than an absence.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

fn check_len(width: u32, height: u32, len: usize) -> Result<(), CaptureError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|p| p.checked_mul(BYTES_PER_PIXEL))
        .ok_or_else(|| CaptureError::invalid_frame("cursor does not fit in memory"))?;
    if len != expected {
        return Err(CaptureError::invalid_frame(format!(
            "{width}x{height} cursor needs {expected} bytes, got {len}"
        )));
    }
    Ok(())
}

/// Divide by 255 with rounding. Exact for the `0..=255*255` range these blends
/// produce, and avoids the off-by-one that `v / 255` accumulates.
fn div255(v: u32) -> u32 {
    let t = v + 128;
    (t + (t >> 8)) >> 8
}

/// Blend `cursor` into `frame` where the two overlap.
///
/// Both are positioned on the virtual desktop, so this handles the cursor
/// hanging off the edge of the frame — the normal case at a screen border, and
/// the one that turns a missing bounds check into a panic. A cursor that misses
/// the frame entirely, or has no pixels, leaves the frame untouched.
///
/// The frame keeps its straight-alpha convention: the source is premultiplied,
/// the destination is not, and the result is converted back.
pub fn composite_cursor(frame: &mut RawFrame, cursor: &CursorImage) {
    if cursor.is_empty() || frame.is_empty() {
        return;
    }

    // Work out the overlapping rectangle once rather than bounds-checking every
    // pixel. `as i64` on a float saturates, so an absurd origin cannot wrap.
    let left = (cursor.position.x - frame.origin.x).round() as i64;
    let top = (cursor.position.y - frame.origin.y).round() as i64;
    let x0 = left.max(0);
    let y0 = top.max(0);
    let x1 = (left + i64::from(cursor.width)).min(i64::from(frame.width));
    let y1 = (top + i64::from(cursor.height)).min(i64::from(frame.height));
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    for fy in y0..y1 {
        let cy = (fy - top) as usize;
        for fx in x0..x1 {
            let cx = (fx - left) as usize;
            let src = (cy * cursor.width as usize + cx) * BYTES_PER_PIXEL;
            let sa = u32::from(cursor.data[src + 3]);
            if sa == 0 {
                // Premultiplied: zero alpha means zero contribution.
                continue;
            }
            let dst = (fy as usize * frame.width as usize + fx as usize) * BYTES_PER_PIXEL;
            blend_premultiplied_over(
                &cursor.data[src..src + BYTES_PER_PIXEL],
                &mut frame.data[dst..dst + BYTES_PER_PIXEL],
                sa,
            );
        }
    }
}

/// Source-over of a premultiplied `src` onto a straight-alpha `dst`, in place.
fn blend_premultiplied_over(src: &[u8], dst: &mut [u8], sa: u32) {
    let inv = 255 - sa;
    let da = u32::from(dst[3]);

    // The overwhelmingly common case: the destination is an opaque screenshot,
    // so the result stays opaque and no un-premultiplying is needed.
    if da == 255 {
        for c in 0..3 {
            dst[c] = (u32::from(src[c]) + div255(u32::from(dst[c]) * inv)) as u8;
        }
        return;
    }

    let out_a = sa + div255(da * inv);
    if out_a == 0 {
        dst.fill(0);
        return;
    }
    for c in 0..3 {
        // Premultiply the destination, blend, then convert back to straight.
        let dst_pm = div255(u32::from(dst[c]) * da);
        let out_pm = u32::from(src[c]) + div255(dst_pm * inv);
        dst[c] = ((out_pm * 255 + out_a / 2) / out_a).min(255) as u8;
    }
    dst[3] = out_a as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_cursor(w: u32, h: u32, rgb: [u8; 3], at: Vec2D) -> CursorImage {
        let mut data = Vec::new();
        for _ in 0..w * h {
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        CursorImage::from_premultiplied(w, h, data, CursorAnchor::TopLeft(at)).unwrap()
    }

    fn black_frame(w: u32, h: u32, origin: Vec2D) -> RawFrame {
        RawFrame::filled(w, h, [0, 0, 0, 255], origin, 1.0).unwrap()
    }

    #[test]
    fn an_opaque_cursor_replaces_the_pixels_under_it() {
        let mut frame = black_frame(4, 4, Vec2D::ZERO);
        composite_cursor(
            &mut frame,
            &opaque_cursor(2, 2, [255, 0, 0], Vec2D::new(1.0, 1.0)),
        );
        assert_eq!(frame.pixel(1, 1), Some([255, 0, 0, 255]));
        assert_eq!(frame.pixel(2, 2), Some([255, 0, 0, 255]));
        // Outside the cursor is untouched.
        assert_eq!(frame.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(frame.pixel(3, 3), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_half_transparent_cursor_blends_with_the_background() {
        let mut frame = black_frame(1, 1, Vec2D::ZERO);
        // Straight white at 50% alpha over black should land mid-grey.
        let cursor = CursorImage::from_straight(
            1,
            1,
            vec![255, 255, 255, 128],
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        composite_cursor(&mut frame, &cursor);
        let px = frame.pixel(0, 0).unwrap();
        assert!(
            (120..=136).contains(&px[0]),
            "expected roughly half brightness, got {px:?}"
        );
        assert_eq!(px[3], 255, "an opaque frame must stay opaque");
    }

    #[test]
    fn a_fully_transparent_cursor_changes_nothing() {
        let mut frame = black_frame(2, 2, Vec2D::ZERO);
        let before = frame.data.clone();
        // Bright white, but with every alpha byte zeroed.
        let pixels: Vec<u8> = (0..16).map(|i| if i % 4 == 3 { 0 } else { 255 }).collect();
        let cursor =
            CursorImage::from_straight(2, 2, pixels, CursorAnchor::TopLeft(Vec2D::ZERO)).unwrap();
        composite_cursor(&mut frame, &cursor);
        assert_eq!(frame.data, before);
    }

    #[test]
    fn a_cursor_hanging_off_each_edge_is_clipped_not_wrapped() {
        // Off the top-left: only the bottom-right quarter lands.
        let mut frame = black_frame(4, 4, Vec2D::ZERO);
        composite_cursor(
            &mut frame,
            &opaque_cursor(2, 2, [255, 0, 0], Vec2D::new(-1.0, -1.0)),
        );
        assert_eq!(frame.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(frame.pixel(1, 0), Some([0, 0, 0, 255]));
        assert_eq!(frame.pixel(0, 1), Some([0, 0, 0, 255]));

        // Off the bottom-right: only the top-left quarter lands.
        let mut frame = black_frame(4, 4, Vec2D::ZERO);
        composite_cursor(
            &mut frame,
            &opaque_cursor(2, 2, [0, 255, 0], Vec2D::new(3.0, 3.0)),
        );
        assert_eq!(frame.pixel(3, 3), Some([0, 255, 0, 255]));
        assert_eq!(frame.pixel(2, 3), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_cursor_entirely_outside_the_frame_is_a_no_op() {
        let mut frame = black_frame(4, 4, Vec2D::ZERO);
        let before = frame.data.clone();
        for at in [
            Vec2D::new(-10.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(0.0, -10.0),
            Vec2D::new(0.0, 10.0),
        ] {
            composite_cursor(&mut frame, &opaque_cursor(2, 2, [255, 0, 0], at));
        }
        assert_eq!(frame.data, before);
    }

    #[test]
    fn the_frame_origin_is_taken_into_account() {
        // A frame that starts at (100, 100) on the desktop: a cursor at desktop
        // (100, 100) belongs at frame-local (0, 0), not (100, 100).
        let mut frame = black_frame(4, 4, Vec2D::new(100.0, 100.0));
        composite_cursor(
            &mut frame,
            &opaque_cursor(1, 1, [255, 0, 0], Vec2D::new(100.0, 100.0)),
        );
        assert_eq!(frame.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn a_hotspot_anchor_shifts_the_bitmap_but_a_top_left_anchor_does_not() {
        let data = vec![255, 0, 0, 255];
        let top_left = CursorImage::from_premultiplied(
            1,
            1,
            data.clone(),
            CursorAnchor::TopLeft(Vec2D::new(10.0, 10.0)),
        )
        .unwrap();
        assert_eq!(top_left.position, Vec2D::new(10.0, 10.0));

        let hotspot = CursorImage::from_premultiplied(
            1,
            1,
            data,
            CursorAnchor::Hotspot {
                position: Vec2D::new(10.0, 10.0),
                xhot: 4,
                yhot: 3,
            },
        )
        .unwrap();
        assert_eq!(hotspot.position, Vec2D::new(6.0, 7.0));
    }

    #[test]
    fn argb_words_are_unpacked_in_the_right_channel_order() {
        // 0xAARRGGBB, premultiplied and opaque.
        let cursor = CursorImage::from_argb_words(
            2,
            1,
            &[0xff11_2233, 0xff44_5566],
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        assert_eq!(
            cursor.data,
            vec![0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff]
        );
    }

    #[test]
    fn argb_words_rejects_a_short_buffer() {
        assert!(matches!(
            CursorImage::from_argb_words(4, 4, &[0; 3], CursorAnchor::TopLeft(Vec2D::ZERO)),
            Err(CaptureError::InvalidFrame(_))
        ));
    }

    #[test]
    fn channels_brighter_than_the_alpha_are_clamped_to_it() {
        // A premultiplied pixel may not have a channel above its alpha; if one
        // does, the blend would add more than the pixel's share of light.
        let cursor = CursorImage::from_premultiplied(
            1,
            1,
            vec![255, 255, 255, 10],
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        assert_eq!(cursor.data, vec![10, 10, 10, 10]);

        // And the blend consequently stays in range.
        let mut frame = RawFrame::filled(1, 1, [255, 255, 255, 255], Vec2D::ZERO, 1.0).unwrap();
        composite_cursor(&mut frame, &cursor);
        assert_eq!(frame.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn blending_onto_a_transparent_gap_keeps_the_cursor_colour() {
        // Stitched desktops have fully transparent gaps between monitors.
        let mut frame = RawFrame::transparent(1, 1, Vec2D::ZERO, 1.0).unwrap();
        composite_cursor(
            &mut frame,
            &opaque_cursor(1, 1, [200, 100, 50], Vec2D::ZERO),
        );
        assert_eq!(frame.pixel(0, 0), Some([200, 100, 50, 255]));
    }

    #[test]
    fn a_partly_transparent_cursor_over_a_transparent_gap_accumulates_alpha() {
        let mut frame = RawFrame::transparent(1, 1, Vec2D::ZERO, 1.0).unwrap();
        let cursor = CursorImage::from_straight(
            1,
            1,
            vec![255, 0, 0, 128],
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        composite_cursor(&mut frame, &cursor);
        let px = frame.pixel(0, 0).unwrap();
        assert_eq!(px[3], 128, "alpha should come from the cursor alone");
        assert!(px[0] > 250, "un-premultiplying should restore full red");
    }

    #[test]
    fn mismatched_buffer_lengths_are_rejected() {
        assert!(matches!(
            CursorImage::from_premultiplied(2, 2, vec![0; 8], CursorAnchor::TopLeft(Vec2D::ZERO)),
            Err(CaptureError::InvalidFrame(_))
        ));
    }

    #[test]
    fn an_empty_cursor_is_a_no_op() {
        let mut frame = black_frame(2, 2, Vec2D::ZERO);
        let before = frame.data.clone();
        let cursor =
            CursorImage::from_premultiplied(0, 0, Vec::new(), CursorAnchor::TopLeft(Vec2D::ZERO))
                .unwrap();
        assert!(cursor.is_empty());
        composite_cursor(&mut frame, &cursor);
        assert_eq!(frame.data, before);
    }

    #[test]
    fn div255_rounds_rather_than_truncating() {
        assert_eq!(div255(0), 0);
        assert_eq!(div255(255), 1);
        assert_eq!(div255(255 * 255), 255);
        assert_eq!(div255(128 * 255), 128);
        // Exact against the reference for the whole range these blends produce.
        for v in 0..=(255 * 255u32) {
            assert_eq!(div255(v), (f64::from(v) / 255.0).round() as u32, "v={v}");
        }
    }
}
