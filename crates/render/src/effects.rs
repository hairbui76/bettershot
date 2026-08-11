//! Blur and pixelate, sampled from the **base** image.
//!
//! # The rule that makes redaction trustworthy
//!
//! An [`ImageEffect`] re-draws a region of the *original, unannotated*
//! screenshot. It never reads the working canvas. Two consequences fall out of
//! that, and both are the reason the rule exists:
//!
//! 1. Effects are idempotent and order-independent with respect to what has
//!    already been drawn. Blurring a region twice, or drawing a rectangle and
//!    then blurring over it, both yield the blur of the *original* pixels.
//! 2. A redaction cannot be defeated by drawing something under it. If the blur
//!    sampled the working canvas, an annotation placed underneath would leak
//!    into the "blurred" output and, worse, the blur radius would be effectively
//!    applied twice on re-render, making the result depend on how many times the
//!    editor happened to repaint.
//!
//! Because effects *replace* rather than composite, they also erase any
//! annotation already drawn inside their rect. That is deliberate: a blur is a
//! statement about what the viewer may see there, not a translucent overlay.
//!
//! # What you see is what you save
//!
//! The editor shows a live preview by pre-processing the whole base image once
//! per effect strength and uploading it as a texture; export re-runs the effect
//! over just the rect each annotation covers. Those are different call shapes
//! over the same pixels, and for a redaction tool "the preview and the file
//! agree" is a correctness property, not a nicety — otherwise the user approves
//! one set of pixels and ships another.
//!
//! So this module guarantees **region invariance**:
//!
//! > For any rect `R`, [`apply_effect`] over the whole image, cropped to `R`,
//! > is byte-for-byte equal to [`apply_effect_in_region`] over `R`.
//!
//! Three things are needed to make that true, and each of them costs something
//! that a naive implementation would have spent differently:
//!
//! * **Pixelate blocks are anchored to the image origin**, not to the rect. A
//!   rect-anchored mosaic cannot survive whole-image preprocessing at all (every
//!   rect would need its own texture), and image-anchoring is the nicer
//!   behaviour anyway: two redactions dragged over adjacent parts of the same
//!   line of text share one block grid instead of visibly disagreeing. The price
//!   is a partial block at the rect's edge, which is the same sliver the
//!   image's own right and bottom edges already have.
//! * **A block's average covers the whole block**, clipped to the image, not
//!   just the part inside the rect. Averaging only the covered part would make
//!   the colour depend on where the rect was dragged.
//! * **Blur padding stops at the image edge**, where sampling replicates the
//!   border, so a region blur clamps exactly where a whole-image blur clamps.
//!   Inside the image the region is padded by the blur's full support, so the
//!   pixels written are the ones a whole-image pass would have written.
//!
//! # Blur quality and arithmetic
//!
//! Three box passes converge on a Gaussian (central limit theorem) and cost
//! O(1) per pixel per pass via a sliding window sum, which is what keeps a 30px
//! blur over a 4K image affordable: the work is independent of the radius.
//!
//! The passes carry 8.8 fixed-point integers rather than floats, which is what
//! makes region invariance hold unconditionally rather than merely usually. A
//! window sum has to be *exactly* the sum of the window for a pixel to depend on
//! its neighbourhood and on nothing about the traversal that reached it — and an
//! `f32` sum stops being exact as soon as it can exceed 2^24, which here means a
//! box radius over about 128. Under that it is exact by luck, which is the worst
//! way for this to be true: a float implementation passes every test in this
//! module and then disagrees with itself by a level the first time someone
//! raises `annotation_size_factor`. Eight fractional bits keep a channel in a
//! `u16` (255 << 8 = 65280), halve the working set against `f32`, and bound the
//! per-pass rounding error at 1/512 of a channel level.
//!
//! Two things make the difference between "correct" and "fast enough to preview
//! a 4K screenshot with", and both are load-bearing rather than micro-tuning:
//!
//! * Dividing the window sum by the window width is done with a reciprocal
//!   multiply. A 64-bit `div` is an order of magnitude slower than a multiply,
//!   and there are four of them per pixel per pass — it dominated everything
//!   else put together. [`BoxAverage`] proves the substitution is exact, so this
//!   costs no accuracy and no determinism.
//! * The vertical pass runs over tiles of columns gathered into contiguous
//!   strips. Walking a column directly loads a 64-byte cache line to use eight
//!   bytes of it; gathering a tile first means every byte fetched is used.

use bettershot_core::math::Rect;
use bettershot_core::painter::ImageEffect;

use crate::canvas::{BYTES_PER_PIXEL, Canvas};
use crate::raster::PixelBox;

/// Box passes used to approximate a Gaussian.
const BLUR_PASSES: usize = 3;
/// Fractional bits carried between box passes. See the module docs.
const FRACTION_BITS: u32 = 8;
/// Fixed-point 1.0.
const ONE: u32 = 1 << FRACTION_BITS;
/// The largest box radius a blur will use, however absurd the requested radius.
const MAX_BOX_RADIUS: usize = 4096;
/// Columns gathered into a contiguous strip by the vertical pass. Eight pixels
/// is 64 bytes — exactly one cache line per row.
const COLUMN_TILE: usize = 8;

/// Apply `effect` to the whole of `base`, returning a new canvas.
///
/// This is what a live preview uploads as a texture: process once, then draw
/// the region each annotation covers. The result is byte-for-byte what
/// [`apply_effect_in_region`] (and therefore export) produces over any rect —
/// see the module docs for the invariants that guarantee it.
///
/// An effect too weak to see (`ImageEffect::is_visible`) returns `base`
/// unchanged, so a caller can skip the upload entirely.
pub fn apply_effect(base: &Canvas, effect: ImageEffect) -> Canvas {
    let mut out = base.clone();
    apply_effect_in_region(&mut out, base, base.bounds(), effect);
    out
}

/// Apply `effect` to `rect`, reading from `base` and writing into `dst`.
///
/// This is the export path: `dst` is the working canvas carrying annotations
/// already drawn, and the effect replaces the pixels it covers with a fresh
/// treatment of the *base* image underneath.
///
/// `dst` and `base` need not be the same size; only pixels present in both are
/// written. Sampling always uses the full base, so what lands in `dst` does not
/// depend on how big `dst` is.
pub fn apply_effect_in_region(dst: &mut Canvas, base: &Canvas, rect: Rect, effect: ImageEffect) {
    if dst.is_empty() || base.is_empty() || !effect.is_visible() {
        return;
    }
    // Only pixels that exist in both images can be recomputed.
    let width = dst.width().min(base.width());
    let height = dst.height().min(base.height());
    let Some(area) = crate::raster::clip_to_pixels(rect, width, height) else {
        return;
    };

    match effect {
        ImageEffect::Blur { radius } => blur(dst, base, area, radius),
        ImageEffect::Pixelate { block_size } => pixelate(dst, base, area, block_size),
    }
}

fn blur(canvas: &mut Canvas, base: &Canvas, area: PixelBox, radius: f32) {
    if !radius.is_finite() {
        return;
    }
    // Three box passes of radius r spread light over roughly 3r pixels, so this
    // is the box radius that makes the visible blur match the requested one.
    let box_radius =
        ((radius / BLUR_PASSES as f32).round() as i64).clamp(1, MAX_BOX_RADIUS as i64) as usize;
    // Pad the working region by the blur's full support, so every pixel inside
    // `area` sees the same neighbourhood a whole-image pass would have given it.
    // Padding past the image is pointless — sampling already replicates the
    // border, so the buffer edge and the image edge clamp identically — and
    // would let a large radius allocate a buffer far bigger than the image.
    let (bw, bh) = (base.width() as i32, base.height() as i32);
    let want = (box_radius * BLUR_PASSES) as i32 + 1;
    let pad_l = want.min(area.x0);
    let pad_t = want.min(area.y0);
    let pad_r = want.min(bw - area.x1);
    let pad_b = want.min(bh - area.y1);

    let (sx, sy) = (area.x0 - pad_l, area.y0 - pad_t);
    let sw = area.width() + (pad_l + pad_r) as usize;
    let sh = area.height() + (pad_t + pad_b) as usize;
    if sw == 0 || sh == 0 {
        return;
    }

    let mut buffer = vec![0u16; sw * sh * BYTES_PER_PIXEL];
    for row in 0..sh {
        for col in 0..sw {
            let px = base.sample_clamped(sx + col as i32, sy + row as i32);
            let at = (row * sw + col) * BYTES_PER_PIXEL;
            for (slot, value) in buffer[at..at + BYTES_PER_PIXEL].iter_mut().zip(px) {
                *slot = (value as u16) << FRACTION_BITS;
            }
        }
    }

    // Two scratch buffers, reused by every pass: one line of output (a box pass
    // cannot write over its own input) and one strip of gathered columns.
    let avg = BoxAverage::new(box_radius);
    let mut line = vec![0u16; sw.max(sh) * BYTES_PER_PIXEL];
    let mut strip = vec![0u16; sh * COLUMN_TILE * BYTES_PER_PIXEL];
    for _ in 0..BLUR_PASSES {
        box_pass_horizontal(&mut buffer, &mut line, sw, sh, box_radius, avg);
        box_pass_vertical(&mut buffer, &mut strip, &mut line, sw, sh, box_radius, avg);
    }

    for py in area.y0..area.y1 {
        for px in area.x0..area.x1 {
            let at = (((py - sy) as usize) * sw + (px - sx) as usize) * BYTES_PER_PIXEL;
            let mut rgba = [0u8; BYTES_PER_PIXEL];
            for (out, value) in rgba.iter_mut().zip(&buffer[at..at + BYTES_PER_PIXEL]) {
                *out = ((*value as u32 + ONE / 2) >> FRACTION_BITS).min(255) as u8;
            }
            canvas.put(px, py, rgba);
        }
    }
}

/// Rounding division by a box window, as a reciprocal multiply.
///
/// `(sum + window / 2) / window` for every channel of every pixel of every pass
/// is where a straightforward implementation spends most of its time, because a
/// 64-bit integer division is an order of magnitude slower than a multiply. The
/// substitution is exact, not approximate, given the bounds this module works
/// in — see [`BoxAverage::apply`].
#[derive(Debug, Clone, Copy)]
struct BoxAverage {
    window: u32,
    half: u32,
    /// `ceil(2^RECIPROCAL_BITS / window)`.
    reciprocal: u64,
}

/// Chosen so the reciprocal multiply is exact and cannot overflow; the argument
/// is in [`BoxAverage::apply`].
const RECIPROCAL_BITS: u32 = 46;

impl BoxAverage {
    fn new(radius: usize) -> Self {
        let window = (2 * radius + 1) as u32;
        Self {
            window,
            half: window / 2,
            reciprocal: (1u64 << RECIPROCAL_BITS).div_ceil(window as u64),
        }
    }

    /// The rounded mean of a window whose channel values are all `<= 255 <<
    /// FRACTION_BITS`.
    ///
    /// Exactness: write `n = sum + half` and `d = window`. With
    /// `m = ceil(2^k / d)` the error `e = m*d - 2^k` satisfies `e < d`, and
    /// `floor(n*m / 2^k) == floor(n/d)` whenever `n*e < 2^k`. Here every summand
    /// is at most `255 << 8 = 65280 < 2^16`, so `n < d * 2^16`, giving
    /// `n*e < d^2 * 2^16 <= 8193^2 * 2^16 < 2^42 <= 2^k`. The same bound caps
    /// the product at `n*m <= 2^16 * (2^k + d) < 2^63`, so nothing overflows —
    /// and it is why a window sum fits in a `u32` in the first place.
    #[inline]
    fn apply(&self, sum: u32) -> u16 {
        debug_assert!(sum as u64 <= self.window as u64 * ((255 << FRACTION_BITS) as u64));
        ((((sum + self.half) as u64) * self.reciprocal) >> RECIPROCAL_BITS) as u16
    }
}

/// One box pass over a contiguous run of `n` pixels, clamping at both ends.
///
/// `src` and `dst` must be separate: a sliding window still needs the value
/// leaving it, which an in-place write would already have destroyed.
///
/// The four channels are kept in a fixed-size array and every slice taken here
/// has a length the compiler can see, which is what lets it drop the bounds
/// checks and run the four lanes at once — this is the crate's hottest loop by
/// a wide margin.
fn box_line(src: &[u16], dst: &mut [u16], n: usize, radius: usize, avg: BoxAverage) {
    debug_assert!(n > 0 && src.len() >= n * BYTES_PER_PIXEL && dst.len() >= n * BYTES_PER_PIXEL);
    let last = (n - 1) * BYTES_PER_PIXEL;

    // The window at x = 0 reaches `radius` pixels off the near end, and every
    // sample that falls off replicates the border pixel.
    let mut sum: [u32; BYTES_PER_PIXEL] =
        std::array::from_fn(|c| src[c] as u32 * (radius as u32 + 1));
    for x in 1..=radius {
        let at = x.min(n - 1) * BYTES_PER_PIXEL;
        for (slot, value) in sum.iter_mut().zip(&src[at..at + BYTES_PER_PIXEL]) {
            *slot += *value as u32;
        }
    }

    for x in 0..n {
        let at = x * BYTES_PER_PIXEL;
        for (out, total) in dst[at..at + BYTES_PER_PIXEL].iter_mut().zip(sum) {
            *out = avg.apply(total);
        }
        // Slide: the sample entering on the right, the one leaving on the left,
        // both clamped to the ends. `saturating_sub` *is* the left clamp.
        let entering = ((x + radius + 1) * BYTES_PER_PIXEL).min(last);
        let leaving = x.saturating_sub(radius) * BYTES_PER_PIXEL;
        let a = &src[entering..entering + BYTES_PER_PIXEL];
        let b = &src[leaving..leaving + BYTES_PER_PIXEL];
        for c in 0..BYTES_PER_PIXEL {
            sum[c] = sum[c] + a[c] as u32 - b[c] as u32;
        }
    }
}

/// One horizontal box pass, row by row.
fn box_pass_horizontal(
    buf: &mut [u16],
    line: &mut [u16],
    w: usize,
    h: usize,
    radius: usize,
    avg: BoxAverage,
) {
    let stride = w * BYTES_PER_PIXEL;
    for row in 0..h {
        let at = row * stride;
        box_line(&buf[at..at + stride], line, w, radius, avg);
        buf[at..at + stride].copy_from_slice(&line[..stride]);
    }
}

/// One vertical box pass, over tiles of columns gathered into contiguous
/// strips so that the strided access pattern is paid for once per cache line
/// instead of once per pixel.
fn box_pass_vertical(
    buf: &mut [u16],
    strip: &mut [u16],
    line: &mut [u16],
    w: usize,
    h: usize,
    radius: usize,
    avg: BoxAverage,
) {
    let stride = w * BYTES_PER_PIXEL;
    let mut tile = 0;
    while tile < w {
        let cols = COLUMN_TILE.min(w - tile);
        for y in 0..h {
            let at = y * stride + tile * BYTES_PER_PIXEL;
            let row = &buf[at..at + cols * BYTES_PER_PIXEL];
            for (col, px) in row.chunks_exact(BYTES_PER_PIXEL).enumerate() {
                let to = (col * h + y) * BYTES_PER_PIXEL;
                strip[to..to + BYTES_PER_PIXEL].copy_from_slice(px);
            }
        }
        for col in 0..cols {
            let at = col * h * BYTES_PER_PIXEL;
            box_line(&strip[at..at + h * BYTES_PER_PIXEL], line, h, radius, avg);
            for y in 0..h {
                let to = y * stride + (tile + col) * BYTES_PER_PIXEL;
                buf[to..to + BYTES_PER_PIXEL]
                    .copy_from_slice(&line[y * BYTES_PER_PIXEL..(y + 1) * BYTES_PER_PIXEL]);
            }
        }
        tile += cols;
    }
}

fn pixelate(canvas: &mut Canvas, base: &Canvas, area: PixelBox, block_size: f32) {
    if !block_size.is_finite() {
        return;
    }
    let block = (block_size.round() as i32).clamp(1, 4096);
    let (bw, bh) = (base.width() as i32, base.height() as i32);

    // Blocks tile from the image origin, and each one averages its whole cell
    // (clipped to the image), regardless of how much of it the rect covers.
    // That is what lets the editor mosaic the image once and draw any region of
    // the result; see the module docs.
    let mut y = area.y0 - area.y0.rem_euclid(block);
    while y < area.y1 {
        let y_end = y + block;
        let mut x = area.x0 - area.x0.rem_euclid(block);
        while x < area.x1 {
            let x_end = x + block;
            let mut sums = [0u64; BYTES_PER_PIXEL];
            let mut count = 0u64;
            for by in y..y_end.min(bh) {
                for bx in x..x_end.min(bw) {
                    let px = base.sample_clamped(bx, by);
                    for (sum, value) in sums.iter_mut().zip(px) {
                        *sum += value as u64;
                    }
                    count += 1;
                }
            }
            // `NonZero` rather than a `count > 0` guard, so the divisor carries
            // its own proof and the division cannot be a division by zero.
            if let Some(count) = std::num::NonZeroU64::new(count) {
                let n = count.get();
                let mut rgba = [0u8; BYTES_PER_PIXEL];
                for (out, sum) in rgba.iter_mut().zip(sums) {
                    // + n/2 rounds to nearest rather than truncating.
                    *out = ((sum + n / 2) / n) as u8;
                }
                // Only the part of the cell the rect actually covers is written.
                for by in y.max(area.y0)..y_end.min(area.y1) {
                    for bx in x.max(area.x0)..x_end.min(area.x1) {
                        canvas.put(bx, by, rgba);
                    }
                }
            }
            x = x_end;
        }
        y = y_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::style::Color;

    /// A deterministic high-frequency checkerboard: maximum neighbour
    /// difference, so any smoothing is unambiguous.
    fn checkerboard(w: u32, h: u32) -> Canvas {
        let mut c = Canvas::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let on = (x + y) % 2 == 0;
                c.set_pixel(x, y, if on { Color::white() } else { Color::black() });
            }
        }
        c
    }

    /// A base with no symmetry at all, so an off-by-one in block alignment or
    /// blur padding cannot hide behind a repeating pattern.
    fn noise(w: u32, h: u32) -> Canvas {
        let mut c = Canvas::new(w, h);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for y in 0..h {
            for x in 0..w {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let b = state.to_le_bytes();
                c.set_pixel(x, y, Color::new(b[0], b[1], b[2], 255));
            }
        }
        c
    }

    /// Mean absolute difference between horizontally adjacent pixels.
    fn roughness(canvas: &Canvas, area: (u32, u32, u32, u32)) -> f32 {
        let (x0, y0, x1, y1) = area;
        let mut total = 0.0f32;
        let mut n = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 - 1 {
                let a = canvas.pixel(x, y).r as f32;
                let b = canvas.pixel(x + 1, y).r as f32;
                total += (a - b).abs();
                n += 1.0;
            }
        }
        total / n.max(1.0)
    }

    /// Assert that a whole-image pass and a region pass agree everywhere in
    /// `rect`, and that the region pass touched nothing else.
    fn assert_region_matches_whole_image(base: &Canvas, rect: Rect, effect: ImageEffect) {
        let whole = apply_effect(base, effect);
        let mut region = base.clone();
        apply_effect_in_region(&mut region, base, rect, effect);

        let inside =
            crate::raster::clip_to_pixels(rect, base.width(), base.height()).expect("visible rect");
        for y in 0..base.height() {
            for x in 0..base.width() {
                let covered = (inside.x0..inside.x1).contains(&(x as i32))
                    && (inside.y0..inside.y1).contains(&(y as i32));
                let want = if covered { &whole } else { base };
                assert_eq!(
                    region.pixel(x, y),
                    want.pixel(x, y),
                    "({x},{y}) covered={covered} for {effect:?} over {rect:?}",
                );
            }
        }
    }

    #[test]
    fn blur_smooths_the_region_it_covers() {
        let base = checkerboard(60, 60);
        let mut canvas = base.clone();
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(10.0, 10.0, 40.0, 40.0),
            ImageEffect::Blur { radius: 9.0 },
        );

        let before = roughness(&base, (15, 15, 45, 45));
        let after = roughness(&canvas, (15, 15, 45, 45));
        assert!(before > 200.0, "the checkerboard should start rough");
        assert!(after < 20.0, "blur should smooth it, got {after}");
    }

    #[test]
    fn blur_leaves_pixels_outside_the_rect_alone() {
        let base = checkerboard(40, 40);
        let mut canvas = base.clone();
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
            ImageEffect::Blur { radius: 6.0 },
        );
        for (x, y) in [(0, 0), (9, 9), (30, 30), (39, 39), (5, 20), (35, 20)] {
            assert_eq!(canvas.pixel(x, y), base.pixel(x, y), "at ({x},{y})");
        }
    }

    #[test]
    fn blur_at_the_image_edge_does_not_darken() {
        let base = Canvas::filled(20, 20, Color::rgb(200, 200, 200));
        let mut canvas = base.clone();
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(0.0, 0.0, 20.0, 20.0),
            ImageEffect::Blur { radius: 9.0 },
        );
        // Clamp-to-edge means a flat image blurs to itself, corners included.
        for (x, y) in [(0, 0), (19, 0), (0, 19), (19, 19), (10, 10)] {
            assert_eq!(canvas.pixel(x, y), Color::rgb(200, 200, 200));
        }
    }

    #[test]
    fn pixelate_produces_uniform_blocks() {
        let base = checkerboard(40, 40);
        let mut canvas = base.clone();
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(8.0, 8.0, 24.0, 24.0),
            ImageEffect::Pixelate { block_size: 8.0 },
        );
        // Blocks tile from the image origin; this rect happens to start on one.
        let expected = canvas.pixel(8, 8);
        for y in 8..16 {
            for x in 8..16 {
                assert_eq!(canvas.pixel(x, y), expected, "block pixel ({x},{y})");
            }
        }
        // A checkerboard averages to mid grey.
        assert!((120..=136).contains(&expected.r), "{expected}");
        assert_eq!(canvas.pixel(7, 7), base.pixel(7, 7), "outside the rect");
    }

    #[test]
    fn pixelate_blocks_are_anchored_to_the_image_not_the_rect() {
        let base = noise(48, 48);
        // Two rects on the same block row, dragged from different offsets.
        let mut a = base.clone();
        apply_effect_in_region(
            &mut a,
            &base,
            Rect::from_xywh(3.0, 0.0, 20.0, 16.0),
            ImageEffect::Pixelate { block_size: 8.0 },
        );
        let mut b = base.clone();
        apply_effect_in_region(
            &mut b,
            &base,
            Rect::from_xywh(9.0, 0.0, 14.0, 16.0),
            ImageEffect::Pixelate { block_size: 8.0 },
        );
        // Where they overlap, both must show the same grid-aligned blocks: the
        // colour of a block cannot depend on where the drag started.
        for y in 0..16 {
            for x in 9..23 {
                assert_eq!(a.pixel(x, y), b.pixel(x, y), "at ({x},{y})");
            }
        }
        // And a block boundary really does fall at a multiple of 8, not at the
        // rect's own left edge.
        assert_eq!(a.pixel(8, 0), a.pixel(15, 0), "8..16 is one block");
        assert_ne!(a.pixel(7, 0), a.pixel(8, 0), "a new block starts at 8");
    }

    #[test]
    fn a_region_effect_matches_the_whole_image_effect() {
        let base = noise(96, 72);
        let effects = [
            ImageEffect::Blur { radius: 9.0 },
            ImageEffect::Blur { radius: 30.0 },
            ImageEffect::Pixelate { block_size: 8.0 },
            ImageEffect::Pixelate { block_size: 30.0 },
        ];
        let rects = [
            // Middle of the image, on and off the pixelate grid.
            Rect::from_xywh(24.0, 20.0, 40.0, 32.0),
            Rect::from_xywh(21.0, 17.0, 37.0, 29.0),
            // Touching each edge, and the whole image.
            Rect::from_xywh(0.0, 0.0, 30.0, 30.0),
            Rect::from_xywh(66.0, 42.0, 30.0, 30.0),
            Rect::from_xywh(0.0, 30.0, 96.0, 10.0),
            Rect::from_xywh(0.0, 0.0, 96.0, 72.0),
            // Sub-pixel geometry, which clips outwards to whole pixels.
            Rect::from_xywh(10.5, 12.25, 20.75, 15.5),
        ];
        for effect in effects {
            for rect in rects {
                assert_region_matches_whole_image(&base, rect, effect);
            }
        }
    }

    #[test]
    fn region_invariance_survives_a_long_row_and_a_huge_radius() {
        // The guard on the arithmetic rather than on the geometry, and the
        // reason it needs both a wide image and a big radius: a float window sum
        // stops being exact once the window can hold more than 2^24, i.e. past a
        // box radius of about 128. Below that a float implementation passes
        // every other test in this module, then quietly disagrees with itself by
        // a level once someone raises `annotation_size_factor`.
        let base = noise(4096, 6);
        for rect in [
            Rect::from_xywh(2000.0, 1.0, 60.0, 4.0),
            Rect::from_xywh(4030.0, 0.0, 66.0, 6.0),
        ] {
            for radius in [12.0, 1200.0] {
                assert_region_matches_whole_image(&base, rect, ImageEffect::Blur { radius });
            }
        }
    }

    #[test]
    fn effects_read_the_base_not_the_working_canvas() {
        let base = checkerboard(40, 40);
        let mut canvas = Canvas::filled(40, 40, Color::red());
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
            ImageEffect::Pixelate { block_size: 10.0 },
        );
        let p = canvas.pixel(12, 12);
        assert!(
            p.r == p.g && p.g == p.b,
            "should be grey from the base, not red: {p}"
        );
    }

    #[test]
    fn invisible_empty_and_offscreen_effects_are_no_ops() {
        let base = checkerboard(20, 20);
        let cases = [
            (
                Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                ImageEffect::Blur { radius: 0.1 },
            ),
            (
                Rect::from_xywh(0.0, 0.0, 0.0, 0.0),
                ImageEffect::Blur { radius: 10.0 },
            ),
            (
                Rect::from_xywh(-500.0, -500.0, 100.0, 100.0),
                ImageEffect::Blur { radius: 10.0 },
            ),
            (
                Rect::from_xywh(f32::NAN, 0.0, 10.0, 10.0),
                ImageEffect::Blur { radius: 10.0 },
            ),
            (
                Rect::from_xywh(0.0, 0.0, f32::INFINITY, 10.0),
                ImageEffect::Pixelate { block_size: 4.0 },
            ),
            (
                Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                ImageEffect::Blur { radius: f32::NAN },
            ),
            (
                Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                ImageEffect::Pixelate {
                    block_size: f32::INFINITY,
                },
            ),
        ];
        for (rect, effect) in cases {
            let mut canvas = base.clone();
            apply_effect_in_region(&mut canvas, &base, rect, effect);
            assert_eq!(canvas, base, "{rect:?} {effect:?} should be a no-op");
        }
    }

    #[test]
    fn an_invisible_whole_image_effect_returns_the_base() {
        let base = checkerboard(20, 20);
        assert_eq!(apply_effect(&base, ImageEffect::Blur { radius: 0.1 }), base);
        assert_eq!(
            apply_effect(&base, ImageEffect::Pixelate { block_size: 1.0 }),
            base
        );
        assert_ne!(apply_effect(&base, ImageEffect::Blur { radius: 6.0 }), base);
    }

    #[test]
    fn a_whole_image_effect_on_an_empty_canvas_is_empty() {
        let base = Canvas::new(0, 0);
        let out = apply_effect(&base, ImageEffect::Blur { radius: 10.0 });
        assert!(out.is_empty());
    }

    #[test]
    fn effects_clip_to_the_smaller_of_canvas_and_base() {
        let base = checkerboard(10, 10);
        let mut canvas = Canvas::filled(40, 40, Color::red());
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(0.0, 0.0, 40.0, 40.0),
            ImageEffect::Pixelate { block_size: 5.0 },
        );
        assert_ne!(canvas.pixel(2, 2), Color::red(), "inside the base");
        assert_eq!(canvas.pixel(20, 20), Color::red(), "beyond the base");
    }

    #[test]
    fn a_smaller_working_canvas_does_not_change_the_pixels_written() {
        // Sampling uses the whole base, so cropping the destination must only
        // remove pixels, never alter the ones that remain.
        let base = noise(64, 64);
        let effects = [
            ImageEffect::Blur { radius: 12.0 },
            ImageEffect::Pixelate { block_size: 10.0 },
        ];
        for effect in effects {
            let full = apply_effect(&base, effect);
            let mut small = Canvas::filled(32, 32, Color::red());
            apply_effect_in_region(&mut small, &base, base.bounds(), effect);
            for y in 0..32 {
                for x in 0..32 {
                    assert_eq!(small.pixel(x, y), full.pixel(x, y), "({x},{y}) {effect:?}");
                }
            }
        }
    }

    #[test]
    fn a_huge_radius_is_clamped_rather_than_exploding() {
        let base = checkerboard(16, 16);
        let mut canvas = base.clone();
        apply_effect_in_region(
            &mut canvas,
            &base,
            Rect::from_xywh(0.0, 0.0, 16.0, 16.0),
            ImageEffect::Blur { radius: 1.0e12 },
        );
        assert_ne!(canvas, base);
    }

    #[test]
    fn a_flat_image_blurs_to_itself_exactly() {
        // The fixed-point passes must not drift: every window sum over a
        // constant image divides exactly, so no channel may move at all.
        let base = Canvas::filled(40, 40, Color::new(37, 128, 201, 255));
        for radius in [2.0, 9.0, 30.0, 90.0] {
            assert_eq!(
                apply_effect(&base, ImageEffect::Blur { radius }),
                base,
                "radius {radius}"
            );
        }
    }
}
