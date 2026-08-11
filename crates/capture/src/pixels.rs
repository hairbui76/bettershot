//! Turning platform pixel buffers into the RGBA8 a [`crate::RawFrame`] wants.
//!
//! Backends receive pixels in whatever layout the display server prefers —
//! X11's `ZPixmap` is native-endian words masked by the visual, Windows DIBs are
//! BGRA with a junk alpha channel. Both conversions are pure byte pushing, so
//! they live here where they can be tested on any machine rather than inside a
//! `cfg`-gated backend that only compiles on one OS.

use crate::CaptureError;

/// Description of an X11 `ZPixmap` buffer: how the server packed it, and which
/// bits of each pixel word hold which channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZPixmapFormat {
    /// Bits per pixel from the server's `pixmap-formats` (24 or 32 in practice).
    pub bits_per_pixel: u8,
    /// Scanline padding in bits from the same table (8, 16 or 32).
    pub scanline_pad: u8,
    /// `true` when the server's `image-byte-order` is LSB-first.
    pub little_endian: bool,
    /// Red channel mask from the visual, e.g. `0x00ff0000`.
    pub red_mask: u32,
    /// Green channel mask from the visual, e.g. `0x0000ff00`.
    pub green_mask: u32,
    /// Blue channel mask from the visual, e.g. `0x000000ff`.
    pub blue_mask: u32,
}

impl ZPixmapFormat {
    /// The layout essentially every modern X server uses: 32 bits per pixel,
    /// 32-bit scanline padding, little-endian, TrueColor BGRX byte order.
    pub const COMMON_BGRX: Self = Self {
        bits_per_pixel: 32,
        scanline_pad: 32,
        little_endian: true,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
    };

    /// Bytes each pixel occupies.
    fn bytes_per_pixel(&self) -> Result<usize, CaptureError> {
        match self.bits_per_pixel {
            24 => Ok(3),
            32 => Ok(4),
            other => Err(CaptureError::unsupported(format!(
                "{other}-bit X11 pixmaps are not supported (need 24 or 32)"
            ))),
        }
    }

    /// Bytes per row including the server's scanline padding.
    fn stride(&self, width: u32) -> Result<usize, CaptureError> {
        let bpp = self.bytes_per_pixel()?;
        let pad_bits = match self.scanline_pad {
            0 => 8,
            p => u32::from(p),
        };
        let row_bits = u64::from(width) * (bpp as u64) * 8;
        let pad = u64::from(pad_bits);
        let padded_bits = row_bits.div_ceil(pad) * pad;
        usize::try_from(padded_bits / 8)
            .map_err(|_| CaptureError::invalid_frame("X11 scanline does not fit in memory"))
    }
}

/// Extract one 8-bit channel from a pixel word.
///
/// Handles masks narrower than 8 bits (16-bit 5-6-5 visuals) by replicating the
/// high bits downwards, which is the standard way to expand e.g. 5 bits to 8
/// without a division.
fn channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = (pixel & mask) >> shift;
    match bits {
        8 => raw as u8,
        b if b > 8 => (raw >> (b - 8)) as u8,
        b => {
            // Replicate the value downwards: 5 bits 0bxyzwv -> 0bxyzwvxyz, so
            // an all-ones input reaches 255 instead of 31.
            let mut value = 0u32;
            let mut filled = 0i32;
            while filled < 8 {
                let shift = 8 - filled - b as i32;
                value |= if shift >= 0 {
                    raw << shift
                } else {
                    raw >> (-shift)
                };
                filled += b as i32;
            }
            (value & 0xff) as u8
        }
    }
}

/// Convert an X11 `ZPixmap` buffer to tightly packed RGBA8.
///
/// Alpha is forced to `255`: the X11 root window has no alpha channel, and the
/// unused byte of a 32-bit XRGB pixel is undefined — copying it through is the
/// classic "screenshot is entirely transparent" bug.
///
/// Fails with [`CaptureError::InvalidFrame`] when `data` is shorter than
/// `height` rows of the format's stride, and [`CaptureError::Unsupported`] for
/// exotic bit depths.
pub fn zpixmap_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    format: ZPixmapFormat,
) -> Result<Vec<u8>, CaptureError> {
    let bpp = format.bytes_per_pixel()?;
    let stride = format.stride(width)?;
    let needed = stride
        .checked_mul(height as usize)
        .ok_or_else(|| CaptureError::invalid_frame("X11 image does not fit in memory"))?;
    if data.len() < needed {
        return Err(CaptureError::invalid_frame(format!(
            "X11 image is {} bytes, expected at least {needed} for {width}x{height}",
            data.len()
        )));
    }

    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row = &data[y * stride..y * stride + width as usize * bpp];
        for x in 0..width as usize {
            let bytes = &row[x * bpp..x * bpp + bpp];
            let pixel = if format.little_endian {
                bytes
                    .iter()
                    .enumerate()
                    .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (8 * i)))
            } else {
                bytes.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
            };
            out.push(channel(pixel, format.red_mask));
            out.push(channel(pixel, format.green_mask));
            out.push(channel(pixel, format.blue_mask));
            out.push(255);
        }
    }
    Ok(out)
}

/// Convert a tightly packed BGRA8 buffer (Windows DIB / DXGI order) to RGBA8 in
/// place-ish.
///
/// `force_opaque` overwrites alpha with `255`, which is what you want for
/// desktop and monitor grabs: DXGI leaves the alpha channel undefined for
/// opaque surfaces, and some drivers hand back zeroes.
pub fn bgra_to_rgba(data: &mut [u8], force_opaque: bool) -> Result<(), CaptureError> {
    if data.len() % 4 != 0 {
        return Err(CaptureError::invalid_frame(format!(
            "BGRA buffer length {} is not a multiple of 4",
            data.len()
        )));
    }
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
        if force_opaque {
            px[3] = 255;
        }
    }
    Ok(())
}

/// Convert premultiplied RGBA8 to straight (non-premultiplied) alpha, in place.
///
/// [`crate::RawFrame`] is straight-alpha, as documented on the type, because
/// PNG is and the render canvas is. Some platforms hand over premultiplied
/// pixels — macOS's ScreenCaptureKit returns premultiplied BGRA — and passing
/// those through unchanged makes every consumer apply alpha a second time.
///
/// Invisible on a normal screenshot, where every pixel is opaque and the two
/// representations are identical bytes; it shows on the translucent edges of a
/// window capture, which come out too dark.
///
/// The conversion is lossy in the direction it has to be: a pixel with a low
/// alpha carries little colour information, and dividing it back out cannot
/// invent precision that premultiplying discarded. Fully transparent pixels
/// have no colour to recover at all and are zeroed.
pub fn unpremultiply_rgba(data: &mut [u8]) -> Result<(), CaptureError> {
    if data.len() % 4 != 0 {
        return Err(CaptureError::invalid_frame(format!(
            "RGBA buffer length {} is not a multiple of 4",
            data.len()
        )));
    }
    for px in data.chunks_exact_mut(4) {
        let alpha = u32::from(px[3]);
        // Opaque is the overwhelmingly common case and is exactly identity, so
        // a whole-screen capture pays almost nothing for this pass.
        if alpha == 255 {
            continue;
        }
        if alpha == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            continue;
        }
        for channel in &mut px[..3] {
            // `min(alpha)` upholds the premultiplied invariant `c <= a`. A
            // source that violates it would otherwise divide out to more than
            // 255 and wrap.
            let value = u32::from(*channel).min(alpha);
            *channel = ((value * 255 + alpha / 2) / alpha) as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_bgrx_pixels_become_rgba() {
        // Two pixels: pure red then pure blue, little-endian BGRX.
        let data = vec![
            0x00, 0x00, 0xff, 0x00, // B=0 G=0 R=255 X
            0xff, 0x00, 0x00, 0x00, // B=255 G=0 R=0 X
        ];
        let rgba = zpixmap_to_rgba(&data, 2, 1, ZPixmapFormat::COMMON_BGRX).unwrap();
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn alpha_is_forced_opaque_even_when_the_pad_byte_is_zero() {
        let data = vec![0x10, 0x20, 0x30, 0x00];
        let rgba = zpixmap_to_rgba(&data, 1, 1, ZPixmapFormat::COMMON_BGRX).unwrap();
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 255]);
    }

    #[test]
    fn scanline_padding_between_rows_is_skipped() {
        // 1 pixel wide, 24bpp, 32-bit scanline pad -> 4-byte stride, so byte 3
        // of each row is padding and must not be read as pixel data.
        let format = ZPixmapFormat {
            bits_per_pixel: 24,
            scanline_pad: 32,
            ..ZPixmapFormat::COMMON_BGRX
        };
        let data = vec![
            0x01, 0x02, 0x03, 0xee, // row 0: B G R + pad
            0x04, 0x05, 0x06, 0xee, // row 1
        ];
        let rgba = zpixmap_to_rgba(&data, 1, 2, format).unwrap();
        assert_eq!(rgba, vec![0x03, 0x02, 0x01, 255, 0x06, 0x05, 0x04, 255]);
    }

    #[test]
    fn big_endian_servers_assemble_pixels_the_other_way_round() {
        let format = ZPixmapFormat {
            little_endian: false,
            ..ZPixmapFormat::COMMON_BGRX
        };
        // MSB-first XRGB: 0x00 R G B
        let data = vec![0x00, 0x11, 0x22, 0x33];
        let rgba = zpixmap_to_rgba(&data, 1, 1, format).unwrap();
        assert_eq!(rgba, vec![0x11, 0x22, 0x33, 255]);
    }

    #[test]
    fn unusual_channel_masks_are_honoured() {
        // BGRA-in-memory little-endian with swapped red/blue masks: the same
        // bytes must decode to the mirrored colour.
        let format = ZPixmapFormat {
            red_mask: 0x0000_00ff,
            blue_mask: 0x00ff_0000,
            ..ZPixmapFormat::COMMON_BGRX
        };
        let data = vec![0x00, 0x00, 0xff, 0x00];
        let rgba = zpixmap_to_rgba(&data, 1, 1, format).unwrap();
        assert_eq!(rgba, vec![0x00, 0x00, 0xff, 255]);
    }

    #[test]
    fn narrow_masks_are_expanded_to_eight_bits() {
        // 5-bit red mask: all ones must reach 255, not 31.
        assert_eq!(channel(0b1_1111, 0b1_1111), 255);
        assert_eq!(channel(0, 0b1_1111), 0);
        // 6-bit green in a 5-6-5 layout.
        assert_eq!(channel(0b111_111 << 5, 0b111_111 << 5), 255);
        assert_eq!(channel(0xffff_ffff, 0), 0);
    }

    #[test]
    fn truncated_buffers_are_rejected() {
        let err = zpixmap_to_rgba(&[0, 0, 0], 2, 1, ZPixmapFormat::COMMON_BGRX).unwrap_err();
        assert!(matches!(err, CaptureError::InvalidFrame(_)));
    }

    #[test]
    fn exotic_bit_depths_are_unsupported() {
        let format = ZPixmapFormat {
            bits_per_pixel: 16,
            ..ZPixmapFormat::COMMON_BGRX
        };
        assert!(matches!(
            zpixmap_to_rgba(&[0; 64], 2, 2, format),
            Err(CaptureError::Unsupported(_))
        ));
    }

    #[test]
    fn output_length_always_matches_the_requested_size() {
        let data = vec![0u8; 4 * 7 * 5];
        let rgba = zpixmap_to_rgba(&data, 7, 5, ZPixmapFormat::COMMON_BGRX).unwrap();
        assert_eq!(rgba.len(), 7 * 5 * 4);
    }

    #[test]
    fn bgra_swaps_red_and_blue_and_can_force_opacity() {
        let mut data = vec![10, 20, 30, 0, 1, 2, 3, 128];
        bgra_to_rgba(&mut data, true).unwrap();
        assert_eq!(data, vec![30, 20, 10, 255, 3, 2, 1, 255]);

        let mut data = vec![10, 20, 30, 40];
        bgra_to_rgba(&mut data, false).unwrap();
        assert_eq!(data, vec![30, 20, 10, 40]);
    }

    #[test]
    fn bgra_rejects_ragged_buffers() {
        let mut data = vec![0u8; 6];
        assert!(matches!(
            bgra_to_rgba(&mut data, true),
            Err(CaptureError::InvalidFrame(_))
        ));
    }
    #[test]
    fn opaque_pixels_are_left_exactly_alone() {
        // Where alpha is 255 the two representations are the same bytes, and a
        // screenshot is opaque almost everywhere, so this path must not drift.
        let mut data = vec![10, 20, 30, 255, 200, 100, 50, 255];
        unpremultiply_rgba(&mut data).unwrap();
        assert_eq!(data, vec![10, 20, 30, 255, 200, 100, 50, 255]);
    }

    #[test]
    fn a_half_transparent_pixel_recovers_its_full_colour() {
        // Premultiplied white at 50% alpha is (128, 128, 128, 128); straight,
        // it is white.
        let mut data = vec![128, 128, 128, 128];
        unpremultiply_rgba(&mut data).unwrap();
        assert_eq!(data, vec![255, 255, 255, 128]);
    }

    #[test]
    fn a_fully_transparent_pixel_has_no_colour_to_recover() {
        let mut data = vec![0, 0, 0, 0];
        unpremultiply_rgba(&mut data).unwrap();
        assert_eq!(data, vec![0, 0, 0, 0]);
    }

    #[test]
    fn channels_above_the_alpha_cannot_overflow() {
        // Not a legal premultiplied pixel, but a backend could hand one over,
        // and dividing 255 by an alpha of 1 would wrap without the clamp.
        let mut data = vec![255, 255, 255, 1];
        unpremultiply_rgba(&mut data).unwrap();
        assert_eq!(data, vec![255, 255, 255, 1]);
    }

    #[test]
    fn premultiplying_and_undoing_it_round_trips_within_the_precision_left() {
        // Premultiplication discards precision at low alpha and no inverse can
        // invent it back, so the tolerance scales with how much was thrown
        // away: 255/alpha is the size of one representable step.
        for alpha in 1..=255u32 {
            for straight in (0..=255u32).step_by(5) {
                let premultiplied = ((straight * alpha + 127) / 255) as u8;
                let mut data = vec![premultiplied, 0, 0, alpha as u8];
                unpremultiply_rgba(&mut data).unwrap();
                let tolerance = (255.0 / alpha as f64).ceil() as i32;
                let drift = (i32::from(data[0]) - straight as i32).abs();
                assert!(
                    drift <= tolerance,
                    "alpha={alpha} straight={straight}: got {}, off by {drift} \
                     with a tolerance of {tolerance}",
                    data[0]
                );
            }
        }
    }

    #[test]
    fn unpremultiply_rejects_a_ragged_buffer() {
        let mut data = vec![0u8; 7];
        assert!(matches!(
            unpremultiply_rgba(&mut data),
            Err(CaptureError::InvalidFrame(_))
        ));
    }
}
