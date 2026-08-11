//! Triangle and polyline rasterization with anti-aliasing.
//!
//! # The anti-aliasing scheme
//!
//! Every triangle is rasterized through three affine *edge functions*. For a
//! pixel we evaluate each edge at the pixel centre and compare it against
//! `0.5 * (|A| + |B|)`, the largest amount the edge function can change between
//! the centre and any point in the pixel. That single comparison classifies the
//! pixel as fully outside, fully inside, or *straddling* an edge. Only the
//! straddling pixels — a thin outline, never the interior — pay for a 4x4
//! supersample. So the cost is ~3 multiply-adds per interior pixel and 48 per
//! edge pixel, which keeps a full-screen fill in the low milliseconds while
//! still producing 16 distinct coverage levels along a slanted edge.
//!
//! # Why a coverage mask instead of blending each triangle
//!
//! A filled rectangle is two triangles that share a diagonal. If each triangle
//! were composited separately, the shared edge would be blended twice at ~50%
//! coverage each and a visible seam would run across the shape. The same
//! applies to a stroked polyline, where every round join overlaps the segments
//! it connects.
//!
//! So a whole `fill_mesh` or `stroke_path` call first accumulates coverage into
//! an f32 [`Mask`] (clamped to 1.0), and the mask is composited onto the canvas
//! exactly once. Union-by-clamped-sum slightly overestimates coverage where two
//! anti-aliased edges overlap inside one pixel, which is the standard trade and
//! is invisible next to the seams it removes.

use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::path::{LineCap, Mesh, Path as CorePath, Stroke};
use bettershot_core::style::Color;

use crate::canvas::Canvas;

/// Sub-samples per axis inside a pixel an edge passes through.
const SUBSAMPLES: usize = 4;
const SUBSAMPLE_TOTAL: f32 = (SUBSAMPLES * SUBSAMPLES) as f32;

/// Triangles with less than this much (doubled) area contribute nothing and
/// have numerically useless edge functions.
const MIN_TRIANGLE_AREA2: f32 = 1e-7;

/// Segments below this length cannot be given a meaningful direction.
const MIN_SEGMENT_LENGTH: f32 = 1e-5;

#[inline]
fn finite(p: Vec2D) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// A half-open integer pixel rectangle, always already clipped to a canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelBox {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelBox {
    pub(crate) fn width(&self) -> usize {
        (self.x1 - self.x0).max(0) as usize
    }

    pub(crate) fn height(&self) -> usize {
        (self.y1 - self.y0).max(0) as usize
    }
}

/// Clip a float rect to whole pixels inside a `w`x`h` canvas.
///
/// Returns `None` for non-finite input, which is how "skip this operation"
/// propagates out of every entry point in this module. `f32 as i32` saturates
/// in Rust, so even `1e30` lands harmlessly on the canvas edge.
pub(crate) fn clip_to_pixels(rect: Rect, w: u32, h: u32) -> Option<PixelBox> {
    let r = rect.normalized();
    if !finite(r.pos) || !finite(r.size) {
        return None;
    }
    let x0 = (r.left().floor() as i32).max(0);
    let y0 = (r.top().floor() as i32).max(0);
    let x1 = (r.right().ceil() as i32).min(w as i32);
    let y1 = (r.bottom().ceil() as i32).min(h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(PixelBox { x0, y0, x1, y1 })
}

/// `E(x, y) = a*x + b*y + c`, positive on the left-hand side of `p -> q`
/// (in a y-down coordinate system, that is the inside of a clockwise polygon).
#[derive(Debug, Clone, Copy)]
struct Edge {
    a: f32,
    b: f32,
    c: f32,
    /// Largest possible |E(sample) - E(centre)| within one pixel.
    radius: f32,
}

impl Edge {
    fn new(p: Vec2D, q: Vec2D) -> Self {
        let a = p.y - q.y;
        let b = q.x - p.x;
        let c = -(a * p.x + b * p.y);
        Self {
            a,
            b,
            c,
            radius: 0.5 * (a.abs() + b.abs()),
        }
    }

    #[inline]
    fn eval(&self, x: f32, y: f32) -> f32 {
        self.a * x + self.b * y + self.c
    }
}

#[inline]
fn cross(o: Vec2D, a: Vec2D, b: Vec2D) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Per-pixel coverage accumulator over a fixed pixel box.
pub(crate) struct Mask {
    bounds: PixelBox,
    stride: usize,
    data: Vec<f32>,
}

impl Mask {
    pub(crate) fn new(bounds: PixelBox) -> Self {
        let stride = bounds.width();
        Self {
            data: vec![0.0; stride * bounds.height()],
            stride,
            bounds,
        }
    }

    /// Accumulate one triangle. Winding order is irrelevant: it is normalized
    /// here so callers (and core's ear clipper) never have to care.
    pub(crate) fn add_triangle(&mut self, a: Vec2D, b: Vec2D, c: Vec2D) {
        if self.data.is_empty() || !finite(a) || !finite(b) || !finite(c) {
            return;
        }
        let area2 = cross(a, b, c);
        if !area2.is_finite() || area2.abs() < MIN_TRIANGLE_AREA2 {
            return;
        }
        let (b, c) = if area2 < 0.0 { (c, b) } else { (b, c) };

        let x0 = ((a.x.min(b.x).min(c.x)).floor() as i32).clamp(self.bounds.x0, self.bounds.x1);
        let x1 = ((a.x.max(b.x).max(c.x)).ceil() as i32).clamp(self.bounds.x0, self.bounds.x1);
        let y0 = ((a.y.min(b.y).min(c.y)).floor() as i32).clamp(self.bounds.y0, self.bounds.y1);
        let y1 = ((a.y.max(b.y).max(c.y)).ceil() as i32).clamp(self.bounds.y0, self.bounds.y1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let (e0, e1, e2) = (Edge::new(a, b), Edge::new(b, c), Edge::new(c, a));

        for py in y0..y1 {
            let cy = py as f32 + 0.5;
            let row = (py - self.bounds.y0) as usize * self.stride;
            let mut idx = row + (x0 - self.bounds.x0) as usize;

            for px in x0..x1 {
                // Evaluated directly rather than stepped along the row. Each
                // triangle starts from its own bounding box, so incremental
                // stepping accumulates a different rounding error on each side
                // of a shared edge; where both drift low, the seam pixel is
                // counted by neither and a translucent pinhole appears inside
                // what should be solid — visible along round caps and joins.
                // Direct evaluation depends only on the pixel centre, so two
                // triangles sharing an edge agree exactly.
                let cx = px as f32 + 0.5;
                let (v0, v1, v2) = (e0.eval(cx, cy), e1.eval(cx, cy), e2.eval(cx, cy));
                let cov = coverage(&e0, &e1, &e2, v0, v1, v2);
                if cov > 0.0 {
                    let slot = &mut self.data[idx];
                    *slot = (*slot + cov).min(1.0);
                }
                idx += 1;
            }
        }
    }

    pub(crate) fn add_quad(&mut self, a: Vec2D, b: Vec2D, c: Vec2D, d: Vec2D) {
        self.add_triangle(a, b, c);
        self.add_triangle(a, c, d);
    }

    /// A filled circle, tessellated as a fan. Used for round caps and joins.
    pub(crate) fn add_disc(&mut self, center: Vec2D, radius: f32) {
        if !finite(center) || !radius.is_finite() || radius <= 0.0 {
            return;
        }
        // Enough segments that the facets stay under about half a pixel.
        let segments = ((radius * 2.0).ceil() as usize).clamp(8, 64);
        let step = std::f32::consts::TAU / segments as f32;
        let mut prev = Vec2D::new(center.x + radius, center.y);
        for i in 1..=segments {
            let t = i as f32 * step;
            let next = Vec2D::new(center.x + radius * t.cos(), center.y + radius * t.sin());
            self.add_triangle(center, prev, next);
            prev = next;
        }
    }

    /// Composite the accumulated coverage onto `canvas` in one pass.
    pub(crate) fn composite(&self, canvas: &mut Canvas, color: Color) {
        if color.a == 0 || self.data.is_empty() {
            return;
        }
        for (row, chunk) in self.data.chunks_exact(self.stride).enumerate() {
            let py = self.bounds.y0 + row as i32;
            for (col, &cov) in chunk.iter().enumerate() {
                if cov > 0.0 {
                    canvas.blend(self.bounds.x0 + col as i32, py, color, cov);
                }
            }
        }
    }

    #[cfg(test)]
    fn coverage_at(&self, x: i32, y: i32) -> f32 {
        if x < self.bounds.x0 || x >= self.bounds.x1 || y < self.bounds.y0 || y >= self.bounds.y1 {
            return 0.0;
        }
        self.data[(y - self.bounds.y0) as usize * self.stride + (x - self.bounds.x0) as usize]
    }
}

/// Coverage of one pixel, given the three edge functions evaluated at its
/// centre. See the module docs for why this shortcut is sound.
#[inline]
fn coverage(e0: &Edge, e1: &Edge, e2: &Edge, v0: f32, v1: f32, v2: f32) -> f32 {
    if v0 < -e0.radius || v1 < -e1.radius || v2 < -e2.radius {
        return 0.0;
    }
    if v0 > e0.radius && v1 > e1.radius && v2 > e2.radius {
        return 1.0;
    }

    let mut hits = 0u32;
    for sy in 0..SUBSAMPLES {
        let dy = (sy as f32 + 0.5) / SUBSAMPLES as f32 - 0.5;
        for sx in 0..SUBSAMPLES {
            let dx = (sx as f32 + 0.5) / SUBSAMPLES as f32 - 0.5;
            if v0 + e0.a * dx + e0.b * dy >= 0.0
                && v1 + e1.a * dx + e1.b * dy >= 0.0
                && v2 + e2.a * dx + e2.b * dy >= 0.0
            {
                hits += 1;
            }
        }
    }
    hits as f32 / SUBSAMPLE_TOTAL
}

/// Bounding box of the vertices a mesh actually references, or `None` if any of
/// them is non-finite.
fn mesh_bounds(mesh: &Mesh) -> Option<Rect> {
    let mut min = Vec2D::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2D::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut any = false;
    for tri in mesh.indices.chunks_exact(3) {
        for &i in tri {
            let p = *mesh.vertices.get(i as usize)?;
            if !finite(p) {
                return None;
            }
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            any = true;
        }
    }
    any.then(|| Rect::new(min, max - min))
}

/// Rasterize and composite a triangle mesh. Silently does nothing for empty,
/// off-canvas, degenerate or non-finite input.
pub(crate) fn fill_mesh(canvas: &mut Canvas, mesh: &Mesh, color: Color) {
    if mesh.is_empty() || color.a == 0 || canvas.is_empty() {
        return;
    }
    let Some(bounds) = mesh_bounds(mesh) else {
        log::debug!("skipping a mesh with non-finite or out-of-range vertices");
        return;
    };
    let Some(pixels) = clip_to_pixels(bounds, canvas.width(), canvas.height()) else {
        return;
    };

    let mut mask = Mask::new(pixels);
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (
            mesh.vertices[tri[0] as usize],
            mesh.vertices[tri[1] as usize],
            mesh.vertices[tri[2] as usize],
        );
        mask.add_triangle(a, b, c);
    }
    mask.composite(canvas, color);
}

/// Exact-coverage fill of an axis-aligned rectangle.
///
/// Worth having separately from the mesh path: text backgrounds, the caret and
/// the fallback glyph blocks are all axis-aligned, and the analytic
/// pixel-overlap area is both cheaper and sharper than supersampling.
pub(crate) fn fill_axis_rect(canvas: &mut Canvas, rect: Rect, color: Color) {
    if color.a == 0 || canvas.is_empty() {
        return;
    }
    let r = rect.normalized();
    let Some(pixels) = clip_to_pixels(r, canvas.width(), canvas.height()) else {
        return;
    };
    for py in pixels.y0..pixels.y1 {
        let top = py as f32;
        let cy = (r.bottom().min(top + 1.0) - r.top().max(top)).clamp(0.0, 1.0);
        if cy <= 0.0 {
            continue;
        }
        for px in pixels.x0..pixels.x1 {
            let left = px as f32;
            let cx = (r.right().min(left + 1.0) - r.left().max(left)).clamp(0.0, 1.0);
            if cx > 0.0 {
                canvas.blend(px, py, color, cx * cy);
            }
        }
    }
}

/// Expand every subpath of `path` into filled geometry and composite it once.
pub(crate) fn stroke_path(canvas: &mut Canvas, path: &CorePath, stroke: Stroke) {
    if !stroke.is_visible() || !stroke.width.is_finite() || canvas.is_empty() {
        return;
    }
    let half = stroke.width / 2.0;

    let Some(bounds) = path.bounds() else { return };
    if !finite(bounds.pos) || !finite(bounds.size) {
        log::debug!("skipping a stroke with non-finite points");
        return;
    }
    // One extra pixel of slack for the anti-aliased fringe.
    let Some(pixels) = clip_to_pixels(bounds.expanded(half + 1.0), canvas.width(), canvas.height())
    else {
        return;
    };

    let mut mask = Mask::new(pixels);
    for sub in &path.subpaths {
        add_subpath(&mut mask, &sub.points, sub.closed, half, stroke.cap);
    }
    mask.composite(canvas, stroke.color);
}

fn add_subpath(mask: &mut Mask, points: &[Vec2D], closed: bool, half: f32, cap: LineCap) {
    if points.iter().any(|p| !finite(*p)) {
        return;
    }
    let round = cap == LineCap::Round;

    match points.len() {
        0 => return,
        // A single point is only visible with a round cap; a butt-capped
        // zero-length stroke genuinely covers nothing.
        1 => {
            if round {
                mask.add_disc(points[0], half);
            }
            return;
        }
        _ => {}
    }

    // Walk the segments, closing the ring when asked.
    let n = points.len();
    let segment_count = if closed { n } else { n - 1 };
    for i in 0..segment_count {
        let p = points[i];
        let q = points[(i + 1) % n];
        add_segment(mask, p, q, half);
    }

    if round {
        // A disc at every vertex is simultaneously the round join and, at the
        // ends of an open path, the round cap.
        for &p in points {
            mask.add_disc(p, half);
        }
    } else {
        // Butt caps leave a notch on the outside of each corner; fill it with a
        // bevel so a rectangle outline does not have holes.
        let joints = if closed { n } else { n - 1 };
        for i in if closed { 0..joints } else { 1..joints } {
            let prev = points[(i + n - 1) % n];
            let curr = points[i];
            let next = points[(i + 1) % n];
            add_bevel(mask, prev, curr, next, half);
        }
    }
}

fn add_segment(mask: &mut Mask, p: Vec2D, q: Vec2D, half: f32) {
    let Some(n) = normal(p, q, half) else { return };
    mask.add_quad(p + n, q + n, q - n, p - n);
}

fn add_bevel(mask: &mut Mask, prev: Vec2D, curr: Vec2D, next: Vec2D, half: f32) {
    let (Some(n1), Some(n2)) = (normal(prev, curr, half), normal(curr, next, half)) else {
        return;
    };
    // Only one of these two is outside the union; the other is harmlessly
    // inside it, and coverage is clamped anyway.
    mask.add_triangle(curr, curr + n1, curr + n2);
    mask.add_triangle(curr, curr - n1, curr - n2);
}

/// Left-hand normal of `p -> q`, scaled to `half`.
fn normal(p: Vec2D, q: Vec2D, half: f32) -> Option<Vec2D> {
    let d = q - p;
    let len = d.norm();
    if !len.is_finite() || len < MIN_SEGMENT_LENGTH {
        return None;
    }
    Some(Vec2D::new(-d.y, d.x) * (half / len))
}

#[cfg(test)]
mod tests {
    /// A shape built from several triangles must be solid inside, with no
    /// translucent pixels along the seams where triangles meet.
    ///
    /// Round caps and joins are fans of triangles, so this used to show as
    /// pinholes down the middle of a thick stroke.
    #[test]
    fn adjacent_triangles_leave_no_translucent_seam() {
        use crate::Canvas;
        use bettershot_core::math::Vec2D;
        use bettershot_core::painter::Painter;
        use bettershot_core::path::{LineCap, Path, Stroke};
        use bettershot_core::style::Color;

        // Sweep sub-pixel offsets: the drift depended on where the shape fell
        // relative to the pixel grid, so a single placement missed it.
        let mut bad = 0;
        for step in 0..16 {
            let offset = step as f32 / 16.0;
            let base = Canvas::filled(60, 40, Color::white());
            let mut canvas = base.clone();
            {
                let mut painter = crate::CpuPainter::new(&mut canvas, &base);
                let mut path = Path::new();
                path.add_polyline(&[
                    Vec2D::new(12.0 + offset, 20.0 + offset),
                    Vec2D::new(48.0 + offset, 20.0 + offset),
                ]);
                painter.stroke_path(
                    &path,
                    Stroke::new(14.0, Color::black()).with_cap(LineCap::Round),
                );
            }
            // Well inside the stroke, so every sample must be fully covered.
            for x in 16..44 {
                let c = canvas.pixel(x, 20);
                if c != Color::black() {
                    bad += 1;
                }
            }
        }
        assert_eq!(bad, 0, "{bad} interior pixels were not fully covered");
    }

    use super::*;

    fn box_of(x0: i32, y0: i32, x1: i32, y1: i32) -> PixelBox {
        PixelBox { x0, y0, x1, y1 }
    }

    #[test]
    fn clipping_rejects_non_finite_and_offscreen_rects() {
        assert!(clip_to_pixels(Rect::from_xywh(f32::NAN, 0.0, 5.0, 5.0), 10, 10).is_none());
        assert!(clip_to_pixels(Rect::from_xywh(0.0, 0.0, f32::INFINITY, 5.0), 10, 10).is_none());
        assert!(clip_to_pixels(Rect::from_xywh(-100.0, 0.0, 10.0, 5.0), 10, 10).is_none());
        assert!(clip_to_pixels(Rect::from_xywh(0.0, 0.0, 0.0, 0.0), 10, 10).is_none());
    }

    #[test]
    fn clipping_saturates_huge_coordinates_onto_the_canvas() {
        let b = clip_to_pixels(Rect::from_xywh(-1e30, -1e30, 2e30, 2e30), 10, 8).unwrap();
        assert_eq!(b, box_of(0, 0, 10, 8));
    }

    #[test]
    fn a_pixel_aligned_triangle_pair_covers_a_rect_exactly() {
        let mut mask = Mask::new(box_of(0, 0, 8, 8));
        let (a, b) = (Vec2D::new(2.0, 2.0), Vec2D::new(6.0, 2.0));
        let (c, d) = (Vec2D::new(6.0, 6.0), Vec2D::new(2.0, 6.0));
        mask.add_quad(a, b, c, d);

        assert_eq!(mask.coverage_at(3, 3), 1.0, "interior");
        assert_eq!(mask.coverage_at(1, 3), 0.0, "left of the rect");
        assert_eq!(mask.coverage_at(6, 3), 0.0, "right of the rect");
        // The shared diagonal must not leave a partially covered seam.
        assert_eq!(mask.coverage_at(4, 4), 1.0, "seam pixel");
    }

    #[test]
    fn a_diagonal_edge_produces_intermediate_coverage() {
        let mut mask = Mask::new(box_of(0, 0, 20, 20));
        mask.add_triangle(
            Vec2D::new(0.0, 0.0),
            Vec2D::new(20.0, 20.0),
            Vec2D::new(0.0, 20.0),
        );
        let along: Vec<f32> = (2..18).map(|i| mask.coverage_at(i, i)).collect();
        assert!(
            along.iter().all(|c| *c > 0.0 && *c < 1.0),
            "the diagonal should be partially covered: {along:?}"
        );
    }

    #[test]
    fn degenerate_triangles_are_dropped() {
        let mut mask = Mask::new(box_of(0, 0, 8, 8));
        mask.add_triangle(Vec2D::ZERO, Vec2D::ZERO, Vec2D::ZERO);
        mask.add_triangle(
            Vec2D::new(0.0, 0.0),
            Vec2D::new(4.0, 4.0),
            Vec2D::new(8.0, 8.0),
        );
        mask.add_triangle(Vec2D::new(f32::NAN, 0.0), Vec2D::ZERO, Vec2D::new(4.0, 4.0));
        assert!(mask.data.iter().all(|c| *c == 0.0));
    }

    #[test]
    fn a_disc_is_round() {
        let mut mask = Mask::new(box_of(0, 0, 20, 20));
        mask.add_disc(Vec2D::new(10.0, 10.0), 8.0);
        assert_eq!(mask.coverage_at(10, 10), 1.0, "centre");
        assert_eq!(mask.coverage_at(3, 3), 0.0, "corner of the bounding box");
        assert_eq!(mask.coverage_at(10, 3), 1.0, "top of the disc");
    }

    #[test]
    fn fill_axis_rect_gives_fractional_pixels_partial_alpha() {
        let mut canvas = Canvas::new(4, 1);
        fill_axis_rect(
            &mut canvas,
            Rect::from_xywh(0.5, 0.0, 2.0, 1.0),
            Color::new(0, 0, 0, 255),
        );
        assert_eq!(canvas.pixel(0, 0).a, 128, "half-covered left pixel");
        assert_eq!(canvas.pixel(1, 0).a, 255, "fully covered");
        assert_eq!(canvas.pixel(2, 0).a, 128, "half-covered right pixel");
        assert_eq!(canvas.pixel(3, 0).a, 0, "untouched");
    }

    #[test]
    fn stroking_a_horizontal_line_hits_the_requested_width() {
        let mut canvas = Canvas::new(40, 40);
        let mut path = CorePath::new();
        path.add_polyline(&[Vec2D::new(5.0, 20.0), Vec2D::new(35.0, 20.0)]);
        stroke_path(&mut canvas, &path, Stroke::new(6.0, Color::black()));

        let painted = (0..40).filter(|y| canvas.pixel(20, *y).a > 0).count();
        assert_eq!(painted, 6, "a 6px stroke should cover 6 rows");
    }

    #[test]
    fn round_and_butt_caps_differ_past_the_endpoint() {
        let mut path = CorePath::new();
        path.add_polyline(&[Vec2D::new(10.0, 20.0), Vec2D::new(30.0, 20.0)]);

        let mut butt = Canvas::new(40, 40);
        stroke_path(&mut butt, &path, Stroke::new(8.0, Color::black()));
        let mut round = Canvas::new(40, 40);
        stroke_path(
            &mut round,
            &path,
            Stroke::new(8.0, Color::black()).with_cap(LineCap::Round),
        );

        assert_eq!(butt.pixel(7, 20).a, 0, "butt cap stops at the endpoint");
        assert!(round.pixel(7, 20).a > 200, "round cap bulges past it");
        // Inside the shaft the two are identical.
        assert_eq!(butt.pixel(20, 20).a, round.pixel(20, 20).a);
    }

    #[test]
    fn a_single_point_subpath_is_a_dot_only_with_round_caps() {
        let mut path = CorePath::new();
        path.add_polyline(&[Vec2D::new(20.0, 20.0)]);

        let mut round = Canvas::new(40, 40);
        stroke_path(
            &mut round,
            &path,
            Stroke::new(9.0, Color::black()).with_cap(LineCap::Round),
        );
        assert!(round.pixel(20, 20).a > 200);

        let mut butt = Canvas::new(40, 40);
        stroke_path(&mut butt, &path, Stroke::new(9.0, Color::black()));
        assert_eq!(butt.pixel(20, 20).a, 0);
    }

    #[test]
    fn a_closed_subpath_joins_the_last_point_back_to_the_first() {
        let mut canvas = Canvas::new(40, 40);
        let mut path = CorePath::new();
        path.add_rect(Rect::from_xywh(10.0, 10.0, 20.0, 20.0));
        stroke_path(&mut canvas, &path, Stroke::new(4.0, Color::black()));

        // All four sides, including the closing one, must be painted.
        for (x, y) in [(20, 10), (20, 30), (10, 20), (30, 20)] {
            assert!(canvas.pixel(x, y).a > 200, "side at ({x},{y})");
        }
        assert_eq!(canvas.pixel(20, 20).a, 0, "the outline is not filled");
    }

    #[test]
    fn butt_joins_leave_no_hole_at_a_corner() {
        let mut canvas = Canvas::new(40, 40);
        let mut path = CorePath::new();
        path.add_polyline(&[
            Vec2D::new(10.0, 10.0),
            Vec2D::new(30.0, 10.0),
            Vec2D::new(30.0, 30.0),
        ]);
        stroke_path(&mut canvas, &path, Stroke::new(8.0, Color::black()));
        assert!(canvas.pixel(31, 9).a > 200, "outer corner is filled in");
    }

    #[test]
    fn non_finite_geometry_is_skipped_rather_than_drawn() {
        let mut canvas = Canvas::filled(20, 20, Color::white());
        let mut path = CorePath::new();
        path.add_polyline(&[Vec2D::new(f32::NAN, 5.0), Vec2D::new(10.0, 10.0)]);
        stroke_path(&mut canvas, &path, Stroke::new(4.0, Color::black()));

        let mut path = CorePath::new();
        path.add_polyline(&[Vec2D::new(0.0, f32::INFINITY), Vec2D::new(10.0, 10.0)]);
        stroke_path(&mut canvas, &path, Stroke::new(4.0, Color::black()));

        assert!(
            (0..20).all(|y| (0..20).all(|x| canvas.pixel(x, y) == Color::white())),
            "nothing should have been painted"
        );
    }

    #[test]
    fn a_mesh_with_out_of_range_indices_is_ignored() {
        let mut canvas = Canvas::filled(8, 8, Color::white());
        let mesh = Mesh {
            vertices: vec![Vec2D::ZERO, Vec2D::new(8.0, 0.0), Vec2D::new(0.0, 8.0)],
            indices: vec![0, 1, 7],
        };
        fill_mesh(&mut canvas, &mesh, Color::black());
        assert_eq!(canvas.pixel(1, 1), Color::white());
    }
}
