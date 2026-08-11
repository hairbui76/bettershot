//! Renderer-agnostic path geometry and triangulation.
//!
//! Curves are flattened and polygons triangulated **here**, in image-pixel
//! space, so that a backend only ever has to draw two things: a triangle mesh
//! and a polyline. That keeps all annotation geometry unit-testable without a
//! GPU, and it avoids relying on any particular renderer's handling of concave
//! fills (the "fat" arrow is concave, which several immediate-mode tessellators
//! get wrong).

use crate::math::{Angle, Vec2D};
use crate::style::Color;

/// How the ends and corners of a stroked polyline are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub color: Color,
    pub cap: LineCap,
}

impl Stroke {
    pub fn new(width: f32, color: Color) -> Self {
        Self {
            width,
            color,
            cap: LineCap::default(),
        }
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    pub fn is_visible(&self) -> bool {
        self.width > 0.0 && self.color.a > 0
    }
}

/// A connected run of points. Curves are already flattened.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubPath {
    pub points: Vec<Vec2D>,
    pub closed: bool,
}

impl SubPath {
    pub fn len(&self) -> usize {
        self.points.len()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// One or more subpaths in image-pixel space.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub subpaths: Vec<SubPath>,
}

/// A triangulated fill: `indices` are triples into `vertices`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub vertices: Vec<Vec2D>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Number of line segments used to flatten a full ellipse or circle. 64 keeps
/// a 4K-wide ellipse visually smooth without inflating meshes.
const ELLIPSE_SEGMENTS: usize = 64;
/// Segments per flattened quadratic/cubic curve.
const CURVE_SEGMENTS: usize = 16;

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.subpaths.iter().all(|s| s.points.len() < 2)
    }

    pub fn move_to(&mut self, p: Vec2D) -> &mut Self {
        self.subpaths.push(SubPath {
            points: vec![p],
            closed: false,
        });
        self
    }

    pub fn line_to(&mut self, p: Vec2D) -> &mut Self {
        match self.subpaths.last_mut() {
            Some(sub) => sub.points.push(p),
            // A `line_to` with no current point starts one, matching the
            // forgiving behaviour of most 2D canvas APIs.
            None => {
                self.move_to(p);
            }
        }
        self
    }

    pub fn close(&mut self) -> &mut Self {
        if let Some(sub) = self.subpaths.last_mut() {
            sub.closed = true;
        }
        self
    }

    pub fn quad_to(&mut self, ctrl: Vec2D, to: Vec2D) -> &mut Self {
        let Some(from) = self.current_point() else {
            return self.move_to(to);
        };
        for i in 1..=CURVE_SEGMENTS {
            let t = i as f32 / CURVE_SEGMENTS as f32;
            let inv = 1.0 - t;
            let p = from * (inv * inv) + ctrl * (2.0 * inv * t) + to * (t * t);
            self.line_to(p);
        }
        self
    }

    pub fn cubic_to(&mut self, c1: Vec2D, c2: Vec2D, to: Vec2D) -> &mut Self {
        let Some(from) = self.current_point() else {
            return self.move_to(to);
        };
        for i in 1..=CURVE_SEGMENTS {
            let t = i as f32 / CURVE_SEGMENTS as f32;
            let inv = 1.0 - t;
            let p = from * (inv * inv * inv)
                + c1 * (3.0 * inv * inv * t)
                + c2 * (3.0 * inv * t * t)
                + to * (t * t * t);
            self.line_to(p);
        }
        self
    }

    pub fn current_point(&self) -> Option<Vec2D> {
        self.subpaths.last().and_then(|s| s.points.last()).copied()
    }

    /// Append a closed polygon.
    ///
    /// Consecutive duplicate points are dropped: shapes assembled from
    /// several arcs (the rounded rectangle) repeat a point at every junction,
    /// and zero-length edges make the ear-clipper fall back to its
    /// force-progress path, which silently loses area.
    pub fn add_polygon(&mut self, points: &[Vec2D]) -> &mut Self {
        let points = dedup_consecutive(points, true);
        if points.is_empty() {
            return self;
        }
        self.subpaths.push(SubPath {
            points,
            closed: true,
        });
        self
    }

    /// Append an open polyline.
    pub fn add_polyline(&mut self, points: &[Vec2D]) -> &mut Self {
        if points.is_empty() {
            return self;
        }
        self.subpaths.push(SubPath {
            points: points.to_vec(),
            closed: false,
        });
        self
    }

    pub fn add_rect(&mut self, rect: crate::math::Rect) -> &mut Self {
        self.add_polygon(&rect.normalized().corners())
    }

    /// Append an axis-aligned ellipse inscribed in `rect`.
    pub fn add_ellipse(&mut self, rect: crate::math::Rect) -> &mut Self {
        let r = rect.normalized();
        let center = r.center();
        let radii = r.size * 0.5;
        let points: Vec<Vec2D> = (0..ELLIPSE_SEGMENTS)
            .map(|i| {
                let t = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                Vec2D::new(center.x + radii.x * t.cos(), center.y + radii.y * t.sin())
            })
            .collect();
        self.add_polygon(&points)
    }

    pub fn add_circle(&mut self, center: Vec2D, radius: f32) -> &mut Self {
        self.add_ellipse(crate::math::Rect::new(
            center - Vec2D::splat(radius),
            Vec2D::splat(radius * 2.0),
        ))
    }

    /// A rectangle with rounded corners.
    pub fn add_round_rect(&mut self, rect: crate::math::Rect, radius: f32) -> &mut Self {
        let r = rect.normalized();
        let radius = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
        if radius <= 0.0 {
            return self.add_rect(r);
        }
        let mut points = Vec::with_capacity(4 * (CURVE_SEGMENTS + 1));
        // Corner centers, clockwise from top-left, with the sweep start angle
        // for each (screen coordinates: y grows downward).
        let corners = [
            (
                Vec2D::new(r.left() + radius, r.top() + radius),
                std::f32::consts::PI,
            ),
            (
                Vec2D::new(r.right() - radius, r.top() + radius),
                std::f32::consts::PI * 1.5,
            ),
            (Vec2D::new(r.right() - radius, r.bottom() - radius), 0.0),
            (
                Vec2D::new(r.left() + radius, r.bottom() - radius),
                std::f32::consts::FRAC_PI_2,
            ),
        ];
        for (center, start) in corners {
            for i in 0..=CURVE_SEGMENTS {
                let t = start + (i as f32 / CURVE_SEGMENTS as f32) * std::f32::consts::FRAC_PI_2;
                points.push(Vec2D::new(
                    center.x + radius * t.cos(),
                    center.y + radius * t.sin(),
                ));
            }
        }
        self.add_polygon(&points)
    }

    /// Triangulate every closed subpath into a single mesh.
    pub fn fill_mesh(&self) -> Mesh {
        let mut mesh = Mesh::default();
        for sub in &self.subpaths {
            if sub.points.len() < 3 {
                continue;
            }
            let base = mesh.vertices.len() as u32;
            let indices = triangulate(&sub.points);
            mesh.vertices.extend_from_slice(&sub.points);
            mesh.indices.extend(indices.into_iter().map(|i| i + base));
        }
        mesh
    }

    /// Bounding box of every point in the path, if it has any.
    pub fn bounds(&self) -> Option<crate::math::Rect> {
        let mut iter = self.subpaths.iter().flat_map(|s| s.points.iter());
        let first = *iter.next()?;
        let (mut min, mut max) = (first, first);
        for p in iter {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        Some(crate::math::Rect::new(min, max - min))
    }

    /// Translate every point.
    pub fn translated(&self, delta: Vec2D) -> Path {
        Path {
            subpaths: self
                .subpaths
                .iter()
                .map(|s| SubPath {
                    points: s.points.iter().map(|p| *p + delta).collect(),
                    closed: s.closed,
                })
                .collect(),
        }
    }

    /// Rotate every point around `origin`.
    pub fn rotated_around(&self, origin: Vec2D, angle: Angle) -> Path {
        Path {
            subpaths: self
                .subpaths
                .iter()
                .map(|s| SubPath {
                    points: s
                        .points
                        .iter()
                        .map(|p| origin + (*p - origin).rotated(angle))
                        .collect(),
                    closed: s.closed,
                })
                .collect(),
        }
    }
}

/// Points closer together than this are treated as the same point.
const POINT_EPSILON: f32 = 1e-4;

/// Drop consecutive duplicate points. With `wrap`, also drops a final point
/// that coincides with the first (an explicitly closed ring).
fn dedup_consecutive(points: &[Vec2D], wrap: bool) -> Vec<Vec2D> {
    let mut out: Vec<Vec2D> = Vec::with_capacity(points.len());
    for &p in points {
        if out
            .last()
            .is_some_and(|last| last.distance_to(&p) < POINT_EPSILON)
        {
            continue;
        }
        out.push(p);
    }
    if wrap && out.len() > 1 && out[0].distance_to(&out[out.len() - 1]) < POINT_EPSILON {
        out.pop();
    }
    out
}

/// Twice the signed area of a polygon. Positive means counter-clockwise in a
/// y-down coordinate system.
fn signed_area2(points: &[Vec2D]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let a = points[i];
            let b = points[(i + 1) % n];
            a.x * b.y - b.x * a.y
        })
        .sum()
}

fn triangle_area2(a: Vec2D, b: Vec2D, c: Vec2D) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
}

fn point_in_triangle(p: Vec2D, a: Vec2D, b: Vec2D, c: Vec2D) -> bool {
    let d1 = triangle_area2(p, a, b);
    let d2 = triangle_area2(p, b, c);
    let d3 = triangle_area2(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Ear-clipping triangulation of a simple polygon. Handles concave shapes
/// (the filled arrow) and either winding order. Degenerate input degrades to a
/// triangle fan rather than looping forever.
pub fn triangulate(points: &[Vec2D]) -> Vec<u32> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![0, 1, 2];
    }

    // Work on an index list wound counter-clockwise.
    let mut remaining: Vec<usize> = (0..n).collect();
    if signed_area2(points) < 0.0 {
        remaining.reverse();
    }

    let mut indices = Vec::with_capacity((n - 2) * 3);
    let mut guard = 0;
    let max_iterations = n * n;

    while remaining.len() > 3 {
        guard += 1;
        if guard > max_iterations {
            // Self-intersecting or otherwise degenerate: fan the remainder so
            // the caller still gets a drawable (if imperfect) mesh.
            for i in 1..remaining.len() - 1 {
                indices.extend_from_slice(&[
                    remaining[0] as u32,
                    remaining[i] as u32,
                    remaining[i + 1] as u32,
                ]);
            }
            return indices;
        }

        let count = remaining.len();
        let mut clipped = false;
        for i in 0..count {
            let prev = remaining[(i + count - 1) % count];
            let curr = remaining[i];
            let next = remaining[(i + 1) % count];
            let (a, b, c) = (points[prev], points[curr], points[next]);

            // Convex in CCW winding?
            if triangle_area2(a, b, c) <= 0.0 {
                continue;
            }
            // No other vertex inside the candidate ear?
            let contains_other = remaining
                .iter()
                .filter(|&&idx| idx != prev && idx != curr && idx != next)
                .any(|&idx| point_in_triangle(points[idx], a, b, c));
            if contains_other {
                continue;
            }

            indices.extend_from_slice(&[prev as u32, curr as u32, next as u32]);
            remaining.remove(i);
            clipped = true;
            break;
        }

        if !clipped {
            // No ear found: force progress to guarantee termination.
            remaining.remove(0);
        }
    }

    indices.extend_from_slice(&[
        remaining[0] as u32,
        remaining[1] as u32,
        remaining[2] as u32,
    ]);
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Rect;

    /// Sum of triangle areas, used to prove a triangulation covers exactly the
    /// polygon it came from.
    fn mesh_area(mesh: &Mesh) -> f32 {
        mesh.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    mesh.vertices[t[0] as usize],
                    mesh.vertices[t[1] as usize],
                    mesh.vertices[t[2] as usize],
                );
                triangle_area2(a, b, c).abs() / 2.0
            })
            .sum()
    }

    fn polygon_area(points: &[Vec2D]) -> f32 {
        signed_area2(points).abs() / 2.0
    }

    #[test]
    fn triangulates_a_square() {
        let square = [
            Vec2D::new(0.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(10.0, 10.0),
            Vec2D::new(0.0, 10.0),
        ];
        let indices = triangulate(&square);
        assert_eq!(indices.len(), 6, "a quad is two triangles");
        let mesh = Mesh {
            vertices: square.to_vec(),
            indices,
        };
        assert!((mesh_area(&mesh) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn triangulates_a_concave_polygon_without_gaps_or_overlap() {
        // An arrow-like concave shape: total area must be preserved exactly.
        let concave = [
            Vec2D::new(0.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(10.0, 10.0),
            Vec2D::new(5.0, 4.0), // the notch
            Vec2D::new(0.0, 10.0),
        ];
        let indices = triangulate(&concave);
        assert_eq!(indices.len(), (concave.len() - 2) * 3);
        let mesh = Mesh {
            vertices: concave.to_vec(),
            indices,
        };
        assert!(
            (mesh_area(&mesh) - polygon_area(&concave)).abs() < 1e-2,
            "mesh {} vs polygon {}",
            mesh_area(&mesh),
            polygon_area(&concave)
        );
    }

    #[test]
    fn winding_order_does_not_matter() {
        let ccw = [
            Vec2D::new(0.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(10.0, 10.0),
            Vec2D::new(0.0, 10.0),
        ];
        let mut cw = ccw.to_vec();
        cw.reverse();
        let a = Mesh {
            vertices: ccw.to_vec(),
            indices: triangulate(&ccw),
        };
        let b = Mesh {
            vertices: cw.clone(),
            indices: triangulate(&cw),
        };
        assert!((mesh_area(&a) - mesh_area(&b)).abs() < 1e-3);
    }

    #[test]
    fn degenerate_input_is_handled() {
        assert!(triangulate(&[]).is_empty());
        assert!(triangulate(&[Vec2D::ZERO]).is_empty());
        assert!(triangulate(&[Vec2D::ZERO, Vec2D::new(1.0, 1.0)]).is_empty());
        // Collinear points must terminate and not panic.
        let collinear = [
            Vec2D::new(0.0, 0.0),
            Vec2D::new(1.0, 0.0),
            Vec2D::new(2.0, 0.0),
            Vec2D::new(3.0, 0.0),
        ];
        let _ = triangulate(&collinear);
    }

    #[test]
    fn ellipse_area_approximates_pi_r_squared() {
        let mut path = Path::new();
        path.add_ellipse(Rect::from_xywh(0.0, 0.0, 20.0, 20.0));
        let mesh = path.fill_mesh();
        let expected = std::f32::consts::PI * 100.0;
        // A 64-gon slightly under-covers the circle; 1% is plenty.
        assert!(
            (mesh_area(&mesh) - expected).abs() / expected < 0.01,
            "got {}",
            mesh_area(&mesh)
        );
    }

    #[test]
    fn rect_path_bounds_match_the_rect() {
        let rect = Rect::from_xywh(5.0, 6.0, 30.0, 40.0);
        let mut path = Path::new();
        path.add_rect(rect);
        assert_eq!(path.bounds().unwrap(), rect);
        assert_eq!(path.fill_mesh().triangle_count(), 2);
    }

    #[test]
    fn negative_size_rect_still_fills() {
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(10.0, 10.0, -10.0, -10.0));
        let mesh = path.fill_mesh();
        assert!((mesh_area(&mesh) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn multiple_subpaths_are_merged_with_offset_indices() {
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        path.add_rect(Rect::from_xywh(20.0, 20.0, 10.0, 10.0));
        let mesh = path.fill_mesh();
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.triangle_count(), 4);
        assert!(mesh.indices.iter().all(|&i| (i as usize) < 8));
        assert!((mesh_area(&mesh) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn open_subpaths_are_not_filled() {
        let mut path = Path::new();
        path.add_polyline(&[Vec2D::ZERO, Vec2D::new(10.0, 0.0)]);
        assert!(path.fill_mesh().is_empty());
    }

    #[test]
    fn transforms_preserve_shape() {
        let mut path = Path::new();
        path.add_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        let moved = path.translated(Vec2D::new(5.0, 5.0));
        assert_eq!(
            moved.bounds().unwrap(),
            Rect::from_xywh(5.0, 5.0, 10.0, 10.0)
        );

        let rotated = path.rotated_around(Vec2D::ZERO, Angle::from_degrees(90.0));
        let area_before = mesh_area(&path.fill_mesh());
        let area_after = mesh_area(&rotated.fill_mesh());
        assert!((area_before - area_after).abs() < 1e-2);
    }

    #[test]
    fn line_to_without_move_to_starts_a_subpath() {
        let mut path = Path::new();
        path.line_to(Vec2D::new(1.0, 1.0));
        assert_eq!(path.subpaths.len(), 1);
        assert_eq!(path.current_point(), Some(Vec2D::new(1.0, 1.0)));
    }

    #[test]
    fn round_rect_stays_within_bounds() {
        let rect = Rect::from_xywh(0.0, 0.0, 100.0, 50.0);
        let mut path = Path::new();
        path.add_round_rect(rect, 10.0);
        let bounds = path.bounds().unwrap();
        assert!(bounds.left() >= rect.left() - 1e-3);
        assert!(bounds.right() <= rect.right() + 1e-3);
        assert!(bounds.top() >= rect.top() - 1e-3);
        assert!(bounds.bottom() <= rect.bottom() + 1e-3);
        // A rounded rect has less area than the full rect but most of it.
        let area = mesh_area(&path.fill_mesh());
        assert!(area < 5000.0 && area > 4700.0, "got {area}");
    }

    #[test]
    fn duplicate_points_do_not_eat_into_the_filled_area() {
        // A square with every vertex repeated must still fill 100 units.
        let square = [
            Vec2D::new(0.0, 0.0),
            Vec2D::new(0.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(10.0, 0.0),
            Vec2D::new(10.0, 10.0),
            Vec2D::new(0.0, 10.0),
            // Explicitly closed: the last point repeats the first.
            Vec2D::new(0.0, 0.0),
        ];
        let mut path = Path::new();
        path.add_polygon(&square);
        assert_eq!(path.subpaths[0].points.len(), 4, "duplicates dropped");
        assert!((mesh_area(&path.fill_mesh()) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn round_rect_radius_is_clamped_to_half_the_shortest_side() {
        let mut path = Path::new();
        path.add_round_rect(Rect::from_xywh(0.0, 0.0, 20.0, 20.0), 1000.0);
        // Degenerates to a circle of radius 10, not a broken shape.
        let area = mesh_area(&path.fill_mesh());
        let circle = std::f32::consts::PI * 100.0;
        assert!((area - circle).abs() / circle < 0.02, "got {area}");
    }
}
