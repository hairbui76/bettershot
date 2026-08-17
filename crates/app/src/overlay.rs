//! The region-selection overlay.
//!
//! Selection runs against a **frozen frame**: the screen is captured first and
//! the overlay draws that image, so the overlay can never appear in its own
//! screenshot and nothing moves under the pointer mid-drag.
//!
//! The overlay is a *mode of the same window* as the editor rather than a
//! separate process or a second event loop — winit only reliably supports one
//! event loop per process, so capture and annotation have to share one.
//!
//! The decision logic ("what is selected right now?") lives in
//! [`RegionSelection`], which is pure and tested; the egui code around it only
//! draws.

use bettershot_capture::WindowInfo;
use bettershot_core::config::CaptureMode;
use bettershot_core::math::{Rect, Vec2D};

use crate::egui_painter::{from_pos, to_color, to_rect};

/// Smallest selection worth keeping; below this a drag is treated as a click.
pub const MIN_SELECTION: f32 = 4.0;

/// What the user did with the overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selection {
    /// Still choosing.
    Pending,
    /// Confirmed this region, in image coordinates.
    Confirmed(Rect),
    /// Backed out; the app should exit without annotating.
    Cancelled,
}

/// The pure selection state machine.
#[derive(Debug, Clone)]
pub struct RegionSelection {
    /// The whole frozen frame.
    bounds: Rect,
    /// Candidate window rectangles, in frame coordinates, topmost first.
    windows: Vec<Rect>,
    /// Monitor rectangles, in frame coordinates.
    monitors: Vec<Rect>,
    mode: CaptureMode,
    snap: bool,
    drag: Option<(Vec2D, Vec2D)>,
    hover: Option<Vec2D>,
}

impl RegionSelection {
    pub fn new(
        bounds: Rect,
        windows: Vec<Rect>,
        monitors: Vec<Rect>,
        mode: CaptureMode,
        snap: bool,
    ) -> Self {
        Self {
            bounds,
            windows,
            monitors,
            mode,
            snap,
            drag: None,
            hover: None,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// Switch what a click selects, mid-selection.
    ///
    /// Abandons any drag in progress: the gesture was started with a different
    /// intent, and carrying it across would let a half-dragged rectangle
    /// survive into a mode where dragging is not what the user is doing.
    pub fn set_mode(&mut self, mode: CaptureMode) {
        if self.mode != mode {
            self.mode = mode;
            self.drag = None;
        }
    }

    /// The monitor containing `point`. Monitors do not overlap, so the first
    /// hit is the only hit.
    pub fn monitor_at(&self, point: Vec2D) -> Option<Rect> {
        self.monitors.iter().copied().find(|m| m.contains(point))
    }

    /// What a click selects in the current mode, ignoring drags.
    fn click_target(&self, at: Vec2D) -> Option<Rect> {
        match self.mode {
            CaptureMode::Monitor => self.monitor_at(at),
            // Region mode falls back to the window under the pointer, which is
            // how "click a window to grab it" has always worked.
            _ => self.window_at(at),
        }
    }

    pub fn hover(&self) -> Option<Vec2D> {
        self.hover
    }

    pub fn set_hover(&mut self, pos: Option<Vec2D>) {
        self.hover = pos;
    }

    pub fn begin_drag(&mut self, at: Vec2D) {
        self.drag = Some((at, at));
    }

    pub fn update_drag(&mut self, to: Vec2D) {
        if let Some((_, current)) = &mut self.drag {
            *current = to;
        }
    }

    /// Finish the gesture. Returns the selection, or `None` if the drag was
    /// too small and there was no window to fall back to.
    pub fn end_drag(&mut self, at: Vec2D) -> Option<Rect> {
        let (start, _) = self.drag.take()?;
        let dragged = Rect::from_corners(start, at).clamped_to(self.bounds);
        if dragged.width() >= MIN_SELECTION && dragged.height() >= MIN_SELECTION {
            return Some(dragged.rounded());
        }
        // A click rather than a drag: take whatever the mode says is under the
        // pointer.
        self.click_target(at)
            .map(|r| r.clamped_to(self.bounds).rounded())
    }

    /// The topmost window containing `point`.
    ///
    /// Windows arrive topmost-first, so the first hit wins. Ties on z-order
    /// are broken by area so a small dialog inside a large parent is still
    /// selectable.
    pub fn window_at(&self, point: Vec2D) -> Option<Rect> {
        let mut best: Option<Rect> = None;
        for window in &self.windows {
            if !window.contains(point) {
                continue;
            }
            match best {
                None => best = Some(*window),
                Some(current) if window.area() < current.area() => best = Some(*window),
                _ => {}
            }
        }
        best
    }

    /// What would be captured if the user released right now.
    pub fn current(&self) -> Option<Rect> {
        if let Some((start, current)) = self.drag {
            let rect = Rect::from_corners(start, current).clamped_to(self.bounds);
            if rect.width() >= MIN_SELECTION && rect.height() >= MIN_SELECTION {
                return Some(rect);
            }
            // Fall through to the window hint while the drag is still tiny.
        }
        // Window and monitor modes always preview what is under the pointer;
        // region mode only does so when snapping is enabled and nothing is
        // being dragged.
        let always_hints = matches!(self.mode, CaptureMode::Window | CaptureMode::Monitor);
        if always_hints || (self.snap && self.drag.is_none()) {
            if let Some(pos) = self.hover {
                return self.click_target(pos).map(|r| r.clamped_to(self.bounds));
            }
        }
        None
    }
}

/// The overlay screen, drawn over the frozen frame.
pub struct Overlay {
    selection: RegionSelection,
    texture: Option<egui::TextureHandle>,
    image: image::RgbaImage,
    outcome: Selection,
}

impl Overlay {
    pub fn new(
        image: image::RgbaImage,
        windows: &[WindowInfo],
        monitors: &[Rect],
        frame_origin: Vec2D,
        mode: CaptureMode,
        snap: bool,
    ) -> Self {
        let bounds = Rect::from_xywh(0.0, 0.0, image.width() as f32, image.height() as f32);
        // Window bounds are in virtual-desktop coordinates; the frozen frame
        // starts at `frame_origin`, so rebase them onto the image.
        let windows = windows
            .iter()
            .filter(|w| !w.is_minimized)
            .map(|w| w.bounds.translated(-frame_origin))
            .filter(|r| r.intersects(bounds))
            .collect();

        // Monitor bounds share the window coordinate space, so they rebase the
        // same way.
        let monitors = monitors
            .iter()
            .map(|m| m.translated(-frame_origin))
            .filter(|r| r.intersects(bounds))
            .collect();

        Self {
            selection: RegionSelection::new(bounds, windows, monitors, mode, snap),
            texture: None,
            image,
            outcome: Selection::Pending,
        }
    }

    pub fn outcome(&self) -> Selection {
        self.outcome
    }

    pub fn image(&self) -> &image::RgbaImage {
        &self.image
    }

    fn texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if self.texture.is_none() {
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [self.image.width() as usize, self.image.height() as usize],
                self.image.as_raw(),
            );
            self.texture =
                Some(ctx.load_texture("bettershot-frozen", color, egui::TextureOptions::NEAREST));
        }
        self.texture.as_ref().expect("just populated").id()
    }

    /// Draw one frame of the overlay and process input.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Escape backs out entirely; Enter accepts the current preview.
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                self.outcome = Selection::Cancelled;
            }
            // The digits are free here: they only mean palette colours in the
            // editor, which is a different mode of the same window.
            for (key, mode) in [
                (egui::Key::Num1, CaptureMode::Region),
                (egui::Key::Num2, CaptureMode::Window),
                (egui::Key::Num3, CaptureMode::Monitor),
            ] {
                if i.key_pressed(key) {
                    self.selection.set_mode(mode);
                }
            }
            if i.key_pressed(egui::Key::Num4) {
                self.outcome = Selection::Confirmed(self.selection.bounds().rounded());
            }
        });
        if self.outcome != Selection::Pending {
            return;
        }

        let texture = self.texture(&ctx);
        let bounds = self.selection.bounds();

        let toolbar = self.draw_toolbar(&ctx);
        // A mode button may have finished the selection outright.
        if self.outcome != Selection::Pending {
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let (response, painter) =
                    ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
                let viewport = crate::egui_painter::from_rect(response.rect);

                // The frozen frame is drawn to fill the overlay window; the
                // mapping between the two is a plain scale.
                let scale = (viewport.width() / bounds.width().max(1.0))
                    .min(viewport.height() / bounds.height().max(1.0));
                let drawn = Rect::new(viewport.pos, bounds.size * scale);
                let to_image = |p: egui::Pos2| -> Vec2D {
                    (from_pos(p) - drawn.pos) * (1.0 / scale.max(f32::EPSILON))
                };

                let mut mesh = egui::epaint::Mesh::with_texture(texture);
                mesh.add_rect_with_uv(
                    to_rect(drawn),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(mesh));

                let pointer = ctx.input(|i| i.pointer.latest_pos());
                // Pointer over the bar is pointer over the bar, not over the
                // screenshot: no hover preview, no crosshair, no drag.
                let on_toolbar = pointer.is_some_and(|p| toolbar.contains(p));
                self.selection
                    .set_hover(pointer.filter(|_| !on_toolbar).map(to_image));

                if on_toolbar {
                    self.paint_selection(&painter, drawn, scale);
                    return;
                }

                if response.drag_started() {
                    if let Some(p) = pointer {
                        self.selection.begin_drag(to_image(p));
                    }
                }
                if response.dragged() {
                    if let Some(p) = pointer {
                        self.selection.update_drag(to_image(p));
                    }
                }
                if response.drag_stopped() || response.clicked() {
                    if let Some(p) = pointer {
                        if !self.selection.is_dragging() {
                            self.selection.begin_drag(to_image(p));
                        }
                        if let Some(rect) = self.selection.end_drag(to_image(p)) {
                            self.outcome = Selection::Confirmed(rect);
                        }
                    }
                }

                self.paint_selection(&painter, drawn, scale);
            });
    }

    /// The floating capture-mode bar, the way the Windows Snipping Tool puts
    /// one at the top of the screen.
    ///
    /// Returns the rectangle it occupies so the canvas underneath can ignore
    /// clicks that landed on it. egui does route pointer input to the topmost
    /// layer, but the canvas allocates the *whole* overlay with a drag sense,
    /// and a press that starts on a button and drifts would otherwise begin a
    /// selection behind the bar.
    fn draw_toolbar(&mut self, ctx: &egui::Context) -> egui::Rect {
        // Modes a click can select. "Full screen" is not among them: it is an
        // action, not a mode, and fires immediately like the Snipping Tool's
        // fullscreen snip rather than asking for a second click.
        const MODES: [(CaptureMode, &str, &str); 3] = [
            (CaptureMode::Region, "Region", "Drag out a rectangle  (1)"),
            (
                CaptureMode::Window,
                "Window",
                "Click the window under the pointer  (2)",
            ),
            (
                CaptureMode::Monitor,
                "Monitor",
                "Click the monitor under the pointer  (3)",
            ),
        ];

        let mut bar = egui::Rect::NOTHING;
        egui::Area::new(egui::Id::new("bettershot-capture-modes"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0))
            .show(ctx, |ui| {
                let painted = egui::Frame::popup(ui.style())
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (mode, label, hint) in MODES {
                                let active = self.selection.mode() == mode;
                                if ui
                                    .selectable_label(active, label)
                                    .on_hover_text(hint)
                                    .clicked()
                                {
                                    self.selection.set_mode(mode);
                                }
                            }

                            ui.separator();

                            if ui
                                .button("Full screen")
                                .on_hover_text("Capture everything, right now  (4)")
                                .clicked()
                            {
                                self.outcome =
                                    Selection::Confirmed(self.selection.bounds().rounded());
                            }

                            ui.separator();

                            if ui.button("✕").on_hover_text("Cancel  (Esc)").clicked() {
                                self.outcome = Selection::Cancelled;
                            }
                        });
                    });
                // The frame's own rect, not `ui.min_rect()`: it includes the
                // popup margin and shadow padding, which is the area the user
                // sees and therefore the area a click can land on.
                bar = painted.response.rect;
            });
        bar
    }

    fn paint_selection(&self, painter: &egui::Painter, drawn: Rect, scale: f32) {
        let to_screen = |r: Rect| -> Rect { Rect::new(drawn.pos + r.pos * scale, r.size * scale) };
        let dim = to_color(bettershot_core::style::Color::black().with_alpha(120));

        match self.selection.current() {
            Some(rect) => {
                let screen = to_screen(rect);
                // Dim everything except the selection, in four bands so the
                // seams do not double-darken.
                for band in surrounding_bands(drawn, screen) {
                    painter.rect_filled(to_rect(band), 0.0, dim);
                }
                painter.rect_stroke(
                    to_rect(screen),
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
                let label = format!("{} × {}", rect.width() as u32, rect.height() as u32);
                painter.text(
                    egui::pos2(screen.left(), (screen.top() - 6.0).max(drawn.top() + 12.0)),
                    egui::Align2::LEFT_BOTTOM,
                    label,
                    egui::FontId::monospace(13.0),
                    egui::Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(to_rect(drawn), 0.0, dim);
                // Mode-aware: the bar above changes what a click does, so a
                // fixed line would be telling the user to do the wrong thing
                // two thirds of the time.
                let hint = match self.selection.mode() {
                    CaptureMode::Window => "Click a window · or drag out a region · Esc to cancel",
                    CaptureMode::Monitor => {
                        "Click a monitor · or drag out a region · Esc to cancel"
                    }
                    _ => "Drag to select a region · click a window · Esc to cancel",
                };
                painter.text(
                    to_rect(drawn).center(),
                    egui::Align2::CENTER_CENTER,
                    hint,
                    egui::FontId::proportional(18.0),
                    egui::Color32::WHITE,
                );
            }
        }

        // Crosshair, so the pointer is findable over a busy screenshot.
        if let Some(hover) = self.selection.hover() {
            let p = drawn.pos + hover * scale;
            let stroke = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90));
            painter.line_segment(
                [
                    egui::pos2(drawn.left(), p.y),
                    egui::pos2(drawn.right(), p.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(p.x, drawn.top()),
                    egui::pos2(p.x, drawn.bottom()),
                ],
                stroke,
            );
        }
    }
}

/// The four rectangles covering `outer` but not `hole`.
pub fn surrounding_bands(outer: Rect, hole: Rect) -> Vec<Rect> {
    let hole = hole.normalized().clamped_to(outer);
    if hole.is_empty() {
        return vec![outer];
    }
    [
        Rect::from_xywh(
            outer.left(),
            outer.top(),
            outer.width(),
            hole.top() - outer.top(),
        ),
        Rect::from_xywh(
            outer.left(),
            hole.bottom(),
            outer.width(),
            outer.bottom() - hole.bottom(),
        ),
        Rect::from_xywh(
            outer.left(),
            hole.top(),
            hole.left() - outer.left(),
            hole.height(),
        ),
        Rect::from_xywh(
            hole.right(),
            hole.top(),
            outer.right() - hole.right(),
            hole.height(),
        ),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
    .collect()
}

/// Crop the frozen frame down to the confirmed selection.
pub fn crop_to(image: &image::RgbaImage, rect: Rect) -> image::RgbaImage {
    crate::editor::crop_image(image, rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(mode: CaptureMode, snap: bool) -> RegionSelection {
        RegionSelection::new(
            Rect::from_xywh(0.0, 0.0, 1000.0, 800.0),
            vec![
                // A big window with a small dialog on top of it.
                Rect::from_xywh(0.0, 0.0, 600.0, 500.0),
                Rect::from_xywh(100.0, 100.0, 200.0, 150.0),
            ],
            // Two side-by-side monitors making up the 1000x800 desktop.
            vec![
                Rect::from_xywh(0.0, 0.0, 500.0, 800.0),
                Rect::from_xywh(500.0, 0.0, 500.0, 800.0),
            ],
            mode,
            snap,
        )
    }

    #[test]
    fn monitor_mode_previews_the_monitor_under_the_pointer() {
        let mut s = selection(CaptureMode::Monitor, false);
        s.set_hover(Some(Vec2D::new(700.0, 400.0)));
        assert_eq!(
            s.current().unwrap(),
            Rect::from_xywh(500.0, 0.0, 500.0, 800.0),
            "the right-hand monitor"
        );
        s.set_hover(Some(Vec2D::new(100.0, 400.0)));
        assert_eq!(
            s.current().unwrap(),
            Rect::from_xywh(0.0, 0.0, 500.0, 800.0)
        );
    }

    #[test]
    fn monitor_mode_clicks_select_a_whole_monitor_not_a_window() {
        // The same click in window mode would land on the big window at the
        // origin, so this is the distinction the toolbar buys.
        let mut s = selection(CaptureMode::Monitor, false);
        let at = Vec2D::new(50.0, 50.0);
        s.begin_drag(at);
        assert_eq!(
            s.end_drag(at),
            Some(Rect::from_xywh(0.0, 0.0, 500.0, 800.0))
        );

        let mut s = selection(CaptureMode::Window, false);
        s.begin_drag(at);
        assert_eq!(
            s.end_drag(at),
            Some(Rect::from_xywh(0.0, 0.0, 600.0, 500.0))
        );
    }

    #[test]
    fn switching_mode_abandons_a_drag_in_progress() {
        // The gesture was begun with a different intent. Carrying it over would
        // let a half-dragged rectangle survive into a mode where dragging is
        // not what the user is doing.
        let mut s = selection(CaptureMode::Region, false);
        s.begin_drag(Vec2D::new(10.0, 10.0));
        s.update_drag(Vec2D::new(400.0, 300.0));
        assert!(s.is_dragging());

        s.set_mode(CaptureMode::Window);
        assert!(!s.is_dragging());
        assert_eq!(s.mode(), CaptureMode::Window);
    }

    #[test]
    fn re_selecting_the_current_mode_leaves_a_drag_alone() {
        // Clicking the already-active button is a no-op, not a cancel.
        let mut s = selection(CaptureMode::Region, false);
        s.begin_drag(Vec2D::new(10.0, 10.0));
        s.update_drag(Vec2D::new(400.0, 300.0));
        s.set_mode(CaptureMode::Region);
        assert!(s.is_dragging());
    }

    #[test]
    fn a_deliberate_drag_still_wins_in_every_mode() {
        // Dragging out a rectangle is unambiguous, so it should not be
        // second-guessed just because a click would have meant something else.
        for mode in [
            CaptureMode::Region,
            CaptureMode::Window,
            CaptureMode::Monitor,
        ] {
            let mut s = selection(mode, false);
            s.begin_drag(Vec2D::new(600.0, 600.0));
            let dragged = s.end_drag(Vec2D::new(800.0, 700.0));
            assert_eq!(
                dragged,
                Some(Rect::from_xywh(600.0, 600.0, 200.0, 100.0)),
                "mode {mode:?}"
            );
        }
    }

    #[test]
    fn a_click_outside_every_monitor_selects_nothing() {
        let mut s = selection(CaptureMode::Monitor, false);
        assert!(s.monitor_at(Vec2D::new(5000.0, 5000.0)).is_none());
        s.begin_drag(Vec2D::new(5000.0, 5000.0));
        assert_eq!(s.end_drag(Vec2D::new(5000.0, 5000.0)), None);
    }

    #[test]
    fn dragging_selects_the_dragged_rectangle() {
        let mut s = selection(CaptureMode::Region, false);
        s.begin_drag(Vec2D::new(100.0, 100.0));
        s.update_drag(Vec2D::new(300.0, 250.0));
        assert_eq!(
            s.current().unwrap(),
            Rect::from_xywh(100.0, 100.0, 200.0, 150.0)
        );
        let done = s.end_drag(Vec2D::new(300.0, 250.0)).unwrap();
        assert_eq!(done, Rect::from_xywh(100.0, 100.0, 200.0, 150.0));
    }

    #[test]
    fn dragging_backwards_still_gives_a_positive_rectangle() {
        let mut s = selection(CaptureMode::Region, false);
        s.begin_drag(Vec2D::new(300.0, 250.0));
        let done = s.end_drag(Vec2D::new(100.0, 100.0)).unwrap();
        assert_eq!(done, Rect::from_xywh(100.0, 100.0, 200.0, 150.0));
    }

    #[test]
    fn a_selection_is_clamped_to_the_frame() {
        let mut s = selection(CaptureMode::Region, false);
        s.begin_drag(Vec2D::new(-500.0, -500.0));
        let done = s.end_drag(Vec2D::new(5000.0, 5000.0)).unwrap();
        assert_eq!(done, Rect::from_xywh(0.0, 0.0, 1000.0, 800.0));
    }

    #[test]
    fn clicking_without_dragging_picks_the_window_underneath() {
        let mut s = selection(CaptureMode::Region, true);
        s.begin_drag(Vec2D::new(150.0, 150.0));
        let done = s.end_drag(Vec2D::new(151.0, 150.0)).expect("should snap");
        // The small dialog wins over the large window behind it.
        assert_eq!(done, Rect::from_xywh(100.0, 100.0, 200.0, 150.0));
    }

    #[test]
    fn the_innermost_window_wins_when_they_overlap() {
        let s = selection(CaptureMode::Window, true);
        assert_eq!(
            s.window_at(Vec2D::new(150.0, 150.0)).unwrap(),
            Rect::from_xywh(100.0, 100.0, 200.0, 150.0)
        );
        // Outside the dialog but inside the big window.
        assert_eq!(
            s.window_at(Vec2D::new(500.0, 400.0)).unwrap(),
            Rect::from_xywh(0.0, 0.0, 600.0, 500.0)
        );
        assert!(s.window_at(Vec2D::new(900.0, 700.0)).is_none());
    }

    #[test]
    fn window_mode_previews_the_window_under_the_pointer() {
        let mut s = selection(CaptureMode::Window, false);
        s.set_hover(Some(Vec2D::new(150.0, 150.0)));
        assert_eq!(
            s.current().unwrap(),
            Rect::from_xywh(100.0, 100.0, 200.0, 150.0),
            "window mode should preview even with snapping off"
        );
    }

    #[test]
    fn region_mode_without_snapping_shows_no_window_hint() {
        let mut s = selection(CaptureMode::Region, false);
        s.set_hover(Some(Vec2D::new(150.0, 150.0)));
        assert!(s.current().is_none());
    }

    #[test]
    fn region_mode_with_snapping_hints_until_a_drag_starts() {
        let mut s = selection(CaptureMode::Region, true);
        s.set_hover(Some(Vec2D::new(150.0, 150.0)));
        assert!(s.current().is_some(), "hint before dragging");

        s.begin_drag(Vec2D::new(400.0, 400.0));
        s.update_drag(Vec2D::new(700.0, 600.0));
        assert_eq!(
            s.current().unwrap(),
            Rect::from_xywh(400.0, 400.0, 300.0, 200.0),
            "the drag takes over from the hint"
        );
    }

    #[test]
    fn a_click_on_empty_space_selects_nothing() {
        let mut s = selection(CaptureMode::Region, true);
        s.begin_drag(Vec2D::new(900.0, 700.0));
        assert!(s.end_drag(Vec2D::new(900.0, 700.0)).is_none());
    }

    #[test]
    fn ending_a_drag_that_never_started_is_harmless() {
        let mut s = selection(CaptureMode::Region, true);
        assert!(s.end_drag(Vec2D::new(10.0, 10.0)).is_none());
        assert!(!s.is_dragging());
    }

    #[test]
    fn windows_are_rebased_onto_the_frozen_frame() {
        // A frame starting at (-1920, 0) — a monitor to the left of the
        // primary, which is where Windows puts negative coordinates.
        let windows = vec![WindowInfo {
            id: bettershot_capture::WindowId::new(1),
            title: "w".into(),
            app_name: "a".into(),
            bounds: Rect::from_xywh(-1900.0, 100.0, 400.0, 300.0),
            is_minimized: false,
            z_order: 0,
        }];
        let image = image::RgbaImage::new(1920, 1080);
        let overlay = Overlay::new(
            image,
            &windows,
            &[],
            Vec2D::new(-1920.0, 0.0),
            CaptureMode::Window,
            true,
        );
        // In frame coordinates the window starts at x = 20.
        assert_eq!(
            overlay.selection.window_at(Vec2D::new(100.0, 200.0)),
            Some(Rect::from_xywh(20.0, 100.0, 400.0, 300.0))
        );
    }

    #[test]
    fn minimised_windows_are_not_selectable() {
        let windows = vec![WindowInfo {
            id: bettershot_capture::WindowId::new(1),
            title: "hidden".into(),
            app_name: "a".into(),
            bounds: Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            is_minimized: true,
            z_order: 0,
        }];
        let overlay = Overlay::new(
            image::RgbaImage::new(500, 500),
            &windows,
            &[],
            Vec2D::ZERO,
            CaptureMode::Window,
            true,
        );
        assert!(
            overlay
                .selection
                .window_at(Vec2D::new(50.0, 50.0))
                .is_none()
        );
    }

    #[test]
    fn surrounding_bands_cover_everything_except_the_hole() {
        let outer = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::from_xywh(25.0, 25.0, 50.0, 50.0);
        let bands = surrounding_bands(outer, hole);
        let covered: f32 = bands.iter().map(|b| b.area()).sum();
        assert!((covered - (10000.0 - 2500.0)).abs() < 1e-2, "{covered}");
        // No band may overlap the hole.
        assert!(bands.iter().all(|b| !b.intersects(hole)));
    }

    #[test]
    fn an_empty_hole_means_everything_is_dimmed() {
        let outer = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert_eq!(surrounding_bands(outer, Rect::default()), vec![outer]);
    }
}
