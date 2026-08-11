//! [`RawFrame`] — the pixels a capture produced — plus the pure compositing
//! operations the backends build on.
//!
//! A frame is RGBA8, row-major, top-left origin, tightly packed (stride ==
//! `width * 4`). Backends are responsible for converting whatever the OS handed
//! them (BGRA, XRGB, premultiplied ...) into that shape, so everything above
//! this module only ever sees one layout.

use std::fmt;

use bettershot_core::{Rect, Vec2D};

use crate::{CaptureError, geometry::PixelRect};

/// Bytes per pixel in a [`RawFrame`].
pub const BYTES_PER_PIXEL: usize = 4;

/// A captured image and where on the virtual desktop it came from.
#[derive(Clone, PartialEq)]
pub struct RawFrame {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// RGBA8, row-major, **straight (non-premultiplied)** alpha,
    /// `len == width * height * 4`.
    ///
    /// Straight is the whole pipeline's convention, not just this type's: PNG
    /// stores straight RGBA8 and `bettershot_render`'s canvas is straight so
    /// that encode/decode round-trips stay bit-exact (premultiplying would
    /// destroy that for translucent pixels). A backend handed premultiplied
    /// pixels by the OS must convert here, or every consumer downstream will
    /// apply alpha twice.
    ///
    /// In practice a screenshot is opaque, where the two representations
    /// coincide; the difference only shows up on translucent window edges and
    /// in the transparent gaps [`stitch`] leaves between monitors.
    pub data: Vec<u8>,
    /// Top-left corner on the virtual desktop, in physical pixels. Negative
    /// values are normal on multi-monitor layouts.
    pub origin: Vec2D,
    /// HiDPI scale of the source display. Purely metadata: `data` is always
    /// physical pixels.
    pub scale_factor: f32,
}

impl fmt::Debug for RawFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never dump megabytes of pixels into a log line.
        f.debug_struct("RawFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("data", &format_args!("[{} bytes]", self.data.len()))
            .field("origin", &self.origin)
            .field("scale_factor", &self.scale_factor)
            .finish()
    }
}

impl RawFrame {
    /// Wrap an existing RGBA8 buffer, validating that it matches `width` and
    /// `height`.
    pub fn new(
        width: u32,
        height: u32,
        data: Vec<u8>,
        origin: Vec2D,
        scale_factor: f32,
    ) -> Result<Self, CaptureError> {
        let expected = expected_len(width, height)?;
        if data.len() != expected {
            return Err(CaptureError::invalid_frame(format!(
                "{width}x{height} RGBA needs {expected} bytes, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
            origin,
            scale_factor: if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            },
        })
    }

    /// A fully transparent frame — the background a stitched desktop starts
    /// from, so gaps between monitors are visibly "no monitor here" rather than
    /// arbitrary garbage.
    pub fn transparent(
        width: u32,
        height: u32,
        origin: Vec2D,
        scale_factor: f32,
    ) -> Result<Self, CaptureError> {
        let len = expected_len(width, height)?;
        Self::new(width, height, vec![0; len], origin, scale_factor)
    }

    /// A frame filled with one RGBA colour. Mostly useful for tests and for
    /// placeholder frames.
    pub fn filled(
        width: u32,
        height: u32,
        rgba: [u8; 4],
        origin: Vec2D,
        scale_factor: f32,
    ) -> Result<Self, CaptureError> {
        let len = expected_len(width, height)?;
        let mut data = Vec::with_capacity(len);
        for _ in 0..(len / BYTES_PER_PIXEL) {
            data.extend_from_slice(&rgba);
        }
        Self::new(width, height, data, origin, scale_factor)
    }

    /// Position and size on the virtual desktop, in physical pixels.
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.origin,
            Vec2D::new(self.width as f32, self.height as f32),
        )
    }

    /// Same as [`RawFrame::bounds`] but snapped to the integer pixel grid.
    pub fn pixel_bounds(&self) -> PixelRect {
        PixelRect::new(
            self.origin.x.round() as i32,
            self.origin.y.round() as i32,
            self.width,
            self.height,
        )
    }

    /// No pixels at all.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Number of pixels.
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// The RGBA value at frame-local `(x, y)`, or `None` when out of range.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let offset = self.offset(x, y)?;
        Some([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    /// Overwrite the RGBA value at frame-local `(x, y)`. Out-of-range writes
    /// are ignored.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if let Some(offset) = self.offset(x, y) {
            self.data[offset..offset + BYTES_PER_PIXEL].copy_from_slice(&rgba);
        }
    }

    fn offset(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL)
    }

    /// Cut a sub-image out of this frame.
    ///
    /// `region` is in **virtual-desktop** coordinates, like
    /// [`RawFrame::origin`], and is clipped to the frame first. The result
    /// carries the clipped origin, so it stays locatable on the desktop.
    ///
    /// This is how backends that can only grab the whole screen (notably the
    /// Wayland screenshot portal) serve monitor and region targets.
    pub fn crop(&self, region: Rect) -> Result<RawFrame, CaptureError> {
        let clipped = crate::geometry::clamp_region(region, self.bounds())?;
        let rect = PixelRect::from_rect(clipped)?;
        let local = rect.relative_to(self.origin);
        // `clamp_region` guarantees overlap; rounding can still nudge an edge,
        // so re-clamp into the buffer rather than trusting the arithmetic.
        let x0 = local.x.max(0) as u32;
        let y0 = local.y.max(0) as u32;
        let x1 = (local.right().max(0) as u32).min(self.width);
        let y1 = (local.bottom().max(0) as u32).min(self.height);
        if x1 <= x0 || y1 <= y0 {
            return Err(CaptureError::EmptyRegion);
        }

        let (w, h) = (x1 - x0, y1 - y0);
        let row_bytes = w as usize * BYTES_PER_PIXEL;
        let mut data = Vec::with_capacity(row_bytes * h as usize);
        for y in y0..y1 {
            let start = (y as usize * self.width as usize + x0 as usize) * BYTES_PER_PIXEL;
            data.extend_from_slice(&self.data[start..start + row_bytes]);
        }
        RawFrame::new(
            w,
            h,
            data,
            self.origin + Vec2D::new(x0 as f32, y0 as f32),
            self.scale_factor,
        )
    }
}

fn expected_len(width: u32, height: u32) -> Result<usize, CaptureError> {
    let pixels = u64::from(width) * u64::from(height);
    let bytes = pixels * BYTES_PER_PIXEL as u64;
    usize::try_from(bytes).map_err(|_| {
        CaptureError::invalid_frame(format!("{width}x{height} RGBA does not fit in memory"))
    })
}

/// Compose several frames into one image covering their combined bounding box.
///
/// This is what turns per-monitor grabs into a
/// [`crate::CaptureTarget::FullDesktop`] capture.
///
/// * The result's origin is the top-left of the bounding box, so negative
///   monitor origins are handled without shifting anyone's content.
/// * Areas no frame covers — the gaps in an L-shaped or staggered layout — stay
///   **fully transparent**. Nothing else is honest: there are no pixels there.
/// * Frames are drawn in slice order, so later frames win where they overlap
///   (mirrored displays). Backends should pass monitors back-to-front.
/// * The reported `scale_factor` is the largest of the inputs, **floored at
///   1.0**: a composite that claimed to be sub-unit scaled would tell a
///   consumer to enlarge it. See [`crate::VirtualDesktop::max_scale_factor`],
///   which uses the same floor, for the reasoning.
pub fn stitch(frames: &[RawFrame]) -> Result<RawFrame, CaptureError> {
    let non_empty: Vec<&RawFrame> = frames.iter().filter(|f| !f.is_empty()).collect();
    if non_empty.is_empty() {
        return Err(CaptureError::EmptyRegion);
    }

    let rects: Vec<PixelRect> = non_empty.iter().map(|f| f.pixel_bounds()).collect();
    let min_x = rects.iter().map(|r| r.x).min().expect("non-empty");
    let min_y = rects.iter().map(|r| r.y).min().expect("non-empty");
    let max_x = rects.iter().map(|r| r.right()).max().expect("non-empty");
    let max_y = rects.iter().map(|r| r.bottom()).max().expect("non-empty");

    let width = u32::try_from(i64::from(max_x) - i64::from(min_x))
        .map_err(|_| CaptureError::invalid_frame("stitched desktop is wider than u32"))?;
    let height = u32::try_from(i64::from(max_y) - i64::from(min_y))
        .map_err(|_| CaptureError::invalid_frame("stitched desktop is taller than u32"))?;

    let scale_factor = non_empty
        .iter()
        .map(|f| f.scale_factor)
        .fold(1.0_f32, f32::max);
    let mut out = RawFrame::transparent(
        width,
        height,
        Vec2D::new(min_x as f32, min_y as f32),
        scale_factor,
    )?;

    for (frame, rect) in non_empty.iter().zip(&rects) {
        let dst_x = (rect.x - min_x) as usize;
        let dst_y = (rect.y - min_y) as usize;
        let row_bytes = frame.width as usize * BYTES_PER_PIXEL;
        for row in 0..frame.height as usize {
            let src = row * row_bytes;
            let dst = ((dst_y + row) * width as usize + dst_x) * BYTES_PER_PIXEL;
            out.data[dst..dst + row_bytes].copy_from_slice(&frame.data[src..src + row_bytes]);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn solid(w: u32, h: u32, colour: [u8; 4], x: f32, y: f32) -> RawFrame {
        RawFrame::filled(w, h, colour, Vec2D::new(x, y), 1.0).unwrap()
    }

    #[test]
    fn new_validates_the_buffer_length() {
        assert!(RawFrame::new(2, 2, vec![0; 16], Vec2D::ZERO, 1.0).is_ok());
        assert!(matches!(
            RawFrame::new(2, 2, vec![0; 15], Vec2D::ZERO, 1.0),
            Err(CaptureError::InvalidFrame(_))
        ));
    }

    #[test]
    fn new_sanitizes_broken_scale_factors() {
        let f = RawFrame::new(1, 1, vec![0; 4], Vec2D::ZERO, 0.0).unwrap();
        assert_eq!(f.scale_factor, 1.0);
        let f = RawFrame::new(1, 1, vec![0; 4], Vec2D::ZERO, f32::NAN).unwrap();
        assert_eq!(f.scale_factor, 1.0);
    }

    #[test]
    fn filled_paints_every_pixel() {
        let f = solid(3, 2, RED, 0.0, 0.0);
        assert_eq!(f.data.len(), 3 * 2 * 4);
        assert_eq!(f.pixel_count(), 6);
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(f.pixel(x, y), Some(RED));
            }
        }
        assert_eq!(f.pixel(3, 0), None);
        assert_eq!(f.pixel(0, 2), None);
    }

    #[test]
    fn transparent_frames_start_fully_clear() {
        let f = RawFrame::transparent(2, 2, Vec2D::ZERO, 1.0).unwrap();
        assert!(f.data.iter().all(|b| *b == 0));
    }

    #[test]
    fn set_pixel_ignores_out_of_range_writes() {
        let mut f = solid(2, 2, RED, 0.0, 0.0);
        f.set_pixel(1, 1, GREEN);
        f.set_pixel(9, 9, BLUE);
        assert_eq!(f.pixel(1, 1), Some(GREEN));
        assert_eq!(f.pixel(0, 0), Some(RED));
    }

    #[test]
    fn bounds_track_the_origin() {
        let f = solid(100, 50, RED, -1280.0, -200.0);
        assert_eq!(f.bounds(), Rect::from_xywh(-1280.0, -200.0, 100.0, 50.0));
        assert_eq!(f.pixel_bounds(), PixelRect::new(-1280, -200, 100, 50));
    }

    #[test]
    fn debug_does_not_dump_the_pixel_buffer() {
        let text = format!("{:?}", solid(64, 64, RED, 0.0, 0.0));
        assert!(text.contains("16384 bytes"), "{text}");
    }

    #[test]
    fn stitch_of_nothing_is_an_empty_region() {
        assert!(matches!(stitch(&[]), Err(CaptureError::EmptyRegion)));
        let empty = RawFrame::transparent(0, 0, Vec2D::ZERO, 1.0).unwrap();
        assert!(matches!(stitch(&[empty]), Err(CaptureError::EmptyRegion)));
    }

    #[test]
    fn stitch_places_two_frames_side_by_side() {
        let left = solid(2, 2, RED, 0.0, 0.0);
        let right = solid(2, 2, GREEN, 2.0, 0.0);
        let out = stitch(&[left, right]).unwrap();

        assert_eq!(out.width, 4);
        assert_eq!(out.height, 2);
        assert_eq!(out.origin, Vec2D::ZERO);
        for y in 0..2 {
            assert_eq!(out.pixel(0, y), Some(RED));
            assert_eq!(out.pixel(1, y), Some(RED));
            assert_eq!(out.pixel(2, y), Some(GREEN));
            assert_eq!(out.pixel(3, y), Some(GREEN));
        }
    }

    #[test]
    fn stitch_handles_a_negative_origin_by_rebasing_the_result() {
        let left = solid(2, 2, RED, -2.0, 0.0);
        let right = solid(2, 2, GREEN, 0.0, 0.0);
        let out = stitch(&[left, right]).unwrap();

        assert_eq!((out.width, out.height), (4, 2));
        assert_eq!(out.origin, Vec2D::new(-2.0, 0.0));
        assert_eq!(out.pixel(0, 0), Some(RED));
        assert_eq!(out.pixel(2, 0), Some(GREEN));
    }

    #[test]
    fn stitch_handles_negative_origins_in_both_axes() {
        let above_left = solid(2, 2, RED, -2.0, -3.0);
        let main = solid(2, 2, GREEN, 0.0, 0.0);
        let out = stitch(&[above_left, main]).unwrap();

        assert_eq!((out.width, out.height), (4, 5));
        assert_eq!(out.origin, Vec2D::new(-2.0, -3.0));
        assert_eq!(out.pixel(0, 0), Some(RED));
        assert_eq!(out.pixel(2, 3), Some(GREEN));
        // The two rectangles do not touch: everything else is transparent.
        assert_eq!(out.pixel(3, 0), Some(CLEAR));
        assert_eq!(out.pixel(0, 4), Some(CLEAR));
    }

    #[test]
    fn stitch_fills_a_vertical_gap_with_transparency() {
        let top = solid(2, 2, RED, 0.0, 0.0);
        let bottom = solid(2, 2, BLUE, 0.0, 5.0);
        let out = stitch(&[top, bottom]).unwrap();

        assert_eq!((out.width, out.height), (2, 7));
        for y in 0..2 {
            assert_eq!(out.pixel(0, y), Some(RED));
        }
        for y in 2..5 {
            assert_eq!(out.pixel(0, y), Some(CLEAR), "row {y} should be the gap");
            assert_eq!(out.pixel(1, y), Some(CLEAR));
        }
        for y in 5..7 {
            assert_eq!(out.pixel(0, y), Some(BLUE));
        }
    }

    #[test]
    fn stitch_fills_an_l_shaped_layout_gap() {
        // Wide monitor on top, narrow one below-left: the bottom-right corner
        // of the bounding box is covered by nothing.
        let top = solid(4, 1, RED, 0.0, 0.0);
        let bottom_left = solid(2, 1, GREEN, 0.0, 1.0);
        let out = stitch(&[top, bottom_left]).unwrap();

        assert_eq!((out.width, out.height), (4, 2));
        assert_eq!(out.pixel(3, 0), Some(RED));
        assert_eq!(out.pixel(1, 1), Some(GREEN));
        assert_eq!(out.pixel(2, 1), Some(CLEAR));
        assert_eq!(out.pixel(3, 1), Some(CLEAR));
    }

    #[test]
    fn stitch_lets_later_frames_win_where_they_overlap() {
        let back = solid(2, 2, RED, 0.0, 0.0);
        let front = solid(2, 2, GREEN, 1.0, 1.0);
        let out = stitch(&[back, front]).unwrap();

        assert_eq!((out.width, out.height), (3, 3));
        assert_eq!(out.pixel(0, 0), Some(RED));
        assert_eq!(out.pixel(1, 1), Some(GREEN)); // overlap
        assert_eq!(out.pixel(2, 2), Some(GREEN));
    }

    #[test]
    fn stitch_reports_the_largest_source_scale_factor() {
        let lo = RawFrame::filled(2, 2, RED, Vec2D::ZERO, 1.0).unwrap();
        let hi = RawFrame::filled(2, 2, GREEN, Vec2D::new(2.0, 0.0), 1.5).unwrap();
        assert_eq!(stitch(&[lo, hi]).unwrap().scale_factor, 1.5);
    }

    #[test]
    fn stitch_never_reports_a_scale_factor_below_one() {
        // `RawFrame::new` keeps any positive finite scale, so sub-unit inputs
        // are reachable; the composite still reports 1.0, because a frame in
        // physical pixels must never ask a consumer to enlarge it.
        let a = RawFrame::filled(2, 2, RED, Vec2D::ZERO, 0.5).unwrap();
        let b = RawFrame::filled(2, 2, GREEN, Vec2D::new(2.0, 0.0), 0.75).unwrap();
        assert_eq!(a.scale_factor, 0.5);
        assert_eq!(stitch(&[a, b]).unwrap().scale_factor, 1.0);
    }

    #[test]
    fn stitch_skips_empty_frames_but_keeps_the_rest() {
        let empty = RawFrame::transparent(0, 0, Vec2D::new(-1000.0, 0.0), 1.0).unwrap();
        let real = solid(2, 2, RED, 0.0, 0.0);
        let out = stitch(&[empty, real]).unwrap();
        assert_eq!((out.width, out.height), (2, 2));
        assert_eq!(out.origin, Vec2D::ZERO);
    }

    #[test]
    fn stitch_of_a_single_frame_is_a_copy() {
        let one = solid(3, 2, BLUE, -5.0, 7.0);
        let out = stitch(std::slice::from_ref(&one)).unwrap();
        assert_eq!(out.width, one.width);
        assert_eq!(out.height, one.height);
        assert_eq!(out.origin, one.origin);
        assert_eq!(out.data, one.data);
    }

    #[test]
    fn crop_cuts_a_sub_image_in_virtual_coordinates() {
        let mut frame = solid(4, 4, RED, 100.0, 100.0);
        frame.set_pixel(2, 1, GREEN);
        let cropped = frame.crop(Rect::from_xywh(102.0, 101.0, 2.0, 2.0)).unwrap();

        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert_eq!(cropped.origin, Vec2D::new(102.0, 101.0));
        assert_eq!(cropped.pixel(0, 0), Some(GREEN));
        assert_eq!(cropped.pixel(1, 0), Some(RED));
    }

    #[test]
    fn crop_clips_to_the_frame_and_keeps_the_clipped_origin() {
        let frame = solid(4, 4, RED, 0.0, 0.0);
        let cropped = frame.crop(Rect::from_xywh(-10.0, 2.0, 20.0, 20.0)).unwrap();
        assert_eq!((cropped.width, cropped.height), (4, 2));
        assert_eq!(cropped.origin, Vec2D::new(0.0, 2.0));
    }

    #[test]
    fn crop_rejects_regions_outside_the_frame() {
        let frame = solid(4, 4, RED, 0.0, 0.0);
        assert!(matches!(
            frame.crop(Rect::from_xywh(100.0, 100.0, 4.0, 4.0)),
            Err(CaptureError::EmptyRegion)
        ));
        assert!(matches!(
            frame.crop(Rect::from_xywh(0.0, 0.0, 0.0, 4.0)),
            Err(CaptureError::EmptyRegion)
        ));
    }

    #[test]
    fn crop_preserves_the_scale_factor() {
        let frame = RawFrame::filled(4, 4, RED, Vec2D::ZERO, 2.0).unwrap();
        assert_eq!(
            frame
                .crop(Rect::from_xywh(1.0, 1.0, 2.0, 2.0))
                .unwrap()
                .scale_factor,
            2.0
        );
    }

    #[test]
    fn stitch_then_crop_round_trips_a_monitor_out_of_the_desktop() {
        // The portal path in miniature: grab everything, then cut one monitor.
        let left = solid(2, 2, RED, -2.0, 0.0);
        let right = solid(2, 2, GREEN, 0.0, 0.0);
        let desktop = stitch(&[left, right]).unwrap();
        let recovered = desktop.crop(Rect::from_xywh(-2.0, 0.0, 2.0, 2.0)).unwrap();
        assert_eq!(recovered.origin, Vec2D::new(-2.0, 0.0));
        assert_eq!(recovered.data, solid(2, 2, RED, 0.0, 0.0).data);
    }
}
