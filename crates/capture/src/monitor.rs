//! Monitors and the virtual desktop they form.
//!
//! # Physical pixels are canonical
//!
//! [`MonitorInfo::bounds`] is always in **physical device pixels** on the
//! virtual desktop, and [`MonitorInfo::scale_factor`] is carried alongside as
//! metadata. This is deliberate:
//!
//! * A capture *is* physical pixels. Storing geometry in anything else means
//!   converting on every grab, and every conversion is a chance to be off by a
//!   rounding error on a 125% or 150% display.
//! * Under mixed DPI the logical layout is **not** a uniform scaling of the
//!   physical layout. Two monitors side by side at 1.0 and 1.5 sit at physical
//!   x = 0 and x = 1920, but at logical x = 0 and x = 1920 as well (Windows
//!   places the second monitor's logical origin where its physical origin is,
//!   then scales only *within* the monitor). There is no single divisor that
//!   turns the virtual-physical plane into the virtual-logical plane, so a
//!   "logical virtual desktop" would be a lie.
//!
//! Consequently the scale conversions here are **monitor-local only**:
//! [`MonitorInfo::local_to_logical`] and [`MonitorInfo::logical_to_local`] take
//! and return coordinates relative to that monitor's own top-left. To go from a
//! virtual-desktop point to logical coordinates, first find the monitor
//! ([`VirtualDesktop::monitor_at`]), then convert within it — which is exactly
//! what [`VirtualDesktop::virtual_to_logical`] does.

use std::fmt;

use bettershot_core::{Rect, Vec2D};

use crate::{
    CaptureError,
    geometry::{bounding_box, clamp_region, contains_half_open},
};

/// Backend-assigned monitor handle.
///
/// Opaque and only meaningful to the backend that produced it, and only until
/// the display configuration changes. Never persist one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonitorId(u64);

impl MonitorId {
    /// Wrap a backend-native handle.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The backend-native handle.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for MonitorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "monitor#{}", self.0)
    }
}

/// One connected display.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Backend handle, valid until the display configuration changes.
    pub id: MonitorId,
    /// Human-readable name, e.g. `DP-1` or `\\.\DISPLAY1`.
    pub name: String,
    /// Position and size on the virtual desktop, in **physical pixels**. May
    /// have a negative origin when the monitor is left of / above the primary.
    pub bounds: Rect,
    /// HiDPI scale of this display (1.0 = 100%, 1.5 = 150%).
    pub scale_factor: f32,
    /// Whether the OS considers this the primary display.
    pub is_primary: bool,
}

impl MonitorInfo {
    /// Build a monitor description. `scale_factor` is clamped to a sane
    /// positive value so downstream divisions can never blow up.
    pub fn new(
        id: MonitorId,
        name: impl Into<String>,
        bounds: Rect,
        scale_factor: f32,
        is_primary: bool,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            bounds: bounds.normalized(),
            scale_factor: sanitize_scale(scale_factor),
            is_primary,
        }
    }

    /// Top-left corner on the virtual desktop, in physical pixels.
    pub fn origin(&self) -> Vec2D {
        self.bounds.pos
    }

    /// Size in physical pixels.
    pub fn physical_size(&self) -> Vec2D {
        self.bounds.size
    }

    /// Size in this monitor's own logical (scaled) pixels — what the OS's
    /// window manager reports for windows on this display.
    pub fn logical_size(&self) -> Vec2D {
        self.physical_size() * (1.0 / self.scale_factor)
    }

    /// Does this monitor own `point` (given on the virtual desktop, physical
    /// pixels)? Half-open, so adjacent monitors never both claim a seam pixel.
    pub fn contains(&self, point: Vec2D) -> bool {
        contains_half_open(self.bounds, point)
    }

    /// Virtual-desktop point -> monitor-local physical point.
    pub fn to_local(&self, virtual_point: Vec2D) -> Vec2D {
        virtual_point - self.origin()
    }

    /// Monitor-local physical point -> virtual-desktop point.
    pub fn to_virtual(&self, local_point: Vec2D) -> Vec2D {
        local_point + self.origin()
    }

    /// Monitor-local physical -> monitor-local logical.
    ///
    /// Only valid within one monitor; see the module docs for why there is no
    /// desktop-wide equivalent.
    pub fn local_to_logical(&self, local_physical: Vec2D) -> Vec2D {
        local_physical * (1.0 / self.scale_factor)
    }

    /// Monitor-local logical -> monitor-local physical.
    pub fn logical_to_local(&self, local_logical: Vec2D) -> Vec2D {
        local_logical * self.scale_factor
    }

    /// A monitor-local rect (physical pixels) translated onto the virtual
    /// desktop and clipped to this monitor.
    pub fn clamp_local_region(&self, local_rect: Rect) -> Result<Rect, CaptureError> {
        clamp_region(local_rect.translated(self.origin()), self.bounds)
    }

    /// A virtual-desktop rect clipped to this monitor.
    pub fn clamp_region(&self, virtual_rect: Rect) -> Result<Rect, CaptureError> {
        clamp_region(virtual_rect, self.bounds)
    }
}

/// Reject nonsense scale factors from flaky backends. Zero, negative and
/// non-finite scales would silently poison every later division.
fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The set of monitors making up the virtual desktop, plus the layout queries
/// the region-selection overlay needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VirtualDesktop {
    monitors: Vec<MonitorInfo>,
}

impl VirtualDesktop {
    /// Take ownership of an enumeration result.
    pub fn new(monitors: Vec<MonitorInfo>) -> Self {
        Self { monitors }
    }

    /// The monitors, in backend enumeration order.
    pub fn monitors(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    /// No monitors at all — a headless or not-yet-enumerated desktop.
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }

    /// How many monitors are attached.
    pub fn len(&self) -> usize {
        self.monitors.len()
    }

    /// Bounding box of every monitor, in physical pixels. This is the extent of
    /// a [`crate::CaptureTarget::FullDesktop`] capture. Empty when there are no
    /// monitors.
    pub fn bounds(&self) -> Rect {
        bounding_box(self.monitors.iter().map(|m| m.bounds)).unwrap_or_default()
    }

    /// The primary monitor, falling back to the first enumerated one so callers
    /// that just need *a* monitor do not have to handle `None` twice.
    pub fn primary(&self) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.is_primary)
            .or_else(|| self.monitors.first())
    }

    /// Look a monitor up by id.
    pub fn get(&self, id: MonitorId) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.id == id)
    }

    /// Look a monitor up by id, or fail with [`CaptureError::NoSuchMonitor`].
    pub fn require(&self, id: MonitorId) -> Result<&MonitorInfo, CaptureError> {
        self.get(id).ok_or(CaptureError::NoSuchMonitor(id))
    }

    /// Which monitor owns a virtual-desktop point?
    ///
    /// Uses half-open containment, so a point on the seam between two monitors
    /// belongs to the right/lower one. When monitors genuinely overlap (cloned
    /// or misconfigured layouts) the primary wins, then the lowest
    /// [`MonitorId`] — the result is always deterministic, never enumeration
    /// order dependent.
    pub fn monitor_at(&self, point: Vec2D) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .filter(|m| m.contains(point))
            .min_by_key(|m| (!m.is_primary, m.id))
    }

    /// The monitor a rect mostly sits on: the one with the largest intersection
    /// area, ties broken like [`VirtualDesktop::monitor_at`]. Used to pick which
    /// display's scale factor a dragged region should inherit.
    pub fn monitor_for_region(&self, rect: Rect) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .map(|m| (m, rect.clamped_to(m.bounds).area()))
            .filter(|(_, area)| *area > 0.0)
            .max_by(|(am, aa), (bm, ba)| {
                aa.total_cmp(ba)
                    .then_with(|| (!bm.is_primary, bm.id).cmp(&(!am.is_primary, am.id)))
            })
            .map(|(m, _)| m)
    }

    /// Virtual-desktop point -> `(monitor, monitor-local physical point)`.
    pub fn to_local(&self, point: Vec2D) -> Option<(&MonitorInfo, Vec2D)> {
        let monitor = self.monitor_at(point)?;
        Some((monitor, monitor.to_local(point)))
    }

    /// Monitor-local physical point -> virtual-desktop point.
    pub fn to_virtual(&self, id: MonitorId, local: Vec2D) -> Result<Vec2D, CaptureError> {
        Ok(self.require(id)?.to_virtual(local))
    }

    /// Virtual-desktop physical point -> logical point *within* its monitor.
    ///
    /// Returns the monitor too, because a logical coordinate is meaningless
    /// without knowing which display's scale produced it.
    pub fn virtual_to_logical(&self, point: Vec2D) -> Option<(&MonitorInfo, Vec2D)> {
        let (monitor, local) = self.to_local(point)?;
        Some((monitor, monitor.local_to_logical(local)))
    }

    /// Logical point within `id` -> virtual-desktop physical point.
    pub fn logical_to_virtual(
        &self,
        id: MonitorId,
        local_logical: Vec2D,
    ) -> Result<Vec2D, CaptureError> {
        let monitor = self.require(id)?;
        Ok(monitor.to_virtual(monitor.logical_to_local(local_logical)))
    }

    /// Clip a virtual-desktop rect to the whole desktop.
    pub fn clamp_region(&self, rect: Rect) -> Result<Rect, CaptureError> {
        if self.is_empty() {
            return Err(CaptureError::NoDisplay);
        }
        clamp_region(rect, self.bounds())
    }

    /// Clip a virtual-desktop rect to one monitor.
    pub fn clamp_region_to_monitor(&self, id: MonitorId, rect: Rect) -> Result<Rect, CaptureError> {
        self.require(id)?.clamp_region(rect)
    }

    /// The largest scale factor in use, i.e. the finest pixel grid on the
    /// desktop, **never below 1.0**. Stitched full-desktop frames report this:
    /// the composite is in physical pixels, so the most conservative (densest)
    /// scale is the only one that will not make the sharpest monitor's content
    /// look wrong when a consumer treats the frame as uniformly scaled.
    ///
    /// The 1.0 floor covers two cases at once: an empty desktop, which has no
    /// largest anything, and a backend reporting a sub-unit factor — a scale
    /// below 1.0 would tell a consumer to *enlarge* a frame that is already in
    /// physical pixels, which is never what a screenshot wants.
    pub fn max_scale_factor(&self) -> f32 {
        self.monitors
            .iter()
            .map(|m| m.scale_factor)
            .fold(1.0_f32, f32::max)
    }

    /// True when the desktop mixes DPIs, which is where most capture bugs live.
    pub fn is_mixed_dpi(&self) -> bool {
        let mut scales = self.monitors.iter().map(|m| m.scale_factor);
        let Some(first) = scales.next() else {
            return false;
        };
        scales.any(|s| (s - first).abs() > f32::EPSILON)
    }
}

impl From<Vec<MonitorInfo>> for VirtualDesktop {
    fn from(monitors: Vec<MonitorInfo>) -> Self {
        Self::new(monitors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: u64, x: f32, y: f32, w: f32, h: f32, scale: f32, primary: bool) -> MonitorInfo {
        MonitorInfo::new(
            MonitorId::new(id),
            format!("DP-{id}"),
            Rect::from_xywh(x, y, w, h),
            scale,
            primary,
        )
    }

    /// Primary 1920x1080 @100%, a 1280x1024 @100% to its *left* (negative x),
    /// and a 2560x1440 @150% above it (negative y) — the classic Windows layout
    /// that breaks naive code.
    fn three_monitor_desktop() -> VirtualDesktop {
        VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1920.0, 1080.0, 1.0, true),
            monitor(2, -1280.0, 0.0, 1280.0, 1024.0, 1.0, false),
            monitor(3, 0.0, -1440.0, 2560.0, 1440.0, 1.5, false),
        ])
    }

    #[test]
    fn bounds_of_an_empty_desktop_are_empty() {
        let desktop = VirtualDesktop::default();
        assert!(desktop.is_empty());
        assert_eq!(desktop.len(), 0);
        assert!(desktop.bounds().is_empty());
        assert!(desktop.primary().is_none());
    }

    #[test]
    fn bounds_span_negative_origins_in_both_axes() {
        let desktop = three_monitor_desktop();
        assert_eq!(
            desktop.bounds(),
            Rect::from_xywh(-1280.0, -1440.0, 3840.0, 2520.0)
        );
    }

    #[test]
    fn bounds_include_gaps_between_monitors() {
        // A layout with a 500px horizontal gap (possible on X11 with manual
        // xrandr placement) must still produce one contiguous bounding box.
        let desktop = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1000.0, 1000.0, 1.0, true),
            monitor(2, 1500.0, 0.0, 1000.0, 1000.0, 1.0, false),
        ]);
        assert_eq!(desktop.bounds(), Rect::from_xywh(0.0, 0.0, 2500.0, 1000.0));
    }

    #[test]
    fn bounds_handle_overlapping_monitors() {
        let desktop = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1000.0, 1000.0, 1.0, true),
            monitor(2, 500.0, 500.0, 1000.0, 1000.0, 1.0, false),
        ]);
        assert_eq!(desktop.bounds(), Rect::from_xywh(0.0, 0.0, 1500.0, 1500.0));
    }

    #[test]
    fn primary_falls_back_to_the_first_monitor() {
        let desktop = VirtualDesktop::new(vec![
            monitor(9, 0.0, 0.0, 800.0, 600.0, 1.0, false),
            monitor(4, 800.0, 0.0, 800.0, 600.0, 1.0, false),
        ]);
        assert_eq!(desktop.primary().unwrap().id, MonitorId::new(9));
    }

    #[test]
    fn require_reports_missing_monitors() {
        let desktop = three_monitor_desktop();
        assert!(desktop.require(MonitorId::new(1)).is_ok());
        assert!(matches!(
            desktop.require(MonitorId::new(99)),
            Err(CaptureError::NoSuchMonitor(id)) if id == MonitorId::new(99)
        ));
    }

    #[test]
    fn monitor_at_finds_points_on_negative_coordinate_monitors() {
        let desktop = three_monitor_desktop();
        assert_eq!(
            desktop.monitor_at(Vec2D::new(-640.0, 500.0)).unwrap().id,
            MonitorId::new(2)
        );
        assert_eq!(
            desktop.monitor_at(Vec2D::new(100.0, -720.0)).unwrap().id,
            MonitorId::new(3)
        );
        assert_eq!(
            desktop.monitor_at(Vec2D::new(10.0, 10.0)).unwrap().id,
            MonitorId::new(1)
        );
    }

    #[test]
    fn monitor_at_misses_outside_the_desktop_and_in_gaps() {
        let desktop = three_monitor_desktop();
        assert!(desktop.monitor_at(Vec2D::new(5000.0, 5000.0)).is_none());
        // Below the left monitor (1024 tall) but left of the primary: a real
        // hole in this layout.
        assert!(desktop.monitor_at(Vec2D::new(-640.0, 1050.0)).is_none());
    }

    #[test]
    fn monitor_at_gives_a_seam_pixel_to_exactly_one_monitor() {
        let desktop = three_monitor_desktop();
        // x = 0 is the boundary between monitor 2 (ends at 0) and monitor 1.
        assert_eq!(
            desktop.monitor_at(Vec2D::new(0.0, 500.0)).unwrap().id,
            MonitorId::new(1)
        );
        // y = 0 is the boundary between monitor 3 (ends at 0) and monitor 1.
        assert_eq!(
            desktop.monitor_at(Vec2D::new(500.0, 0.0)).unwrap().id,
            MonitorId::new(1)
        );
        // The exclusive far corner of the whole desktop belongs to nobody.
        assert!(desktop.monitor_at(Vec2D::new(1920.0, 1080.0)).is_none());
    }

    #[test]
    fn monitor_at_prefers_the_primary_when_monitors_overlap() {
        let desktop = VirtualDesktop::new(vec![
            monitor(7, 0.0, 0.0, 1000.0, 1000.0, 1.0, false),
            monitor(2, 0.0, 0.0, 1000.0, 1000.0, 1.0, true),
        ]);
        assert_eq!(
            desktop.monitor_at(Vec2D::new(10.0, 10.0)).unwrap().id,
            MonitorId::new(2)
        );
    }

    #[test]
    fn monitor_at_ties_break_on_lowest_id_not_enumeration_order() {
        let desktop = VirtualDesktop::new(vec![
            monitor(7, 0.0, 0.0, 1000.0, 1000.0, 1.0, false),
            monitor(3, 0.0, 0.0, 1000.0, 1000.0, 1.0, false),
        ]);
        assert_eq!(
            desktop.monitor_at(Vec2D::new(10.0, 10.0)).unwrap().id,
            MonitorId::new(3)
        );
    }

    #[test]
    fn monitor_for_region_picks_the_biggest_overlap() {
        let desktop = three_monitor_desktop();
        // Straddles monitors 1 and 2, mostly on 2.
        let rect = Rect::from_xywh(-800.0, 100.0, 900.0, 100.0);
        assert_eq!(
            desktop.monitor_for_region(rect).unwrap().id,
            MonitorId::new(2)
        );
        assert!(
            desktop
                .monitor_for_region(Rect::from_xywh(9000.0, 9000.0, 10.0, 10.0))
                .is_none()
        );
    }

    #[test]
    fn local_and_virtual_coordinates_round_trip() {
        let desktop = three_monitor_desktop();
        let point = Vec2D::new(-1000.0, 300.0);
        let (monitor, local) = desktop.to_local(point).unwrap();
        assert_eq!(monitor.id, MonitorId::new(2));
        assert_eq!(local, Vec2D::new(280.0, 300.0));
        assert_eq!(desktop.to_virtual(monitor.id, local).unwrap(), point);
    }

    #[test]
    fn to_virtual_rejects_unknown_monitors() {
        let desktop = three_monitor_desktop();
        assert!(matches!(
            desktop.to_virtual(MonitorId::new(404), Vec2D::ZERO),
            Err(CaptureError::NoSuchMonitor(_))
        ));
    }

    #[test]
    fn a_150_percent_monitor_keeps_physical_bounds_and_scales_only_locally() {
        let desktop = three_monitor_desktop();
        let hidpi = desktop.get(MonitorId::new(3)).unwrap();
        // Physical bounds are untouched by the scale factor.
        assert_eq!(hidpi.bounds, Rect::from_xywh(0.0, -1440.0, 2560.0, 1440.0));
        assert_eq!(hidpi.physical_size(), Vec2D::new(2560.0, 1440.0));
        // 2560x1440 physical at 150% is 1706.67x960 logical.
        let logical = hidpi.logical_size();
        assert!((logical.x - 1706.6667).abs() < 0.01, "{logical}");
        assert!((logical.y - 960.0).abs() < 0.01, "{logical}");
    }

    #[test]
    fn mixed_dpi_neighbours_do_not_disturb_each_others_physical_layout() {
        // 1920 @100% at x=0, then 2560 @150% at x=1920. The second monitor
        // starts at physical 1920 regardless of its scale.
        let desktop = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1920.0, 1080.0, 1.0, true),
            monitor(2, 1920.0, 0.0, 2560.0, 1440.0, 1.5, false),
        ]);
        assert_eq!(desktop.bounds(), Rect::from_xywh(0.0, 0.0, 4480.0, 1440.0));
        assert!(desktop.is_mixed_dpi());
        assert_eq!(desktop.max_scale_factor(), 1.5);

        // A point 300 physical px into the HiDPI monitor is 200 logical px in.
        let (m, logical) = desktop
            .virtual_to_logical(Vec2D::new(2220.0, 300.0))
            .unwrap();
        assert_eq!(m.id, MonitorId::new(2));
        assert_eq!(logical, Vec2D::new(200.0, 200.0));
        // ...and the same point on the 100% monitor is unchanged.
        let (m, logical) = desktop
            .virtual_to_logical(Vec2D::new(300.0, 300.0))
            .unwrap();
        assert_eq!(m.id, MonitorId::new(1));
        assert_eq!(logical, Vec2D::new(300.0, 300.0));
    }

    #[test]
    fn logical_to_virtual_inverts_virtual_to_logical() {
        let desktop = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1920.0, 1080.0, 1.0, true),
            monitor(2, 1920.0, 0.0, 2560.0, 1440.0, 1.5, false),
        ]);
        let point = Vec2D::new(2220.0, 300.0);
        let (m, logical) = desktop.virtual_to_logical(point).unwrap();
        assert_eq!(desktop.logical_to_virtual(m.id, logical).unwrap(), point);
    }

    #[test]
    fn uniform_dpi_desktops_are_not_flagged_as_mixed() {
        assert!(!VirtualDesktop::default().is_mixed_dpi());
        let desktop = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1920.0, 1080.0, 2.0, true),
            monitor(2, 1920.0, 0.0, 1920.0, 1080.0, 2.0, false),
        ]);
        assert!(!desktop.is_mixed_dpi());
        assert_eq!(desktop.max_scale_factor(), 2.0);
    }

    #[test]
    fn the_largest_scale_factor_is_floored_at_one() {
        // Nothing attached: there is no largest anything.
        assert_eq!(VirtualDesktop::default().max_scale_factor(), 1.0);
        // `sanitize_scale` only rejects non-finite and non-positive factors, so
        // a sub-unit scale reaches here intact -- and is still floored, because
        // a frame already in physical pixels must never be reported as needing
        // enlargement.
        let shrunk = VirtualDesktop::new(vec![
            monitor(1, 0.0, 0.0, 1920.0, 1080.0, 0.5, true),
            monitor(2, 1920.0, 0.0, 1920.0, 1080.0, 0.75, false),
        ]);
        assert_eq!(shrunk.monitors()[0].scale_factor, 0.5);
        assert_eq!(shrunk.max_scale_factor(), 1.0);
    }

    #[test]
    fn broken_scale_factors_are_sanitized_to_one() {
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let m = monitor(1, 0.0, 0.0, 100.0, 100.0, bad, true);
            assert_eq!(m.scale_factor, 1.0, "scale {bad} should sanitize to 1.0");
        }
    }

    #[test]
    fn monitor_local_region_clamping_translates_and_clips() {
        let desktop = three_monitor_desktop();
        let left = desktop.get(MonitorId::new(2)).unwrap();
        // Local (100,100)-(300,300) on the left monitor is virtual
        // (-1180,100)-(-980,300).
        let clamped = left
            .clamp_local_region(Rect::from_xywh(100.0, 100.0, 200.0, 200.0))
            .unwrap();
        assert_eq!(clamped, Rect::from_xywh(-1180.0, 100.0, 200.0, 200.0));

        // A local region running off the right edge gets clipped there.
        let clipped = left
            .clamp_local_region(Rect::from_xywh(1200.0, 0.0, 400.0, 100.0))
            .unwrap();
        assert_eq!(clipped, Rect::from_xywh(-80.0, 0.0, 80.0, 100.0));
    }

    #[test]
    fn region_clamping_against_the_whole_desktop_and_one_monitor_differ() {
        let desktop = three_monitor_desktop();
        let rect = Rect::from_xywh(-1500.0, -100.0, 2000.0, 400.0);
        assert_eq!(
            desktop.clamp_region(rect).unwrap(),
            Rect::from_xywh(-1280.0, -100.0, 1780.0, 400.0)
        );
        assert_eq!(
            desktop
                .clamp_region_to_monitor(MonitorId::new(2), rect)
                .unwrap(),
            Rect::from_xywh(-1280.0, 0.0, 1280.0, 300.0)
        );
    }

    #[test]
    fn clamping_on_an_empty_desktop_is_a_no_display_error() {
        assert!(matches!(
            VirtualDesktop::default().clamp_region(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
            Err(CaptureError::NoDisplay)
        ));
    }

    #[test]
    fn monitor_id_displays_readably() {
        assert_eq!(MonitorId::new(5).to_string(), "monitor#5");
        assert_eq!(MonitorId::new(5).get(), 5);
    }
}
