//! The RGBA8 pixel buffer everything in this crate reads from and writes to.
//!
//! # Why straight (non-premultiplied) alpha
//!
//! Screenshots arrive from the capture backends as straight RGBA8, and PNG
//! stores straight RGBA8. Keeping the canvas in the same representation means
//! load and save are memcpy-shaped and lossless; the only place premultiplied
//! math would help is compositing, and there we convert on the fly. Round-trips
//! through `encode_png`/`decode` must be bit-exact, which premultiplying would
//! break for translucent pixels.

use std::io::{BufWriter, Write};
use std::path::Path;

use bettershot_core::style::Color;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

use crate::error::RenderError;

/// Bytes per pixel. RGBA8, in that channel order.
pub const BYTES_PER_PIXEL: usize = 4;

/// A CPU-side RGBA8 image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes, row-major, top row first.
    data: Vec<u8>,
}

impl Canvas {
    /// A fully transparent canvas.
    ///
    /// Zero-sized canvases are legal and turn every drawing operation into a
    /// no-op, which keeps the "annotate an empty capture" path from needing
    /// special cases all over the renderer.
    ///
    /// # Panics
    /// If `width * height * 4` does not fit in `usize`.
    pub fn new(width: u32, height: u32) -> Self {
        let len = Self::byte_len(width, height).expect("canvas dimensions overflow usize");
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    /// A canvas filled with a single (straight-alpha) color.
    pub fn filled(width: u32, height: u32, color: Color) -> Self {
        let mut canvas = Self::new(width, height);
        for px in canvas.data.chunks_exact_mut(BYTES_PER_PIXEL) {
            px.copy_from_slice(&color.to_array());
        }
        canvas
    }

    /// Adopt an existing RGBA8 buffer, e.g. straight from a capture backend.
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self, RenderError> {
        let expected =
            Self::byte_len(width, height).ok_or(RenderError::TooLarge { width, height })?;
        if data.len() != expected {
            return Err(RenderError::BufferSize {
                width,
                height,
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    fn byte_len(width: u32, height: u32) -> Option<usize> {
        (width as u64)
            .checked_mul(height as u64)?
            .checked_mul(BYTES_PER_PIXEL as u64)
            .and_then(|n| usize::try_from(n).ok())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Width and height as a `Vec2D`, which is how core expresses image size.
    pub fn size(&self) -> bettershot_core::math::Vec2D {
        bettershot_core::math::Vec2D::new(self.width as f32, self.height as f32)
    }

    /// The whole canvas as a rect in image-pixel space.
    pub fn bounds(&self) -> bettershot_core::math::Rect {
        bettershot_core::math::Rect::from_xywh(0.0, 0.0, self.width as f32, self.height as f32)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// # Panics
    /// If `(x, y)` is outside the canvas. Use [`Canvas::get_pixel`] when the
    /// coordinate may be out of range.
    pub fn pixel(&self, x: u32, y: u32) -> Color {
        self.get_pixel(x, y).unwrap_or_else(|| {
            panic!(
                "({x},{y}) is outside a {}x{} canvas",
                self.width, self.height
            )
        })
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> Option<Color> {
        let i = self.checked_index(x, y)?;
        Some(Color::new(
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ))
    }

    /// Overwrite a pixel. Out-of-range coordinates are ignored, matching the
    /// "clip, never panic" rule the rasterizer relies on.
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        if let Some(i) = self.checked_index(x, y) {
            self.data[i..i + BYTES_PER_PIXEL].copy_from_slice(&color.to_array());
        }
    }

    pub fn as_rgba8(&self) -> &[u8] {
        &self.data
    }

    pub fn as_rgba8_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_rgba8(self) -> Vec<u8> {
        self.data
    }

    #[inline]
    fn checked_index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL)
    }

    /// Source-over composite of `color` onto `(x, y)`, scaled by `coverage`
    /// (0..=1). Signed coordinates and out-of-range coverage are clipped rather
    /// than rejected: this is the single funnel every rasterizer path goes
    /// through, so it is where "never panic, never corrupt" is enforced.
    #[inline]
    pub(crate) fn blend(&mut self, x: i32, y: i32, color: Color, coverage: f32) {
        if x < 0 || y < 0 || !coverage.is_finite() || coverage <= 0.0 {
            return;
        }
        let (x, y) = (x as u32, y as u32);
        let Some(i) = self.checked_index(x, y) else {
            return;
        };
        blend_into(&mut self.data[i..i + BYTES_PER_PIXEL], color, coverage);
    }

    /// Replace a pixel outright, used by the image effects, which re-draw the
    /// base image rather than compositing on top of it.
    #[inline]
    pub(crate) fn put(&mut self, x: i32, y: i32, rgba: [u8; BYTES_PER_PIXEL]) {
        if x < 0 || y < 0 {
            return;
        }
        if let Some(i) = self.checked_index(x as u32, y as u32) {
            self.data[i..i + BYTES_PER_PIXEL].copy_from_slice(&rgba);
        }
    }

    /// Raw pixel with clamp-to-edge addressing. The blur reads neighbourhoods
    /// that hang off the image, and clamping (rather than treating the outside
    /// as transparent black) is what stops a dark halo forming along the edges.
    #[inline]
    pub(crate) fn sample_clamped(&self, x: i32, y: i32) -> [u8; BYTES_PER_PIXEL] {
        if self.is_empty() {
            return [0; BYTES_PER_PIXEL];
        }
        let x = x.clamp(0, self.width as i32 - 1) as u32;
        let y = y.clamp(0, self.height as i32 - 1) as u32;
        let i = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    // --- encoding / decoding ------------------------------------------------

    /// Encode as PNG in memory (for the clipboard, or for piping to stdout).
    pub fn encode_png(&self) -> Result<Vec<u8>, RenderError> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out).write_image(
            &self.data,
            self.width,
            self.height,
            ExtendedColorType::Rgba8,
        )?;
        Ok(out)
    }

    /// Write a PNG to disk.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), RenderError> {
        let path = path.as_ref();
        let file = std::fs::File::create(path).map_err(|e| RenderError::io(path, e))?;
        let mut writer = BufWriter::new(file);
        PngEncoder::new(&mut writer).write_image(
            &self.data,
            self.width,
            self.height,
            ExtendedColorType::Rgba8,
        )?;
        writer.flush().map_err(|e| RenderError::io(path, e))?;
        Ok(())
    }

    /// Decode any image format this build of `image` supports (PNG, JPEG,
    /// WebP), converting to RGBA8. Used for `--filename` and for stdin.
    pub fn decode(bytes: &[u8]) -> Result<Self, RenderError> {
        Self::from_dynamic(image::load_from_memory(bytes)?)
    }

    /// Like [`Canvas::decode`] but refuses anything that is not a PNG.
    pub fn decode_png(bytes: &[u8]) -> Result<Self, RenderError> {
        Self::from_dynamic(image::load_from_memory_with_format(
            bytes,
            ImageFormat::Png,
        )?)
    }

    /// Read and decode an image file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RenderError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| RenderError::io(path, e))?;
        Self::decode(&bytes)
    }

    /// Read and decode a PNG file.
    pub fn load_png(path: impl AsRef<Path>) -> Result<Self, RenderError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| RenderError::io(path, e))?;
        Self::decode_png(&bytes)
    }

    fn from_dynamic(image: image::DynamicImage) -> Result<Self, RenderError> {
        let rgba = image.to_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        Self::from_rgba8(width, height, rgba.into_raw())
    }
}

/// Straight-alpha source-over compositing.
///
/// Converting to premultiplied, blending, and converting back is the only way
/// to get the right answer when the destination is itself translucent — which
/// it is whenever an annotation lands on a transparent part of a capture.
#[inline]
pub(crate) fn blend_into(dst: &mut [u8], color: Color, coverage: f32) {
    let sa = (color.a as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    if sa >= 1.0 || da == 0.0 {
        // Fully opaque source, or nothing underneath to mix with.
        if sa >= 1.0 {
            dst.copy_from_slice(&[color.r, color.g, color.b, 255]);
        } else {
            dst.copy_from_slice(&[
                color.r,
                color.g,
                color.b,
                (sa * 255.0).round().clamp(0.0, 255.0) as u8,
            ]);
        }
        return;
    }

    let inv = 1.0 - sa;
    let out_a = sa + da * inv;
    let mix = |s: u8, d: u8| -> u8 {
        let s = s as f32 / 255.0;
        let d = d as f32 / 255.0;
        let v = (s * sa + d * da * inv) / out_a;
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let (r, g, b) = (
        mix(color.r, dst[0]),
        mix(color.g, dst[1]),
        mix(color.b, dst[2]),
    );
    dst[0] = r;
    dst[1] = g;
    dst[2] = b;
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_canvas_is_transparent_black() {
        let c = Canvas::new(3, 2);
        assert_eq!(c.width(), 3);
        assert_eq!(c.height(), 2);
        assert_eq!(c.as_rgba8().len(), 3 * 2 * 4);
        assert_eq!(c.pixel(2, 1), Color::transparent());
    }

    #[test]
    fn filled_paints_every_pixel() {
        let c = Canvas::filled(4, 4, Color::blue());
        assert!(
            (0..4).all(|y| (0..4).all(|x| c.pixel(x, y) == Color::blue())),
            "every pixel should be blue"
        );
    }

    #[test]
    fn from_rgba8_rejects_a_mismatched_buffer() {
        let err = Canvas::from_rgba8(2, 2, vec![0; 15]).unwrap_err();
        assert!(matches!(
            err,
            RenderError::BufferSize {
                expected: 16,
                actual: 15,
                ..
            }
        ));
    }

    #[test]
    fn get_pixel_is_none_outside_the_canvas() {
        let c = Canvas::new(2, 2);
        assert!(c.get_pixel(2, 0).is_none());
        assert!(c.get_pixel(0, 2).is_none());
        assert!(c.get_pixel(1, 1).is_some());
    }

    #[test]
    fn a_zero_sized_canvas_is_usable_and_inert() {
        let mut c = Canvas::new(0, 0);
        assert!(c.is_empty());
        c.blend(0, 0, Color::red(), 1.0);
        c.set_pixel(0, 0, Color::red());
        assert!(c.as_rgba8().is_empty());
        assert_eq!(c.sample_clamped(5, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn half_alpha_red_over_white_is_the_textbook_value() {
        let mut c = Canvas::filled(1, 1, Color::white());
        c.blend(0, 0, Color::new(255, 0, 0, 128), 1.0);
        // 128/255 = 0.50196; white contributes 0.49804 -> 127.
        assert_eq!(c.pixel(0, 0), Color::new(255, 127, 127, 255));
    }

    #[test]
    fn blending_onto_transparent_keeps_the_source_color() {
        let mut c = Canvas::new(1, 1);
        c.blend(0, 0, Color::new(10, 20, 30, 128), 1.0);
        assert_eq!(c.pixel(0, 0), Color::new(10, 20, 30, 128));
    }

    #[test]
    fn coverage_scales_the_source_alpha() {
        let mut c = Canvas::filled(1, 1, Color::black());
        c.blend(0, 0, Color::white(), 0.5);
        let p = c.pixel(0, 0);
        assert!((100..=155).contains(&p.r), "expected mid grey, got {p}");
    }

    #[test]
    fn blend_ignores_negative_coords_and_non_finite_coverage() {
        let mut c = Canvas::filled(2, 2, Color::white());
        c.blend(-1, 0, Color::red(), 1.0);
        c.blend(0, -5, Color::red(), 1.0);
        c.blend(99, 99, Color::red(), 1.0);
        c.blend(0, 0, Color::red(), f32::NAN);
        c.blend(0, 0, Color::red(), f32::INFINITY.recip() - 1.0);
        assert_eq!(c.pixel(0, 0), Color::white());
    }

    #[test]
    fn sample_clamped_replicates_the_border() {
        let mut c = Canvas::filled(2, 2, Color::black());
        c.set_pixel(0, 0, Color::red());
        assert_eq!(c.sample_clamped(-10, -10), Color::red().to_array());
        assert_eq!(c.sample_clamped(500, 500), Color::black().to_array());
    }

    #[test]
    fn png_round_trips_exactly_including_alpha() {
        let mut c = Canvas::filled(5, 3, Color::new(12, 200, 90, 255));
        c.set_pixel(1, 1, Color::new(1, 2, 3, 4));
        let bytes = c.encode_png().unwrap();
        let back = Canvas::decode_png(&bytes).unwrap();
        assert_eq!(back, c);
        // The format-guessing entry point must agree.
        assert_eq!(Canvas::decode(&bytes).unwrap(), c);
    }

    #[test]
    fn decoding_garbage_is_an_error_not_a_panic() {
        assert!(Canvas::decode_png(b"definitely not a png").is_err());
        assert!(Canvas::decode(b"").is_err());
    }
}
