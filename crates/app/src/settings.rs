//! The settings window.
//!
//! Edits are applied to the live configuration immediately so their effect is
//! visible straight away, but they are only *persisted* when the user asks.
//! That distinction matters: bettershot is often launched from a hotkey for a
//! single screenshot, and a one-off tweak should not silently rewrite the
//! config file for every future run.

use bettershot_core::config::{Action, CaptureMode, Config, SaveFormat, Theme};
use bettershot_core::style::Size;
use bettershot_core::tools::{ObscureKind, Tools};

use crate::egui_painter::to_color;

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    /// Result of the last save attempt, shown next to the button.
    status: Option<String>,
}

impl SettingsWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.status = None;
    }

    /// Draw the window. Returns true when something changed, so the caller can
    /// re-derive whatever depends on the configuration.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        config: &mut Config,
        history: &std::cell::RefCell<crate::history::History>,
    ) -> bool {
        if !self.open {
            return false;
        }

        let mut changed = false;
        let mut open = self.open;

        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_width(380.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    changed |= self.appearance(ui, config);
                    ui.separator();
                    changed |= self.drawing(ui, config);
                    ui.separator();
                    changed |= self.behaviour(ui, config);
                    ui.separator();
                    changed |= self.capture(ui, config);
                    ui.separator();
                    changed |= self.privacy(ui, config, history);
                    ui.separator();
                    self.persist(ui, config);
                });
            });

        self.open = open;
        changed
    }

    fn appearance(&mut self, ui: &mut egui::Ui, config: &mut Config) -> bool {
        let mut changed = false;
        ui.heading("Appearance");

        ui.horizontal(|ui| {
            ui.label("Language");
            let active = crate::i18n::Catalog::new(config.language.parse().unwrap_or_default());
            ui.label(active.language().to_string());
        });
        ui.small("Only English ships today; see crates/app/src/i18n.rs to add one.");

        ui.horizontal(|ui| {
            ui.label("Theme");
            for theme in Theme::ALL {
                if ui
                    .selectable_label(config.theme == theme, theme.name())
                    .clicked()
                {
                    config.theme = theme;
                    changed = true;
                }
            }
        });

        changed |= ui
            .checkbox(&mut config.hide_toolbars, "Hide toolbars by default")
            .changed();

        ui.horizontal(|ui| {
            ui.label("Palette");
            for color in config.color_palette.all() {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect.shrink(1.0), 3.0, to_color(color));
            }
        });

        changed
    }

    fn drawing(&mut self, ui: &mut egui::Ui, config: &mut Config) -> bool {
        let mut changed = false;
        ui.heading("Drawing");

        ui.horizontal(|ui| {
            ui.label("Starting tool");
            egui::ComboBox::from_id_salt("initial-tool")
                .selected_text(config.initial_tool.name())
                .show_ui(ui, |ui| {
                    for tool in Tools::ALL {
                        changed |= ui
                            .selectable_value(&mut config.initial_tool, tool, tool.name())
                            .changed();
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Starting size");
            for size in Size::ALL {
                if ui
                    .selectable_label(config.initial_size == size, size.to_string())
                    .clicked()
                {
                    config.initial_size = size;
                    changed = true;
                }
            }
        });

        changed |= ui
            .add(
                egui::Slider::new(&mut config.annotation_size_factor, 0.25..=4.0)
                    .text("Annotation scale"),
            )
            .changed();

        changed |= ui
            .checkbox(&mut config.default_fill_shapes, "Fill shapes by default")
            .changed();
        changed |= ui
            .checkbox(&mut config.default_round_caps, "Round line caps")
            .changed();

        ui.horizontal(|ui| {
            ui.label("Obscure with");
            for (kind, label) in [
                (ObscureKind::Blur, "blur"),
                (ObscureKind::Pixelate, "pixelate"),
            ] {
                if ui.selectable_label(config.obscure == kind, label).clicked() {
                    config.obscure = kind;
                    changed = true;
                }
            }
        });

        changed
    }

    fn behaviour(&mut self, ui: &mut egui::Ui, config: &mut Config) -> bool {
        let mut changed = false;
        ui.heading("Behaviour");

        changed |= action_row(
            ui,
            "Enter does",
            "action-enter",
            &mut config.action_on_enter,
        );
        changed |= action_row(
            ui,
            "Escape does",
            "action-escape",
            &mut config.action_on_escape,
        );

        changed |= ui
            .checkbox(&mut config.early_exit, "Quit after saving")
            .changed();
        changed |= ui
            .checkbox(&mut config.save_after_copy, "Also save when copying")
            .changed();
        changed |= ui
            .checkbox(
                &mut config.disable_notifications,
                "Disable desktop notifications",
            )
            .changed();

        let mut output = config.output_filename.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Save to");
            if ui.text_edit_singleline(&mut output).changed() {
                config.output_filename = (!output.trim().is_empty()).then_some(output.clone());
                changed = true;
            }
        });
        ui.small("Supports strftime placeholders, e.g. ~/shots/%Y-%m-%d_%H-%M-%S.png");

        ui.horizontal(|ui| {
            ui.label("Format");
            for (format, label) in [
                (SaveFormat::Png, "png"),
                (SaveFormat::Jpeg, "jpeg"),
                (SaveFormat::Webp, "webp"),
            ] {
                if ui
                    .selectable_label(config.save_format == format, label)
                    .clicked()
                {
                    config.save_format = format;
                    changed = true;
                }
            }
        });

        let mut copy_command = config.copy_command.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Copy command");
            if ui.text_edit_singleline(&mut copy_command).changed() {
                config.copy_command =
                    (!copy_command.trim().is_empty()).then_some(copy_command.clone());
                changed = true;
            }
        });
        ui.small("Leave empty to use the system clipboard; e.g. wl-copy on Wayland.");

        changed
    }

    fn capture(&mut self, ui: &mut egui::Ui, config: &mut Config) -> bool {
        let mut changed = false;
        ui.heading("Capture");

        ui.horizontal(|ui| {
            ui.label("Default mode");
            for mode in CaptureMode::ALL {
                if ui
                    .selectable_label(config.capture.mode == mode, mode.name())
                    .clicked()
                {
                    config.capture.mode = mode;
                    changed = true;
                }
            }
        });

        let mut delay = config.capture.delay_seconds as u32;
        if ui
            .add(egui::Slider::new(&mut delay, 0..=30).text("Delay (seconds)"))
            .changed()
        {
            config.capture.delay_seconds = delay as u64;
            changed = true;
        }

        // Disabled rather than removed: the setting is real and will work, but
        // no capture backend reads it yet, and a checkbox that does nothing is
        // worse than one that says so.
        ui.add_enabled_ui(false, |ui| {
            ui.checkbox(
                &mut config.capture.include_cursor,
                "Include the cursor (not implemented yet)",
            );
        });
        changed |= ui
            .checkbox(
                &mut config.capture.snap_to_windows,
                "Snap the selection to windows",
            )
            .changed();

        changed
    }

    /// What bettershot is holding on to, and how to make it stop.
    ///
    /// A screenshot tool keeps pictures of whatever was on screen, so the user
    /// deserves to see how many are in memory and be able to drop them.
    fn privacy(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        history: &std::cell::RefCell<crate::history::History>,
    ) -> bool {
        let mut changed = false;
        ui.heading("Privacy");

        let mut size = config.history_size;
        if ui
            .add(egui::Slider::new(&mut size, 0..=20).text("Recent captures kept"))
            .changed()
        {
            config.history_size = size;
            // Applied immediately, not just recorded. Lowering this is how a
            // user says "stop holding my screenshots", so the surplus has to
            // be released now rather than at some later eviction.
            history.borrow_mut().set_capacity(size);
            changed = true;
        }
        ui.small("Held in memory only, never written to disk. Zero disables it.");

        {
            let held = history.borrow();
            ui.label(format!(
                "{} of {} kept, using {} KB",
                held.len(),
                held.capacity(),
                held.memory_used() / 1024
            ));
        }
        if ui.button("Forget recent captures").clicked() {
            history.borrow_mut().clear();
        }

        changed |= ui
            .checkbox(
                &mut config.crash_reports,
                "Write a local crash report if bettershot panics",
            )
            .changed();
        ui.small("Reports contain no image data and are never transmitted.");

        changed
    }

    fn persist(&mut self, ui: &mut egui::Ui, config: &Config) {
        ui.horizontal(|ui| {
            if ui.button("Save to config file").clicked() {
                self.status = Some(match save_config(config) {
                    Ok(path) => format!("Saved to {}", path.display()),
                    Err(e) => format!("Could not save: {e}"),
                });
            }
            if let Some(status) = &self.status {
                ui.label(status.clone());
            }
        });
        if let Some(path) = bettershot_cli::config_path() {
            ui.small(format!("Config file: {}", path.display()));
        }
        // Saving writes the settings currently in effect, which include any
        // command-line overrides this run was started with. Worth saying,
        // because a compositor keybinding passes flags the user never intended
        // to make permanent.
        ui.small(
            "Writes every setting currently in effect, including any passed on              the command line this run. Comments in the file are not preserved.",
        );
    }
}

/// One "what should this key do?" row.
fn action_row(ui: &mut egui::Ui, label: &str, id: &str, field: &mut Action) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(field.name())
            .show_ui(ui, |ui| {
                for action in Action::ALL {
                    changed |= ui.selectable_value(field, action, action.name()).changed();
                }
            });
    });
    changed
}

/// Write the current settings to the platform config file.
///
/// Written to a temporary file and renamed into place. A plain write truncates
/// first, so a crash or a full disk mid-write leaves a half-written file — and
/// an unparseable config is a hard error at startup, which would leave
/// bettershot unable to launch at all until the user found and deleted it by
/// hand. Rename is atomic on the same filesystem, so the old config survives
/// any failure.
fn save_config(config: &Config) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;

    let path = bettershot_cli::config_path()
        .ok_or_else(|| "could not determine the configuration directory".to_owned())?;
    let toml = config.to_toml().map_err(|e| e.to_string())?;

    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;

    // Same directory, so the rename cannot cross a filesystem boundary.
    let temporary = path.with_extension("toml.new");
    {
        let mut file = std::fs::File::create(&temporary)
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
        file.write_all(toml.as_bytes())
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
        // Without this the rename can land before the contents reach disk, so
        // a power loss leaves an empty file where the config used to be.
        file.sync_all()
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
    }
    std::fs::rename(&temporary, &path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {e}", path.display())
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_starts_closed_and_toggles() {
        let mut settings = SettingsWindow::new();
        assert!(!settings.open);
        settings.toggle();
        assert!(settings.open);
        settings.toggle();
        assert!(!settings.open);
    }

    #[test]
    fn a_closed_window_reports_no_changes_without_a_context() {
        // `show` returns early when closed, which is what lets the editor call
        // it unconditionally every frame.
        let mut settings = SettingsWindow::new();
        let ctx = egui::Context::default();
        let mut config = Config::default();
        let history = std::cell::RefCell::new(crate::history::History::new(3));
        assert!(!settings.show(&ctx, &mut config, &history));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn what_the_settings_window_writes_can_be_read_back() {
        // The save path is exercised end to end in core's round-trip test;
        // here we only check that the document the window would write is
        // valid, since writing needs a real config directory.
        let mut config = Config {
            theme: Theme::Dark,
            ..Default::default()
        };
        config.capture.delay_seconds = 5;

        let written = config.to_toml().expect("should serialise");
        let mut reloaded = Config::default();
        reloaded.merge_toml(&written).expect("should parse");
        assert_eq!(reloaded.theme, Theme::Dark);
        assert_eq!(reloaded.capture.delay_seconds, 5);
    }
}
