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
    /// The constructors normalise this — they check the length and clamp each
    /// channel to the alpha, so the premultiplied invariant `r, g, b <= a`
    /// holds. The fields are public, though, so a struct literal can sidestep
    /// them; [`composite_cursor`] therefore re-checks the length and clamps
    /// again rather than trusting the invariant it cannot enforce.
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
    ///
    /// A short buffer is an error; a longer one has its tail ignored, since a
    /// reply carrying padding past `width * height` is still a usable cursor.
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

    /// Build from a Win32 colour cursor: a top-down BGRA image plus the 1-bit
    /// AND mask that accompanies it.
    ///
    /// Windows stores cursor colour bitmaps with **straight** alpha, so they
    /// are premultiplied on the way in like any other straight source.
    ///
    /// The mask is not redundant. Plenty of cursors — including several of the
    /// system ones — are 32 bits per pixel with the alpha channel left entirely
    /// zero, because they predate alpha and rely on the AND mask for shape.
    /// Trusting the alpha there yields a completely invisible cursor, so an
    /// all-zero alpha channel falls back to the mask: a clear bit is opaque and
    /// a set bit is transparent, which is the inverse of how it reads.
    ///
    /// Lives here rather than in the Windows backend so the bit-twiddling is
    /// tested on every platform, the same split [`Self::from_argb_words`] uses.
    pub fn from_win32_color(
        width: u32,
        height: u32,
        bgra: &[u8],
        mask: &[u8],
        mask_stride: usize,
        anchor: CursorAnchor,
    ) -> Result<Self, CaptureError> {
        check_len(width, height, bgra.len())?;
        require_mask(width, height, mask, mask_stride, 1)?;

        let opaque_alpha = bgra.chunks_exact(BYTES_PER_PIXEL).all(|px| px[3] == 0);
        let mut data = Vec::with_capacity(bgra.len());
        for y in 0..height {
            for x in 0..width {
                let i = (y as usize * width as usize + x as usize) * BYTES_PER_PIXEL;
                let alpha = if opaque_alpha {
                    // A set AND-mask bit means "leave the screen alone here".
                    if mask_bit(mask, mask_stride, x, y) {
                        0
                    } else {
                        255
                    }
                } else {
                    bgra[i + 3]
                };
                data.push(bgra[i + 2]); // R (the source is B, G, R, A)
                data.push(bgra[i + 1]); // G
                data.push(bgra[i]); // B
                data.push(alpha);
            }
        }
        Self::from_straight(width, height, data, anchor)
    }

    /// Build from a Win32 monochrome cursor, where there is no colour bitmap at
    /// all and the mask is **double height**: the AND mask on top, the XOR mask
    /// below it.
    ///
    /// The two bits together pick one of four behaviours per pixel:
    ///
    /// | AND | XOR | Result |
    /// | --- | --- | --- |
    /// | 0 | 0 | opaque black |
    /// | 0 | 1 | opaque white |
    /// | 1 | 0 | transparent |
    /// | 1 | 1 | invert whatever is underneath |
    ///
    /// The last one cannot be represented in a static image — it is why the
    /// classic text I-beam stays visible over any background. It is rendered as
    /// opaque black, which is what a screenshot of it over a light background
    /// would have looked like, and is what other capture tools do.
    ///
    /// `height` is the cursor's height, not the bitmap's; the buffer must hold
    /// `2 * height` rows.
    pub fn from_win32_monochrome(
        width: u32,
        height: u32,
        mask: &[u8],
        mask_stride: usize,
        anchor: CursorAnchor,
    ) -> Result<Self, CaptureError> {
        require_mask(width, height, mask, mask_stride, 2)?;

        let mut data = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
        for y in 0..height {
            for x in 0..width {
                let and = mask_bit(mask, mask_stride, x, y);
                let xor = mask_bit(mask, mask_stride, x, y + height);
                let (luma, alpha) = match (and, xor) {
                    (false, false) => (0, 255),
                    (false, true) => (255, 255),
                    (true, false) => (0, 0),
                    // Inversion, approximated as black. See the table above.
                    (true, true) => (0, 255),
                };
                data.extend_from_slice(&[luma, luma, luma, alpha]);
            }
        }
        Self::from_straight(width, height, data, anchor)
    }

    /// No pixels at all — a hidden cursor, which several platforms report as a
    /// zero-sized bitmap rather than an absence.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Read one pixel from a 1-bit-per-pixel Windows mask.
///
/// Windows packs these most-significant-bit first, with each row padded out to
/// a 4-byte boundary — `mask_stride` is that padded row length. A set bit means
/// "transparent" in an AND mask, which is the opposite of the intuitive reading
/// and the reason this is a named function rather than an inline expression.
fn mask_bit(mask: &[u8], stride: usize, x: u32, y: u32) -> bool {
    let index = y as usize * stride + (x as usize / 8);
    match mask.get(index) {
        // Out of range is treated as transparent rather than panicking: a short
        // mask is already rejected by `require_mask`, and a cursor is never
        // worth taking the process down for.
        None => true,
        Some(byte) => byte & (0x80 >> (x % 8)) != 0,
    }
}

/// Check a 1bpp mask covers `rows_per_cursor * height` padded rows.
fn require_mask(
    width: u32,
    height: u32,
    mask: &[u8],
    stride: usize,
    rows_per_cursor: u32,
) -> Result<(), CaptureError> {
    if width == 0 || height == 0 {
        return Err(CaptureError::EmptyRegion);
    }
    let minimum = (width as usize).div_ceil(8);
    if stride < minimum {
        return Err(CaptureError::invalid_frame(format!(
            "a {width}px mask row needs at least {minimum} bytes, got a stride of {stride}"
        )));
    }
    let rows = (height as usize) * (rows_per_cursor as usize);
    let needed = stride
        .checked_mul(rows)
        .ok_or_else(|| CaptureError::invalid_frame("cursor mask does not fit in memory"))?;
    if mask.len() < needed {
        return Err(CaptureError::invalid_frame(format!(
            "cursor mask is {} bytes, expected {needed} for {rows} rows of {stride}",
            mask.len()
        )));
    }
    Ok(())
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

    // `CursorImage`'s fields are public, so a caller can build one by struct
    // literal and skip the constructors' validation. Re-check rather than
    // trusting it: the indexing below would otherwise run off the end of a
    // short buffer.
    if cursor.data.len() != (cursor.width as usize) * (cursor.height as usize) * BYTES_PER_PIXEL {
        log::warn!(
            "ignoring a {}x{} cursor with {} bytes of pixel data",
            cursor.width,
            cursor.height,
            cursor.data.len()
        );
        return;
    }

    // Work out the overlapping rectangle once rather than bounds-checking every
    // pixel. The float-to-int casts saturate and the additions are saturating,
    // so no position — however absurd, including infinities — can wrap.
    let left = (cursor.position.x - frame.origin.x).round() as i64;
    let top = (cursor.position.y - frame.origin.y).round() as i64;
    let x0 = left.max(0);
    let y0 = top.max(0);
    let x1 = left
        .saturating_add(i64::from(cursor.width))
        .min(i64::from(frame.width));
    let y1 = top
        .saturating_add(i64::from(cursor.height))
        .min(i64::from(frame.height));
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
///
/// `sa` is `src[3]`, already read by the caller.
fn blend_premultiplied_over(src: &[u8], dst: &mut [u8], sa: u32) {
    let inv = 255 - sa;
    let da = u32::from(dst[3]);

    // The overwhelmingly common case: the destination is an opaque screenshot,
    // so the result stays opaque and no un-premultiplying is needed.
    if da == 255 {
        for c in 0..3 {
            // `min(sa)` upholds the premultiplied invariant for a `CursorImage`
            // that was built by struct literal rather than by a constructor.
            // Without it the sum can exceed 255 and wrap: a bright pixel with a
            // near-zero alpha would come out dark.
            let s = u32::from(src[c]).min(sa);
            dst[c] = (s + div255(u32::from(dst[c]) * inv)) as u8;
        }
        return;
    }

    let out_a = sa + div255(da * inv);
    if out_a == 0 {
        dst.fill(0);
        return;
    }
    // Premultiply the destination, blend, and convert back to straight in a
    // single quotient. Two roundings are deliberately avoided here, because the
    // final division by a small alpha amplifies both: `dst * da / 255` is not
    // reduced to 8 bits first (that alone costs up to 64 LSB), and the divisor
    // is the *unrounded* `out_a`, since `out_a` as a byte can be off by half a
    // unit which at `out_a = 2` is a 25% error. Largest numerator is
    // 255*255*255 ≈ 16.6M, so u32 has plenty of room.
    let denominator = sa * 255 + da * inv;
    for c in 0..3 {
        let s = u32::from(src[c]).min(sa);
        let numerator = s * 255 * 255 + u32::from(dst[c]) * da * inv;
        dst[c] = ((numerator + denominator / 2) / denominator).min(255) as u8;
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
    fn the_blend_matches_a_floating_point_source_over_reference() {
        // The blend is the whole feature, so check it against the textbook
        // formula rather than against itself. Sweeps every alpha pair and a
        // spread of colours, both the opaque fast path and the general one.
        fn reference(src_c: f64, sa: f64, dst_c: f64, da: f64) -> f64 {
            let (sa, da) = (sa / 255.0, da / 255.0);
            let out_a = sa + da * (1.0 - sa);
            if out_a == 0.0 {
                return 0.0;
            }
            // `src_c` arrives premultiplied; `dst_c` does not.
            let out_pm = src_c / 255.0 + (dst_c / 255.0) * da * (1.0 - sa);
            255.0 * out_pm / out_a
        }

        let mut worst: f64 = 0.0;
        for sa in 0..=255u32 {
            for da in (0..=255u32).step_by(3) {
                for src_c in (0..=sa).step_by((sa as usize / 8).max(1)) {
                    for dst_c in (0..=255u32).step_by(17) {
                        let mut dst = [dst_c as u8, 0, 0, da as u8];
                        let src = [src_c as u8, 0, 0, sa as u8];
                        if sa == 0 {
                            continue; // the caller skips these
                        }
                        blend_premultiplied_over(&src, &mut dst, sa);
                        let expected = reference(src_c as f64, sa as f64, dst_c as f64, da as f64);
                        worst = worst.max((f64::from(dst[0]) - expected).abs());

                        // The composited alpha must match too.
                        let expected_a = 255.0
                            * (sa as f64 / 255.0 + (da as f64 / 255.0) * (1.0 - sa as f64 / 255.0));
                        assert!(
                            (f64::from(dst[3]) - expected_a).abs() <= 1.0,
                            "alpha: sa={sa} da={da} got {} want {expected_a:.2}",
                            dst[3]
                        );
                    }
                }
            }
        }
        assert!(
            worst <= 1.0,
            "blend drifts from the reference by {worst:.3} LSB"
        );
    }

    #[test]
    fn a_low_alpha_destination_does_not_lose_the_colour_underneath() {
        // Regression: rounding the premultiplied destination to 8 bits before
        // dividing by a small composited alpha turned this 64 into a 128.
        let mut dst = [128, 0, 0, 1];
        blend_premultiplied_over(&[0, 0, 0, 1], &mut dst, 1);
        assert_eq!(dst[0], 64);
    }

    #[test]
    fn a_cursor_built_by_struct_literal_cannot_corrupt_or_panic() {
        // The fields are public, so the constructors' validation can be
        // bypassed. Neither a broken premultiplied invariant nor a short buffer
        // may take the process down or darken the frame.
        let mut frame = RawFrame::filled(2, 2, [255, 255, 255, 255], Vec2D::ZERO, 1.0).unwrap();
        composite_cursor(
            &mut frame,
            &CursorImage {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 1], // r,g,b far above alpha
                position: Vec2D::ZERO,
            },
        );
        assert_eq!(
            frame.pixel(0, 0),
            Some([255, 255, 255, 255]),
            "white over white must stay white, not wrap to dark"
        );

        // A buffer too short for the stated size is ignored, not indexed.
        let before = frame.data.clone();
        composite_cursor(
            &mut frame,
            &CursorImage {
                width: 4,
                height: 4,
                data: vec![0; 8],
                position: Vec2D::ZERO,
            },
        );
        assert_eq!(frame.data, before);
    }

    #[test]
    fn an_absurd_cursor_position_cannot_overflow_the_overlap_maths() {
        let mut frame = RawFrame::filled(4, 4, [0, 0, 0, 255], Vec2D::ZERO, 1.0).unwrap();
        let before = frame.data.clone();
        for x in [1e30, -1e30, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            composite_cursor(
                &mut frame,
                &opaque_cursor(2, 2, [255, 0, 0], Vec2D::new(x, x)),
            );
        }
        // NaN casts to 0, so that one legitimately draws at the origin; the
        // rest are far outside. What matters is that none of them panicked.
        assert_eq!(frame.pixel(3, 3), Some([0, 0, 0, 255]));
        let _ = before;
    }

    /// Pack `bits` (row-major booleans) into a 1bpp Windows mask, MSB first,
    /// rows padded to 4 bytes.
    fn pack_mask(width: u32, rows: &[Vec<bool>]) -> (Vec<u8>, usize) {
        let stride = ((width as usize).div_ceil(8)).div_ceil(4) * 4;
        let mut out = vec![0u8; stride * rows.len()];
        for (y, row) in rows.iter().enumerate() {
            for (x, &set) in row.iter().enumerate() {
                if set {
                    out[y * stride + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        (out, stride)
    }

    #[test]
    fn a_win32_colour_cursor_uses_its_alpha_channel() {
        // Two BGRA pixels: opaque red, then half-transparent blue.
        let bgra = vec![0, 0, 255, 255, 255, 0, 0, 128];
        let (mask, stride) = pack_mask(2, &[vec![false, false]]);
        let cursor = CursorImage::from_win32_color(
            2,
            1,
            &bgra,
            &mask,
            stride,
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        // Stored premultiplied: opaque red survives, the blue is halved.
        assert_eq!(&cursor.data[..4], &[255, 0, 0, 255]);
        assert_eq!(cursor.data[7], 128, "alpha is preserved");
        assert!(cursor.data[6] < 130, "blue should be premultiplied down");
    }

    #[test]
    fn a_colour_cursor_with_no_alpha_falls_back_to_the_and_mask() {
        // Several system cursors are 32bpp with the alpha channel left zero.
        // Believing that alpha gives a completely invisible pointer.
        let bgra = vec![0, 0, 255, 0, 0, 255, 0, 0];
        // Left pixel opaque (clear bit), right pixel transparent (set bit).
        let (mask, stride) = pack_mask(2, &[vec![false, true]]);
        let cursor = CursorImage::from_win32_color(
            2,
            1,
            &bgra,
            &mask,
            stride,
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        assert_eq!(&cursor.data[..4], &[255, 0, 0, 255], "left is opaque red");
        assert_eq!(&cursor.data[4..], &[0, 0, 0, 0], "right is transparent");
    }

    #[test]
    fn a_monochrome_cursor_decodes_all_four_and_xor_combinations() {
        // One row of four pixels covering every case in the table:
        // (0,0) black, (0,1) white, (1,0) transparent, (1,1) invert.
        let and = vec![false, false, true, true];
        let xor = vec![false, true, false, true];
        let (mask, stride) = pack_mask(4, &[and, xor]);
        let cursor = CursorImage::from_win32_monochrome(
            4,
            1,
            &mask,
            stride,
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        assert_eq!(&cursor.data[0..4], &[0, 0, 0, 255], "opaque black");
        assert_eq!(&cursor.data[4..8], &[255, 255, 255, 255], "opaque white");
        assert_eq!(&cursor.data[8..12], &[0, 0, 0, 0], "transparent");
        // Inversion cannot be represented statically; rendered as black.
        assert_eq!(&cursor.data[12..16], &[0, 0, 0, 255], "invert -> black");
    }

    #[test]
    fn mask_rows_are_read_at_their_padded_stride() {
        // A 1px-wide cursor still has a 4-byte mask row. Reading it as one byte
        // per row would take the second row's bit from the first row's padding.
        let (mask, stride) = pack_mask(1, &[vec![false], vec![true]]);
        assert_eq!(stride, 4);
        let cursor = CursorImage::from_win32_monochrome(
            1,
            1,
            &mask,
            stride,
            CursorAnchor::TopLeft(Vec2D::ZERO),
        )
        .unwrap();
        // AND=0, XOR=1 is opaque white.
        assert_eq!(cursor.data, vec![255, 255, 255, 255]);
    }

    #[test]
    fn mask_bits_are_read_most_significant_first() {
        // Windows packs the leftmost pixel into the *high* bit. Reading LSB
        // first mirrors every cursor horizontally within each 8px group.
        let (mask, stride) = pack_mask(8, &[(0..8).map(|i| i == 0).collect()]);
        assert_eq!(mask[0], 0x80, "pixel 0 belongs in the high bit");
        assert!(mask_bit(&mask, stride, 0, 0));
        assert!(!mask_bit(&mask, stride, 1, 0));
    }

    #[test]
    fn a_truncated_win32_mask_is_an_error_not_a_panic() {
        let bgra = vec![0; 4 * 4];
        assert!(
            CursorImage::from_win32_color(
                4,
                1,
                &bgra,
                &[0; 1],
                4,
                CursorAnchor::TopLeft(Vec2D::ZERO)
            )
            .is_err()
        );
        // Monochrome needs two rows' worth, so one row is short.
        assert!(
            CursorImage::from_win32_monochrome(
                4,
                1,
                &[0; 4],
                4,
                CursorAnchor::TopLeft(Vec2D::ZERO)
            )
            .is_err()
        );
    }

    #[test]
    fn a_win32_mask_stride_narrower_than_the_cursor_is_rejected() {
        assert!(
            CursorImage::from_win32_monochrome(
                64,
                1,
                &[0; 64],
                4,
                CursorAnchor::TopLeft(Vec2D::ZERO)
            )
            .is_err(),
            "a 64px row needs 8 bytes, not 4"
        );
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
