//! The annotation editor window.
//!
//! This is the shell: it owns the image, the scene and the active tool, and it
//! is the only place that knows about egui. Its job is translation — window
//! input into [`bettershot_core::input`] events, and the scene back out onto an
//! [`crate::egui_painter::EguiPainter`]. Anything that could be decided without
//! a window belongs in core instead.

use std::time::{Duration, Instant};

use bettershot_core::Scene;
use bettershot_core::config::{Action, Config};
use bettershot_core::input::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, PointerTracker, TextEvent,
};
use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::style::{Size, Style};
use bettershot_core::tools::{MarkerTool, Tool, ToolEvent, ToolUpdateResult, Tools};
use image::RgbaImage;

use crate::effects::{self, EffectTextures};
use crate::egui_painter::{EguiPainter, from_pos, from_rect, to_color, to_rect};

use crate::view::View;

/// How long a status message stays on screen.
const STATUS_DURATION: Duration = Duration::from_secs(3);

/// What the editor wants the process to do next. Returned so `main` owns the
/// exit path rather than the editor calling `std::process::exit` from inside a
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outcome {
    #[default]
    Continue,
    Quit,
}

pub struct Editor {
    config: Config,
    /// The unannotated screenshot. Never mutated; crops replace it wholesale.
    base: RgbaImage,
    base_texture: Option<egui::TextureHandle>,
    effects: EffectTextures,

    scene: Scene,
    tool: Box<dyn Tool>,
    tool_kind: Tools,
    style: Style,

    view: View,
    pointer: PointerTracker,
    /// Post-paint selection: which committed annotation is picked, and the
    /// total distance it has been dragged so far. Only meaningful with the
    /// Pointer tool.
    selected: Option<usize>,
    drag_total: Vec2D,
    /// True while a middle-button pan is in progress.
    panning: bool,

    show_toolbars: bool,
    settings: crate::settings::SettingsWindow,
    /// Set when the settings window changed the configuration, so the app can
    /// take the new values back — the editor only holds a clone.
    config_changed: bool,
    /// Resolved UI strings.
    strings: crate::i18n::Catalog,
    /// Recent captures, shared with the app so they outlive this editor.
    history: std::rc::Rc<std::cell::RefCell<crate::history::History>>,
    /// Where the last successful save went, for the copy-path action.
    last_saved: Option<std::path::PathBuf>,
    status: Option<(String, Instant)>,
    outcome: Outcome,
    /// Set when the scene changes, so effect textures are only recollected
    /// when they might have changed.
    scene_dirty: bool,
}

impl Editor {
    /// Build an editor sharing a history with whatever created it. The app
    /// owns the history so it survives being handed a new editor per capture.
    pub fn with_history(
        config: Config,
        base: RgbaImage,
        history: std::rc::Rc<std::cell::RefCell<crate::history::History>>,
    ) -> Self {
        let size = Vec2D::new(base.width() as f32, base.height() as f32);
        let style = config.initial_style();
        let tool_kind = config.initial_tool;
        let mut tool = tool_kind.create(style);
        tool.set_canvas_bounds(Rect::new(Vec2D::ZERO, size));

        Self {
            show_toolbars: !config.hide_toolbars,
            settings: crate::settings::SettingsWindow::new(),
            config_changed: false,
            strings: crate::i18n::Catalog::new(config.language.parse().unwrap_or_default()),
            history,
            last_saved: None,
            base,
            base_texture: None,
            effects: EffectTextures::new(),
            scene: Scene::new(size),
            tool,
            tool_kind,
            style,
            view: View::new(size, Rect::default()),
            pointer: PointerTracker::new(),
            selected: None,
            drag_total: Vec2D::ZERO,
            panning: false,
            status: None,
            outcome: Outcome::Continue,
            scene_dirty: true,
            config,
        }
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "read by the editor's tests"))]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn base_image(&self) -> &RgbaImage {
        &self.base
    }

    /// Take the updated configuration, if the settings window changed it.
    ///
    /// The editor holds a *clone* of the configuration, so without this a
    /// setting changed here would be lost the moment the editor is replaced —
    /// which in daemon mode is on the very next capture.
    pub fn take_config_change(&mut self) -> Option<Config> {
        std::mem::take(&mut self.config_changed).then(|| self.config.clone())
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    // --- tools ------------------------------------------------------------

    /// Switch tools, giving the outgoing one a chance to commit its work
    /// (which the text tool relies on to avoid losing what was typed).
    pub fn set_tool(&mut self, kind: Tools) {
        if kind == self.tool_kind {
            return;
        }
        let result = self.tool.handle_event(ToolEvent::Deactivated);
        self.apply(result);

        self.tool_kind = kind;
        self.selected = None;
        self.drag_total = Vec2D::ZERO;
        self.tool = kind.create(self.style);
        self.tool
            .set_canvas_bounds(Rect::new(Vec2D::ZERO, self.scene.size()));
        self.sync_marker_counter();
        let result = self.tool.handle_event(ToolEvent::Activated);
        self.apply(result);
    }

    /// Keep the marker tool's numbering aligned with the scene, so undo and
    /// redo do not leave gaps or repeats.
    fn sync_marker_counter(&mut self) {
        if self.tool_kind != Tools::Marker {
            return;
        }
        let next = self.scene.next_marker_number();
        // Downcasting is avoided elsewhere, but the marker counter genuinely
        // lives in the tool and has to be reconciled with the document.
        let mut fresh = MarkerTool::new(self.style);
        fresh.set_next_number(next);
        self.tool = Box::new(fresh);
    }

    fn set_style(&mut self, style: Style) {
        self.style = style;
        let result = self.tool.handle_event(ToolEvent::StyleChanged(style));
        self.apply(result);
    }

    /// Fold a tool's result back into the document.
    fn apply(&mut self, result: ToolUpdateResult) {
        if let ToolUpdateResult::Commit(drawable) = result {
            self.scene.add(drawable);
            self.scene_dirty = true;
            self.sync_marker_counter();
        }
    }

    fn send(&mut self, event: InputEvent) {
        let result = self.tool.handle_event(ToolEvent::Input(event));
        self.apply(result);
    }

    // --- document actions -------------------------------------------------

    fn undo(&mut self) {
        if self.scene.undo() {
            self.after_document_change();
            self.set_status(self.strings.get("status.undo"));
        }
    }

    fn redo(&mut self) {
        if self.scene.redo() {
            self.after_document_change();
            self.set_status(self.strings.get("status.redo"));
        }
    }

    fn reset(&mut self) {
        self.scene.clear();
        self.after_document_change();
        self.set_status(self.strings.get("status.cleared"));
    }

    /// Anything that changes the scene wholesale: refresh derived state.
    fn after_document_change(&mut self) {
        self.scene_dirty = true;
        // The selected annotation may no longer exist.
        if self
            .selected
            .is_some_and(|i| self.scene.annotation(i).is_none())
        {
            self.selected = None;
        }
        self.sync_marker_counter();
        let bounds = Rect::new(Vec2D::ZERO, self.scene.size());
        self.tool.set_canvas_bounds(bounds);
        if self.view.image_size() != self.scene.size() {
            self.view.set_image_size(self.scene.size());
        }
    }

    /// Apply the crop tool's current selection to the document.
    fn apply_crop(&mut self) {
        if self.tool_kind != Tools::Crop {
            return;
        }
        // The crop rectangle lives in the tool; read it back before the tool
        // is replaced.
        let Some(rect) = self.tool.crop_selection() else {
            self.set_status(self.strings.get("status.nothing_to_crop"));
            return;
        };
        if !self.scene.apply_crop(rect) {
            return;
        }
        self.base = crop_image(&self.base, rect);
        self.base_texture = None;
        self.effects.clear();
        self.view.set_image_size(self.scene.size());
        self.after_document_change();
        self.set_status(format!(
            "Cropped to {}×{}",
            self.scene.size().x as u32,
            self.scene.size().y as u32
        ));
        // A fresh crop tool now covers the new, smaller canvas.
        self.tool = Tools::Crop.create(self.style);
        self.tool
            .set_canvas_bounds(Rect::new(Vec2D::ZERO, self.scene.size()));
    }

    // --- post-paint editing ------------------------------------------------

    /// Handle pointer input for the Pointer tool, which selects, drags and
    /// deletes annotations that have already been committed rather than
    /// drawing new ones.
    ///
    /// Returns true when the event was consumed.
    fn handle_selection(&mut self, event: &bettershot_core::input::MouseEvent) -> bool {
        use bettershot_core::input::MouseEventKind as Kind;

        match event.kind {
            Kind::Press | Kind::Click => {
                self.selected = self.scene.hit_test(event.pos);
                self.drag_total = Vec2D::ZERO;
                true
            }
            Kind::BeginDrag | Kind::UpdateDrag => {
                let Some(index) = self.selected else {
                    return false;
                };
                // Nudge live, and record the whole gesture once on release, so
                // a drag is a single undo step.
                let step = event.delta - self.drag_total;
                if self.scene.nudge_annotation(index, step) {
                    self.drag_total = event.delta;
                    self.scene_dirty = true;
                }
                true
            }
            Kind::EndDrag => {
                if let Some(index) = self.selected {
                    let step = event.delta - self.drag_total;
                    if !step.is_zero() {
                        self.scene.nudge_annotation(index, step);
                    }
                    self.scene.record_move(index, event.delta);
                    self.scene_dirty = true;
                }
                self.drag_total = Vec2D::ZERO;
                true
            }
            Kind::Motion => false,
        }
    }

    fn delete_selected(&mut self) {
        let Some(index) = self.selected else {
            self.set_status(self.strings.get("status.nothing_selected"));
            return;
        };
        if self.scene.delete_annotation(index) {
            self.selected = None;
            self.after_document_change();
            self.set_status(self.strings.get("status.deleted"));
        }
    }

    /// Outline the selected annotation so the selection is visible.
    fn paint_selection(&self, painter: &egui::Painter) {
        let Some(bounds) = self
            .selected
            .and_then(|i| self.scene.annotation(i))
            .and_then(|d| d.bounds())
        else {
            return;
        };
        let screen = to_rect(self.view.image_rect_to_screen(bounds.expanded(2.0)));
        painter.rect_stroke(
            screen,
            2.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 190, 255)),
            egui::StrokeKind::Outside,
        );
    }

    // --- input ------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let mods = to_modifiers(modifiers);
                    if self.handle_accelerator(key, mods) {
                        continue;
                    }
                    if let Some(k) = to_core_key(key) {
                        self.send(InputEvent::Key(KeyEvent::new(k, mods)));
                    }
                }
                // Only the text tool consumes typed characters; for every
                // other tool they fall through so digits can pick colours.
                egui::Event::Text(text) if self.tool.wants_text_input() => {
                    self.send(InputEvent::Text(TextEvent::Commit(text)));
                }
                _ => {}
            }
        }
    }

    /// Application-level shortcuts. Returns true when the key was consumed and
    /// must not reach the tool.
    fn handle_accelerator(&mut self, key: egui::Key, mods: Modifiers) -> bool {
        if mods.command() {
            match key {
                egui::Key::Z if mods.shift => {
                    self.redo();
                    return true;
                }
                egui::Key::Z => {
                    self.undo();
                    return true;
                }
                egui::Key::Y => {
                    self.redo();
                    return true;
                }
                egui::Key::C if mods.alt => {
                    self.copy_last_path();
                    return true;
                }
                egui::Key::C => {
                    self.perform(Action::SaveToClipboard);
                    return true;
                }
                egui::Key::S if mods.shift => {
                    self.perform(Action::SaveAs);
                    return true;
                }
                egui::Key::S => {
                    self.perform(Action::SaveToFile);
                    return true;
                }
                egui::Key::T => {
                    self.show_toolbars = !self.show_toolbars;
                    return true;
                }
                egui::Key::Comma => {
                    self.settings.toggle();
                    return true;
                }
                egui::Key::Plus | egui::Key::Equals => {
                    self.view.zoom_step(2.0, self.view.viewport().center());
                    return true;
                }
                egui::Key::Minus => {
                    self.view.zoom_step(-2.0, self.view.viewport().center());
                    return true;
                }
                egui::Key::Num0 => {
                    self.view.fit();
                    return true;
                }
                _ => return false,
            }
        }

        match key {
            egui::Key::Escape => {
                // Escape cancels the tool's work first; only when there is
                // nothing to cancel does it trigger the configured action.
                if self.tool.is_active() {
                    let result = self.tool.handle_event(ToolEvent::Dismissed);
                    self.apply(result);
                } else {
                    self.perform(self.config.action_on_escape);
                }
                true
            }
            egui::Key::Enter => {
                // The text tool needs Enter for newlines, so it wins.
                if self.tool.wants_text_input() && self.tool.is_active() {
                    false
                } else if self.tool_kind == Tools::Crop {
                    self.apply_crop();
                    true
                } else {
                    self.perform(self.config.action_on_enter);
                    true
                }
            }
            egui::Key::Delete if mods.shift => {
                self.reset();
                true
            }
            egui::Key::Delete | egui::Key::Backspace if self.tool_kind == Tools::Pointer => {
                self.delete_selected();
                true
            }
            _ => {
                // Digits pick palette colours, but not while typing.
                if self.tool.wants_text_input() && self.tool.is_active() {
                    return false;
                }
                if let Some(index) = digit_index(key) {
                    if let Some(color) = self.config.color_palette.nth(index) {
                        self.set_style(self.style.with_color(color));
                    }
                    return true;
                }
                false
            }
        }
    }

    fn handle_pointer(&mut self, ctx: &egui::Context, response: &egui::Response) {
        let (pressed, released, latest, modifiers, middle_down, wheel) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.latest_pos(),
                to_modifiers(i.modifiers),
                i.pointer.middle_down(),
                i.smooth_scroll_delta,
            )
        });

        let Some(screen) = latest else { return };
        let screen = from_pos(screen);
        let image = self.view.screen_to_image(screen);

        // Zoom takes priority over everything else the wheel could do.
        if wheel.y != 0.0 && modifiers.ctrl {
            self.view.zoom_step(wheel.y / 20.0, screen);
        } else if wheel != egui::Vec2::ZERO {
            self.view.pan(Vec2D::new(wheel.x, wheel.y));
            self.view.clamp_to_viewport();
        }

        // Middle-drag pans, which is why every tool ignores the middle button.
        let pan_delta = ctx.input(|i| i.pointer.delta());
        if middle_down {
            if !self.panning {
                // Starting a pan abandons any gesture in progress. Without
                // this the tracker stays latched and the next plain mouse move
                // is reported as a drag.
                self.pointer.cancel();
            }
            self.panning = true;
            self.view.pan(Vec2D::new(pan_delta.x, pan_delta.y));
            self.view.clamp_to_viewport();
            return;
        }
        self.panning = false;

        let over_canvas = response.hovered() || self.pointer.is_pressed();

        // Press and release are handled independently rather than as
        // alternatives: egui reports both in one frame for a click that starts
        // and finishes between two repaints, and an `else if` here swallowed
        // the release and left the tracker latched — the tool would then
        // rubber-band a shape with no button held.
        if pressed && over_canvas {
            let event = self.pointer.press(image, MouseButton::Left, modifiers);
            self.dispatch_mouse(event);
        }
        if released {
            if let Some(event) = self.pointer.release(image, modifiers) {
                self.dispatch_mouse(event);
            }
        } else if !pressed
            && pan_delta != egui::Vec2::ZERO
            && (over_canvas || self.pointer.is_pressed())
        {
            let event = self.pointer.motion(image, modifiers);
            self.dispatch_mouse(event);
        }
    }

    /// The Pointer tool edits what is already there; every other tool draws.
    fn dispatch_mouse(&mut self, event: bettershot_core::input::MouseEvent) {
        if self.tool_kind == Tools::Pointer {
            self.handle_selection(&event);
        } else {
            self.send(InputEvent::Mouse(event));
        }
    }

    // --- output -----------------------------------------------------------

    fn perform(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Exit => self.outcome = Outcome::Quit,
            Action::SaveToClipboard => {
                match crate::output::copy_to_clipboard(&self.base, &self.scene, &self.config) {
                    Ok(()) => {
                        self.set_status(self.strings.get("status.copied"));
                        self.remember();
                        crate::notify::copied(&self.config);
                        if self.config.save_after_copy {
                            self.perform(Action::SaveToFile);
                            return;
                        }
                        self.maybe_exit();
                    }
                    Err(e) => self.set_status(format!("Copy failed: {e}")),
                }
            }
            Action::SaveToFile => {
                match crate::output::save_to_file(&self.base, &self.scene, &self.config) {
                    Ok(path) => {
                        self.set_status(format!("Saved to {}", path.display()));
                        crate::notify::saved(&self.config, &path);
                        self.last_saved = Some(path);
                        self.maybe_exit();
                    }
                    Err(e) => self.set_status(format!("Save failed: {e}")),
                }
            }
            Action::SaveAs => {
                match crate::output::save_with_dialog(&self.base, &self.scene, &self.config) {
                    Ok(Some(path)) => {
                        self.set_status(format!("Saved to {}", path.display()));
                        crate::notify::saved(&self.config, &path);
                        self.last_saved = Some(path);
                        self.maybe_exit();
                    }
                    // The user cancelled the dialog; not an error, and
                    // certainly not a reason to exit.
                    Ok(None) => {}
                    Err(e) => self.set_status(format!("Save failed: {e}")),
                }
            }
        }
    }

    /// Record the current result so it can be copied again later.
    fn remember(&mut self) {
        if self.config.history_size == 0 {
            return;
        }
        let rendered = crate::output::render_annotated(&self.base, &self.scene);
        let Ok(png) = crate::output::encode(&rendered, bettershot_core::config::SaveFormat::Png)
        else {
            return;
        };
        let entry = crate::history::Entry {
            png,
            width: rendered.width(),
            height: rendered.height(),
            label: format!(
                "{}×{}, {} annotation(s)",
                rendered.width(),
                rendered.height(),
                self.scene.annotation_count()
            ),
        };
        self.history.borrow_mut().push(entry);
    }

    /// Copy a remembered capture back to the clipboard.
    fn recopy(&mut self, index: usize) {
        let entry = self.history.borrow().get(index).cloned();
        let Some(entry) = entry else { return };
        match crate::output::copy_png_bytes(&entry.png, &self.config) {
            Ok(()) => {
                self.set_status(format!("Copied {} again", entry.size_label()));
                crate::notify::copied(&self.config);
            }
            Err(e) => self.set_status(format!("Copy failed: {e}")),
        }
    }

    /// Put the path of the most recent save on the clipboard, so it can be
    /// pasted into a chat or a terminal without hunting for it.
    fn copy_last_path(&mut self) {
        let Some(path) = self.last_saved.clone() else {
            self.set_status(self.strings.get("status.nothing_saved"));
            return;
        };
        match crate::output::copy_text(&path.display().to_string()) {
            Ok(()) => self.set_status(format!("Copied path: {}", path.display())),
            Err(e) => self.set_status(format!("Could not copy the path: {e}")),
        }
    }

    fn maybe_exit(&mut self) {
        if self.config.early_exit {
            self.outcome = Outcome::Quit;
        }
    }

    // --- painting ---------------------------------------------------------

    fn base_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if self.base_texture.is_none() {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [self.base.width() as usize, self.base.height() as usize],
                self.base.as_raw(),
            );
            self.base_texture = Some(ctx.load_texture(
                "bettershot-base",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.base_texture.as_ref().expect("just populated").id()
    }

    fn paint_canvas(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        // Refresh the obscure-effect textures before anything samples them.
        // Recomputed whenever the document changed or a tool is previewing an
        // obscure annotation, since a drag moves the rectangle every frame.
        let previewing = self
            .tool
            .drawable()
            .map(effects::effects_in_drawable)
            .unwrap_or_default();
        if self.scene_dirty || !previewing.is_empty() || !self.effects.is_empty() {
            let mut needed = effects::effects_in_scene(&self.scene);
            needed.extend(previewing);
            // Disjoint field borrows: no copy of the screenshot is made here.
            self.effects.ensure(ctx, &self.base, needed);
            self.scene_dirty = false;
        }

        let base_id = self.base_texture(ctx);
        let image_rect = to_rect(self.view.image_screen_rect());

        // A checker-free flat backdrop, then the screenshot itself.
        painter.rect_filled(painter.clip_rect(), 0.0, egui::Color32::from_gray(24));
        let mut mesh = egui::epaint::Mesh::with_texture(base_id);
        mesh.add_rect_with_uv(
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        painter.add(egui::Shape::mesh(mesh));

        let mut p = EguiPainter::new(painter, self.view).with_effects(&self.effects);
        self.scene.draw(&mut p);
        if let Some(preview) = self.tool.drawable() {
            preview.draw(&mut p);
        }
        self.paint_selection(painter);
    }

    fn toolbars(&mut self, ui: &mut egui::Ui) {
        if !self.show_toolbars {
            return;
        }

        egui::Panel::top("bettershot-tools").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for kind in Tools::ALL {
                    let selected = kind == self.tool_kind;
                    if ui
                        .selectable_label(selected, tool_label(&self.strings, kind))
                        .on_hover_text(kind.name())
                        .clicked()
                    {
                        self.set_tool(kind);
                    }
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.scene.can_undo(),
                        egui::Button::new(self.strings.get("action.undo")),
                    )
                    .on_hover_text("Ctrl+Z")
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(
                        self.scene.can_redo(),
                        egui::Button::new(self.strings.get("action.redo")),
                    )
                    .on_hover_text("Ctrl+Y")
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                if ui
                    .button(self.strings.get("action.copy"))
                    .on_hover_text("Ctrl+C")
                    .clicked()
                {
                    self.perform(Action::SaveToClipboard);
                }
                if ui
                    .button(self.strings.get("action.save"))
                    .on_hover_text("Ctrl+S")
                    .clicked()
                {
                    self.perform(Action::SaveToFile);
                }
                if ui
                    .button(self.strings.get("action.settings"))
                    .on_hover_text("Ctrl+,")
                    .clicked()
                {
                    self.settings.toggle();
                }
                let recent: Vec<(usize, String)> = self
                    .history
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i, e.label.clone()))
                    .collect();
                if !recent.is_empty() {
                    let mut recopy = None;
                    ui.menu_button(self.strings.get("action.recent"), |ui| {
                        for (index, label) in &recent {
                            if ui.button(label).clicked() {
                                recopy = Some(*index);
                                ui.close();
                            }
                        }
                    });
                    if let Some(index) = recopy {
                        self.recopy(index);
                    }
                }
                if self.tool_kind == Tools::Crop
                    && ui
                        .button(self.strings.get("action.apply_crop"))
                        .on_hover_text("Enter")
                        .clicked()
                {
                    self.apply_crop();
                }
            });
        });

        egui::Panel::bottom("bettershot-style").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.tool_kind.uses_style();
                ui.add_enabled_ui(enabled, |ui| {
                    for (index, color) in self.config.color_palette.all().into_iter().enumerate() {
                        let selected = color == self.style.color;
                        let size = egui::vec2(22.0, 22.0);
                        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                        let painter = ui.painter();
                        painter.rect_filled(rect.shrink(2.0), 3.0, to_color(color));
                        if selected {
                            painter.rect_stroke(
                                rect.shrink(1.0),
                                3.0,
                                egui::Stroke::new(2.0, egui::Color32::WHITE),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let hint = if index < 10 {
                            format!("{} — press {}", color, (index + 1) % 10)
                        } else {
                            color.to_string()
                        };
                        if response.on_hover_text(hint).clicked() {
                            self.set_style(self.style.with_color(color));
                        }
                    }

                    ui.separator();
                    for size in Size::ALL {
                        if ui
                            .selectable_label(self.style.size == size, size.to_string())
                            .clicked()
                        {
                            self.set_style(self.style.with_size(size));
                        }
                    }

                    ui.separator();
                    let mut fill = self.style.fill;
                    if ui
                        .checkbox(&mut fill, self.strings.get("action.fill"))
                        .changed()
                    {
                        self.set_style(self.style.with_fill(fill));
                    }
                });

                ui.separator();
                ui.label(format!("{:.0}%", self.view.zoom() * 100.0));
                if ui.button(self.strings.get("action.fit")).clicked() {
                    self.view.fit();
                }
                if ui.button(self.strings.get("action.actual_size")).clicked() {
                    self.view.zoom_to_actual_size();
                }

                if let Some((message, at)) = &self.status {
                    if at.elapsed() < STATUS_DURATION {
                        ui.separator();
                        ui.label(message.clone());
                    }
                }
            });
        });
    }
}

impl Editor {
    /// Draw one frame and process its input.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        self.handle_keys(&ctx);
        self.toolbars(ui);

        // Settings edits apply to the live configuration, so a changed style
        // default or size factor has to reach the active tool immediately.
        if self.settings.show(&ctx, &mut self.config, &self.history) {
            self.config_changed = true;
            let style = Style {
                annotation_size_factor: self.config.annotation_size_factor,
                round_caps: self.config.default_round_caps,
                ..self.style
            };
            self.set_style(style);
            self.show_toolbars = !self.config.hide_toolbars;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let (response, painter) =
                    ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
                self.view.set_viewport(from_rect(response.rect));
                self.handle_pointer(&ctx, &response);
                self.paint_canvas(&ctx, &painter);
            });

        if self
            .status
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= STATUS_DURATION)
        {
            self.status = None;
        }
    }
}

/// Crop the backing image, clamped to its bounds.
pub fn crop_image(base: &RgbaImage, rect: Rect) -> RgbaImage {
    let bounds = Rect::from_xywh(0.0, 0.0, base.width() as f32, base.height() as f32);
    let r = rect.normalized().clamped_to(bounds).rounded();
    let (x, y) = (r.left().max(0.0) as u32, r.top().max(0.0) as u32);
    let w = (r.width() as u32)
        .min(base.width().saturating_sub(x))
        .max(1);
    let h = (r.height() as u32)
        .min(base.height().saturating_sub(y))
        .max(1);
    image::imageops::crop_imm(base, x, y, w, h).to_image()
}

/// The toolbar label for a tool, translated.
///
/// The key is derived from the tool's own name so adding a tool cannot forget
/// to add a label — the catalogue test will fail instead.
fn tool_label(strings: &crate::i18n::Catalog, kind: Tools) -> &'static str {
    match kind {
        Tools::Pointer => strings.get("tool.pointer"),
        Tools::Crop => strings.get("tool.crop"),
        Tools::Line => strings.get("tool.line"),
        Tools::Arrow => strings.get("tool.arrow"),
        Tools::Rectangle => strings.get("tool.rectangle"),
        Tools::Ellipse => strings.get("tool.ellipse"),
        Tools::Text => strings.get("tool.text"),
        Tools::Marker => strings.get("tool.marker"),
        Tools::Brush => strings.get("tool.brush"),
        Tools::Highlight => strings.get("tool.highlight"),
        Tools::Blur => strings.get("tool.blur"),
    }
}

pub fn to_modifiers(m: egui::Modifiers) -> Modifiers {
    Modifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.mac_cmd || m.command && cfg!(target_os = "macos"),
    }
}

/// Map an egui key onto the toolkit-neutral key the tools understand.
pub fn to_core_key(key: egui::Key) -> Option<Key> {
    use egui::Key as E;
    Some(match key {
        E::Escape => Key::Escape,
        E::Enter => Key::Enter,
        E::Backspace => Key::Backspace,
        E::Delete => Key::Delete,
        E::Tab => Key::Tab,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::Home => Key::Home,
        E::End => Key::End,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Space => Key::Space,
        E::Plus => Key::Plus,
        E::Minus => Key::Minus,
        // Printable characters arrive separately as `Event::Text`, which is
        // what the text tool actually consumes; reporting them here as well
        // would double every keystroke.
        _ => return None,
    })
}

/// Palette index for a digit key, with `0` meaning the tenth colour.
pub fn digit_index(key: egui::Key) -> Option<usize> {
    use egui::Key as E;
    Some(match key {
        E::Num1 => 0,
        E::Num2 => 1,
        E::Num3 => 2,
        E::Num4 => 3,
        E::Num5 => 4,
        E::Num6 => 5,
        E::Num7 => 6,
        E::Num8 => 7,
        E::Num9 => 8,
        E::Num0 => 9,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]))
    }

    fn editor() -> Editor {
        Editor::with_history(
            Config::default(),
            image(400, 300),
            crate::history::History::shared(5),
        )
    }

    #[test]
    fn a_new_editor_starts_on_the_configured_tool_with_an_empty_scene() {
        let e = editor();
        assert_eq!(e.tool_kind, Tools::Pointer);
        assert!(e.scene().is_empty());
        assert_eq!(e.scene().size(), Vec2D::new(400.0, 300.0));
        assert_eq!(e.outcome(), Outcome::Continue);
    }

    #[test]
    fn switching_tools_creates_the_new_tool() {
        let mut e = editor();
        e.set_tool(Tools::Arrow);
        assert_eq!(e.tool_kind, Tools::Arrow);
        assert_eq!(e.tool.kind(), Tools::Arrow);
    }

    #[test]
    fn switching_away_from_text_keeps_what_was_typed() {
        let mut e = editor();
        e.set_tool(Tools::Text);
        // Click to open a box, then type into it.
        let press = e
            .pointer
            .press(Vec2D::new(10.0, 10.0), MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let release = e
            .pointer
            .release(Vec2D::new(10.0, 10.0), Modifiers::NONE)
            .unwrap();
        e.send(InputEvent::Mouse(release));
        e.send(InputEvent::Text(TextEvent::Commit("hello".into())));

        assert!(e.scene().is_empty(), "not committed yet");
        e.set_tool(Tools::Pointer);
        assert_eq!(e.scene().annotation_count(), 1, "text must survive");
    }

    #[test]
    fn drawing_then_undoing_and_redoing_walks_the_history() {
        let mut e = editor();
        e.set_tool(Tools::Rectangle);
        let press = e
            .pointer
            .press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let motion = e.pointer.motion(Vec2D::new(100.0, 80.0), Modifiers::NONE);
        e.send(InputEvent::Mouse(motion));
        let release = e
            .pointer
            .release(Vec2D::new(100.0, 80.0), Modifiers::NONE)
            .unwrap();
        e.send(InputEvent::Mouse(release));

        assert_eq!(e.scene().annotation_count(), 1);
        e.undo();
        assert_eq!(e.scene().annotation_count(), 0);
        e.redo();
        assert_eq!(e.scene().annotation_count(), 1);
    }

    #[test]
    fn marker_numbering_stays_in_step_with_undo() {
        let mut e = editor();
        e.set_tool(Tools::Marker);
        let place = |e: &mut Editor, at: Vec2D| {
            let press = e.pointer.press(at, MouseButton::Left, Modifiers::NONE);
            e.send(InputEvent::Mouse(press));
            let release = e.pointer.release(at, Modifiers::NONE).unwrap();
            e.send(InputEvent::Mouse(release));
        };
        place(&mut e, Vec2D::new(10.0, 10.0));
        place(&mut e, Vec2D::new(50.0, 50.0));
        assert_eq!(e.scene().next_marker_number(), 3);

        e.undo();
        assert_eq!(
            e.scene().next_marker_number(),
            2,
            "the freed number is reused"
        );
    }

    #[test]
    fn reset_clears_every_annotation() {
        let mut e = editor();
        e.set_tool(Tools::Marker);
        let press = e
            .pointer
            .press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let release = e.pointer.release(Vec2D::ZERO, Modifiers::NONE).unwrap();
        e.send(InputEvent::Mouse(release));
        assert!(!e.scene().is_empty());

        e.reset();
        assert!(e.scene().is_empty());
        assert!(!e.scene().can_undo());
    }

    #[test]
    fn cropping_the_backing_image_clamps_to_its_bounds() {
        let base = image(100, 100);
        let out = crop_image(&base, Rect::from_xywh(-50.0, -50.0, 120.0, 120.0));
        assert_eq!(out.dimensions(), (70, 70));

        let out = crop_image(&base, Rect::from_xywh(90.0, 90.0, 100.0, 100.0));
        assert_eq!(out.dimensions(), (10, 10));
    }

    #[test]
    fn cropping_to_nothing_still_yields_a_valid_image() {
        let base = image(100, 100);
        let out = crop_image(&base, Rect::from_xywh(0.0, 0.0, 0.0, 0.0));
        assert!(out.width() >= 1 && out.height() >= 1, "must not be empty");
    }

    #[test]
    fn escape_cancels_the_tool_before_it_quits_the_app() {
        let mut e = editor();
        e.set_tool(Tools::Rectangle);
        let press = e
            .pointer
            .press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let motion = e.pointer.motion(Vec2D::new(50.0, 50.0), Modifiers::NONE);
        e.send(InputEvent::Mouse(motion));
        assert!(e.tool.is_active());

        e.handle_accelerator(egui::Key::Escape, Modifiers::NONE);
        assert!(!e.tool.is_active(), "the shape should be cancelled");
        assert_eq!(e.outcome(), Outcome::Continue, "and the app should stay up");

        // With nothing to cancel, Escape performs the configured action.
        e.handle_accelerator(egui::Key::Escape, Modifiers::NONE);
        assert_eq!(e.outcome(), Outcome::Quit);
    }

    #[test]
    fn digits_select_palette_colours() {
        let mut e = editor();
        e.set_tool(Tools::Arrow);
        let second = e.config.color_palette.nth(1).unwrap();
        e.handle_accelerator(egui::Key::Num2, Modifiers::NONE);
        assert_eq!(e.style.color, second);
    }

    #[test]
    fn digits_do_not_hijack_typing() {
        let mut e = editor();
        e.set_tool(Tools::Text);
        let press = e
            .pointer
            .press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let release = e.pointer.release(Vec2D::ZERO, Modifiers::NONE).unwrap();
        e.send(InputEvent::Mouse(release));

        let before = e.style.color;
        let consumed = e.handle_accelerator(egui::Key::Num2, Modifiers::NONE);
        assert!(!consumed, "the digit must reach the text tool");
        assert_eq!(e.style.color, before);
    }

    #[test]
    fn enter_is_left_to_the_text_tool_while_typing() {
        let mut e = editor();
        e.set_tool(Tools::Text);
        let press = e
            .pointer
            .press(Vec2D::ZERO, MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let release = e.pointer.release(Vec2D::ZERO, Modifiers::NONE).unwrap();
        e.send(InputEvent::Mouse(release));

        assert!(!e.handle_accelerator(egui::Key::Enter, Modifiers::NONE));
        assert_eq!(e.outcome(), Outcome::Continue);
    }

    // --- post-paint editing -------------------------------------------------

    /// Draw one rectangle so there is something to select.
    fn editor_with_a_rectangle() -> Editor {
        let mut e = editor();
        e.set_tool(Tools::Rectangle);
        let press = e
            .pointer
            .press(Vec2D::new(50.0, 50.0), MouseButton::Left, Modifiers::NONE);
        e.send(InputEvent::Mouse(press));
        let motion = e.pointer.motion(Vec2D::new(150.0, 130.0), Modifiers::NONE);
        e.send(InputEvent::Mouse(motion));
        let release = e
            .pointer
            .release(Vec2D::new(150.0, 130.0), Modifiers::NONE)
            .unwrap();
        e.send(InputEvent::Mouse(release));
        assert_eq!(e.scene().annotation_count(), 1);
        e.set_tool(Tools::Pointer);
        e
    }

    fn click_canvas(e: &mut Editor, at: Vec2D) {
        let press = e.pointer.press(at, MouseButton::Left, Modifiers::NONE);
        e.dispatch_mouse(press);
        let release = e.pointer.release(at, Modifiers::NONE).unwrap();
        e.dispatch_mouse(release);
    }

    #[test]
    fn a_click_that_starts_and_ends_in_one_frame_does_not_latch_the_pointer() {
        // egui reports primary_pressed and primary_released together when a
        // click completes within a single repaint. Treating them as
        // alternatives lost the release and left a phantom drag following the
        // cursor with no button held.
        let mut e = editor();
        e.set_tool(Tools::Rectangle);

        let at = Vec2D::new(40.0, 40.0);
        let press = e.pointer.press(at, MouseButton::Left, Modifiers::NONE);
        e.dispatch_mouse(press);
        let release = e.pointer.release(at, Modifiers::NONE).unwrap();
        e.dispatch_mouse(release);

        assert!(
            !e.pointer.is_pressed(),
            "the tracker stayed latched after a same-frame click"
        );
        let moved = e.pointer.motion(Vec2D::new(200.0, 150.0), Modifiers::NONE);
        assert_eq!(
            moved.kind,
            bettershot_core::input::MouseEventKind::Motion,
            "a plain mouse move was reported as a drag"
        );
    }

    #[test]
    fn the_pointer_tool_selects_an_annotation_it_is_clicked_on() {
        let mut e = editor_with_a_rectangle();
        click_canvas(&mut e, Vec2D::new(100.0, 90.0));
        assert_eq!(e.selected, Some(0));

        // Clicking empty space clears the selection.
        click_canvas(&mut e, Vec2D::new(350.0, 280.0));
        assert_eq!(e.selected, None);
    }

    #[test]
    fn the_pointer_tool_never_draws() {
        let mut e = editor_with_a_rectangle();
        let before = e.scene().annotation_count();
        let press = e
            .pointer
            .press(Vec2D::new(300.0, 250.0), MouseButton::Left, Modifiers::NONE);
        e.dispatch_mouse(press);
        let motion = e.pointer.motion(Vec2D::new(380.0, 290.0), Modifiers::NONE);
        e.dispatch_mouse(motion);
        let release = e
            .pointer
            .release(Vec2D::new(380.0, 290.0), Modifiers::NONE)
            .unwrap();
        e.dispatch_mouse(release);
        assert_eq!(
            e.scene().annotation_count(),
            before,
            "nothing new was drawn"
        );
    }

    #[test]
    fn dragging_a_selected_annotation_moves_it_as_one_undo_step() {
        let mut e = editor_with_a_rectangle();
        let before = e.scene().annotation(0).unwrap().bounds().unwrap().pos;

        let start = Vec2D::new(100.0, 90.0);
        let press = e.pointer.press(start, MouseButton::Left, Modifiers::NONE);
        e.dispatch_mouse(press);
        // Several drag updates, as a real pointer would produce.
        for step in [20.0f32, 40.0, 60.0] {
            let m = e
                .pointer
                .motion(start + Vec2D::new(step, step / 2.0), Modifiers::NONE);
            e.dispatch_mouse(m);
        }
        let end = start + Vec2D::new(60.0, 30.0);
        let release = e.pointer.release(end, Modifiers::NONE).unwrap();
        e.dispatch_mouse(release);

        let after = e.scene().annotation(0).unwrap().bounds().unwrap().pos;
        assert!(
            (after.x - (before.x + 60.0)).abs() < 0.01,
            "{after} vs {before}"
        );
        assert!(
            (after.y - (before.y + 30.0)).abs() < 0.01,
            "{after} vs {before}"
        );

        // One undo restores the whole drag.
        e.undo();
        let restored = e.scene().annotation(0).unwrap().bounds().unwrap().pos;
        assert!(
            (restored.x - before.x).abs() < 0.01,
            "{restored} vs {before}"
        );
    }

    #[test]
    fn delete_removes_the_selected_annotation_and_undo_brings_it_back() {
        let mut e = editor_with_a_rectangle();
        click_canvas(&mut e, Vec2D::new(100.0, 90.0));
        assert_eq!(e.selected, Some(0));

        e.handle_accelerator(egui::Key::Delete, Modifiers::NONE);
        assert_eq!(e.scene().annotation_count(), 0);
        assert_eq!(e.selected, None, "the selection goes with it");

        e.undo();
        assert_eq!(e.scene().annotation_count(), 1);
    }

    #[test]
    fn delete_with_nothing_selected_does_not_destroy_anything() {
        let mut e = editor_with_a_rectangle();
        e.handle_accelerator(egui::Key::Delete, Modifiers::NONE);
        assert_eq!(e.scene().annotation_count(), 1);
    }

    #[test]
    fn shift_delete_still_clears_everything_rather_than_deleting_one() {
        let mut e = editor_with_a_rectangle();
        click_canvas(&mut e, Vec2D::new(100.0, 90.0));
        e.handle_accelerator(
            egui::Key::Delete,
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        );
        assert!(e.scene().is_empty());
    }

    #[test]
    fn switching_tools_drops_the_selection() {
        let mut e = editor_with_a_rectangle();
        click_canvas(&mut e, Vec2D::new(100.0, 90.0));
        assert!(e.selected.is_some());
        e.set_tool(Tools::Arrow);
        assert_eq!(e.selected, None);
    }

    #[test]
    fn undoing_past_the_selected_annotation_clears_the_selection() {
        let mut e = editor_with_a_rectangle();
        click_canvas(&mut e, Vec2D::new(100.0, 90.0));
        assert!(e.selected.is_some());
        e.undo(); // removes the rectangle itself
        assert_eq!(e.selected, None, "a stale index must not survive");
    }

    #[test]
    fn key_mapping_covers_the_editing_keys_and_ignores_characters() {
        assert_eq!(to_core_key(egui::Key::Escape), Some(Key::Escape));
        assert_eq!(to_core_key(egui::Key::ArrowLeft), Some(Key::Left));
        assert_eq!(to_core_key(egui::Key::Backspace), Some(Key::Backspace));
        // Letters arrive as text events instead, so they must not map here.
        assert_eq!(to_core_key(egui::Key::A), None);
    }

    #[test]
    fn digit_keys_map_with_zero_last() {
        assert_eq!(digit_index(egui::Key::Num1), Some(0));
        assert_eq!(digit_index(egui::Key::Num9), Some(8));
        assert_eq!(digit_index(egui::Key::Num0), Some(9));
        assert_eq!(digit_index(egui::Key::A), None);
    }

    #[test]
    fn the_accelerator_modifier_follows_the_platform() {
        // On macOS the accelerator is Command, everywhere else it is Control.
        // egui reports Command as `mac_cmd` on macOS, so that has to map onto
        // `meta`, which is what `Modifiers::command()` reads there.
        let cmd = to_modifiers(egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        });
        assert!(cmd.meta, "Command must map to meta");
        assert_eq!(
            cmd.command(),
            cfg!(target_os = "macos"),
            "Command is the accelerator on macOS only — a Linux host that \
             somehow saw mac_cmd should not treat it as one"
        );

        let ctrl = to_modifiers(egui::Modifiers {
            ctrl: true,
            command: !cfg!(target_os = "macos"),
            ..Default::default()
        });
        assert_eq!(
            ctrl.command(),
            !cfg!(target_os = "macos"),
            "Control is the accelerator everywhere except macOS"
        );
    }

    #[test]
    fn modifier_mapping_preserves_each_flag() {
        let m = to_modifiers(egui::Modifiers {
            shift: true,
            ctrl: true,
            alt: false,
            ..Default::default()
        });
        assert!(m.shift && m.ctrl && !m.alt);
    }
}
