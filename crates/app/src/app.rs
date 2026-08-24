//! The top-level application state machine.
//!
//! Capture, selection and annotation all share one window and one event loop,
//! because winit only reliably supports a single event loop per process. So
//! choosing a region is a *stage* of this app rather than a separate program,
//! and staying resident for a hotkey is another one.
//!
//! ```text
//!            ┌──────────────► Idle ◄───────────┐   (daemon mode only)
//!            │                 │               │
//!  --daemon ─┘        hotkey / tray            │ editor finishes
//!                              ▼               │
//!  --capture region ──► Selecting ──► Editing ─┘
//!                                       ▲
//!  --filename / stdin ──────────────────┘
//! ```

use bettershot_core::config::{CaptureMode, Config, Theme};
use image::RgbaImage;

use std::cell::RefCell;
use std::rc::Rc;

use crate::capture::Acquired;
use crate::daemon::{Hotkeys, Tray, Trigger};
use crate::editor::{Editor, Outcome};
use crate::history::History;
use crate::overlay::{Overlay, Selection};

/// How often the hidden window wakes to check for a hotkey. Frequent enough
/// that a keypress feels instant, rare enough to stay off the CPU.
const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(80);

enum Stage {
    /// Resident and hidden, waiting for a hotkey or a tray click.
    Idle,
    /// A capture is running on a worker thread.
    ///
    /// Capture can take a long time — a portal consent dialog waits on the
    /// user, macOS allows up to 30 s of timeouts, and `--delay` adds whatever
    /// the user configured on top. Doing that on the UI thread freezes the
    /// tray and, on Windows and macOS, shows the process as hung. Both capture
    /// backends' docs say to use a worker thread, and `CaptureBackend: Send`
    /// exists to permit it.
    Capturing(std::sync::mpsc::Receiver<Result<Acquired, String>>),
    /// Choosing a region from the frozen frame.
    Selecting(Box<Overlay>),
    /// Annotating.
    Editing(Box<Editor>),
    /// Showing settings on their own, with no capture behind them. Reached
    /// from the tray, where conjuring a blank screenshot just to host the
    /// window would be a confusing thing to show someone.
    Settings(Box<crate::settings::SettingsWindow>),
    /// Finished; the window should close.
    Done,
}

pub struct BettershotApp {
    stage: Stage,
    config: Config,
    /// Set once the process should exit.
    finished: bool,
    /// The theme currently applied to egui. Compared each frame so a change
    /// from the settings window takes effect; a one-shot latch meant the theme
    /// control did nothing at all.
    applied_theme: Option<Theme>,
    /// Recent captures. Owned here rather than by the editor so that in daemon
    /// mode, where a fresh editor is built per capture, the history persists.
    history: Rc<RefCell<History>>,
    /// Present only in daemon mode.
    daemon: Option<Daemon>,
    /// Cleared once the startup timing has been reported.
    report_first_frame: bool,
}

struct Daemon {
    hotkeys: Hotkeys,
    tray: Option<Tray>,
    /// Reported once, the first time a window is visible.
    warnings: Vec<String>,
    warned: bool,
}

impl BettershotApp {
    /// Start straight in the editor, for a file, stdin, or a whole-screen
    /// capture that needs no selection.
    pub fn editing(config: Config, image: RgbaImage) -> Self {
        let mut app = Self::with_stage(config.clone(), Stage::Idle);
        let editor = Editor::with_history(config, image, Rc::clone(&app.history));
        app.stage = Stage::Editing(Box::new(editor));
        app
    }

    /// Start with the region-selection overlay.
    pub fn selecting(config: Config, overlay: Overlay) -> Self {
        Self::with_stage(config, Stage::Selecting(Box::new(overlay)))
    }

    /// Start resident and hidden, driven by hotkeys and the tray.
    ///
    /// Fails when nothing could drive it. Staying up in that state means a
    /// hidden window polling forever with no way to trigger a capture and no
    /// way to quit short of `kill` — and it is the *normal* case for
    /// `--daemon` on Wayland in a build without the `tray` feature, so it has
    /// to be an error the user sees rather than a warning routed through a
    /// window that will never open.
    pub fn idle(config: Config) -> Result<Self, String> {
        let hotkeys = Hotkeys::register(&config.daemon);
        let mut warnings: Vec<String> = hotkeys.failures().to_vec();

        let tray = if config.daemon.tray {
            match Tray::new(&config.daemon) {
                Ok(tray) => Some(tray),
                Err(e) => {
                    warnings.push(format!("the system tray is unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };

        log::info!("{}", hotkeys.summary());
        for warning in &warnings {
            log::warn!("{warning}");
        }

        if !hotkeys.is_active() && tray.is_none() {
            let mut message = String::from(
                "--daemon has nothing that could start a capture, so it would run \
                 invisibly forever. ",
            );
            for warning in &warnings {
                message.push_str(warning);
                message.push_str(". ");
            }
            message.push_str(
                "Bind your compositor to `bettershot --capture region` instead, or \
                 build with --features tray",
            );
            return Err(message);
        }

        // Only once the daemon is actually going to run. Announcing "bettershot
        // is running" and then returning an error would be a lie, and it would
        // also make every test that exercises the refusal path talk to the
        // notification daemon.
        let (title, mut body) = hotkeys.announcement();
        if !warnings.is_empty() {
            if let Some(path) = bettershot_cli::config_path() {
                body.push_str(&format!("\n\nConfig file: {}", path.display()));
            }
        }
        crate::notify::notify(&config, &title, &body);

        let mut app = Self::with_stage(config, Stage::Idle);
        app.daemon = Some(Daemon {
            hotkeys,
            tray,
            warnings,
            warned: false,
        });
        Ok(app)
    }

    fn with_stage(config: Config, stage: Stage) -> Self {
        Self {
            history: History::shared(config.history_size),
            stage,
            config,
            finished: false,
            applied_theme: None,
            daemon: None,
            report_first_frame: true,
        }
    }

    /// Build the app for a capture that has already been taken.
    pub fn stage_for(config: &Config, acquired: Acquired) -> Self {
        if acquired.needs_selection {
            let overlay = Overlay::new(
                acquired.image,
                &acquired.windows,
                &acquired.monitors,
                acquired.origin,
                acquired.mode,
                config.capture.snap_to_windows,
            );
            Self::selecting(config.clone(), overlay)
        } else {
            Self::editing(config.clone(), acquired.image)
        }
    }

    /// The selection overlay has to cover the whole screen with no chrome; the
    /// editor opens as an ordinary window.
    pub fn starts_fullscreen(&self) -> bool {
        matches!(self.stage, Stage::Selecting(_)) || self.config.fullscreen
    }

    /// Whether the window should sit above other windows.
    ///
    /// The selection overlay takes this regardless of the setting: it is
    /// fullscreen, but "fullscreen" does not mean "in front", and any
    /// always-on-top window belonging to another application would otherwise
    /// float over the very region the user is trying to drag out.
    pub fn starts_on_top(&self) -> bool {
        matches!(self.stage, Stage::Selecting(_)) || self.config.always_on_top
    }

    /// True when the process should start with no visible window.
    pub fn starts_hidden(&self) -> bool {
        matches!(self.stage, Stage::Idle)
    }

    /// Window size to request at startup.
    pub fn initial_size(&self) -> egui::Vec2 {
        let [w, h] = match &self.stage {
            Stage::Editing(editor) => {
                let image = editor.base_image();
                crate::initial_window_size([image.width() as f32, image.height() as f32])
            }
            // Fullscreen anyway, but a sane fallback if the request is refused.
            Stage::Selecting(overlay) => {
                let image = overlay.image();
                [image.width() as f32, image.height() as f32]
            }
            Stage::Idle | Stage::Done | Stage::Capturing(_) => [640.0, 480.0],
            Stage::Settings(_) => [560.0, 720.0],
        };
        egui::vec2(w, h)
    }

    // --- transitions ------------------------------------------------------

    /// Move from selection to editing, cropping the frozen frame down.
    fn enter_editor(&mut self, ctx: &egui::Context, region: bettershot_core::math::Rect) {
        let Stage::Selecting(overlay) = std::mem::replace(&mut self.stage, Stage::Done) else {
            return;
        };
        let cropped = crate::overlay::crop_to(overlay.image(), region);
        drop(overlay);
        self.show_editor(ctx, cropped);
    }

    fn show_editor(&mut self, ctx: &egui::Context, image: RgbaImage) {
        // The overlay ran borderless and fullscreen; the editor wants an
        // ordinary window sized to the capture.
        let size = crate::initial_window_size([image.width() as f32, image.height() as f32]);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(
            !self.config.no_window_decoration,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            size[0], size[1],
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("bettershot".into()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);

        self.stage = Stage::Editing(Box::new(Editor::with_history(
            self.config.clone(),
            image,
            Rc::clone(&self.history),
        )));
    }

    /// Start a capture on a worker thread.
    fn begin_capture(&mut self, ctx: &egui::Context, mode: CaptureMode) {
        // Hide first: the window must never appear in its own screenshot.
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));

        let (tx, rx) = std::sync::mpsc::channel();
        let config = self.config.clone();
        // Waking the UI thread when the result lands, rather than relying on
        // the idle poll, keeps the delay between capture and overlay short.
        let waker = ctx.clone();
        match std::thread::Builder::new()
            .name("bettershot-capture".into())
            .spawn(move || {
                let result = crate::capture::acquire(mode, &config).map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
                waker.request_repaint();
            }) {
            Ok(_handle) => self.stage = Stage::Capturing(rx),
            Err(e) => {
                // Spawning failed, which is close to fatal, but falling back to
                // a blocking capture is better than losing the request.
                log::warn!("could not spawn a capture thread ({e}); capturing inline");
                let result =
                    crate::capture::acquire(mode, &self.config).map_err(|e| format!("{e:#}"));
                self.finish_capture(ctx, result);
            }
        }
    }

    /// Hand a finished capture to the right stage.
    fn finish_capture(&mut self, ctx: &egui::Context, result: Result<Acquired, String>) {
        match result {
            Ok(acquired) if acquired.needs_selection => {
                let overlay = Overlay::new(
                    acquired.image,
                    &acquired.windows,
                    &acquired.monitors,
                    acquired.origin,
                    acquired.mode,
                    self.config.capture.snap_to_windows,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.stage = Stage::Selecting(Box::new(overlay));
            }
            Ok(acquired) => self.show_editor(ctx, acquired.image),
            Err(e) => {
                log::error!("capture failed: {e}");
                if let Some(daemon) = &mut self.daemon {
                    daemon.warnings.push(format!("Capture failed: {e}"));
                    daemon.warned = false;
                }
                self.finish_or_idle(ctx);
            }
        }
    }

    /// A capture finished. In daemon mode go back to waiting; otherwise exit.
    fn finish_or_idle(&mut self, ctx: &egui::Context) {
        if self.daemon.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.stage = Stage::Idle;
        } else {
            self.stage = Stage::Done;
            self.finished = true;
        }
    }

    /// Drain the hotkey and tray channels, whatever stage we are in.
    fn take_trigger(&mut self) -> Option<Trigger> {
        let daemon = self.daemon.as_ref()?;
        let from_tray = daemon.tray.as_ref().and_then(|tray| tray.poll());
        from_tray.or_else(|| daemon.hotkeys.poll().map(Trigger::Capture))
    }

    /// Act on a trigger. Ignored unless we are idle, except for Quit, which
    /// must work from anywhere — otherwise the tray's Quit does nothing while
    /// a window is open.
    fn act_on(&mut self, ctx: &egui::Context, trigger: Option<Trigger>) {
        match trigger {
            Some(Trigger::Capture(mode)) => self.begin_capture(ctx, mode),
            Some(Trigger::Settings) => self.show_settings(ctx),
            Some(Trigger::Quit) => {
                self.stage = Stage::Done;
                self.finished = true;
            }
            None => {}
        }
    }

    /// Show the settings window on its own.
    fn show_settings(&mut self, ctx: &egui::Context) {
        let mut window = crate::settings::SettingsWindow::new();
        window.open = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(560.0, 720.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("bettershot — Settings".into()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.stage = Stage::Settings(Box::new(window));
    }

    /// Surface any daemon warnings once, on the first visible frame.
    fn report_warnings(&mut self) {
        let Some(daemon) = &mut self.daemon else {
            return;
        };
        if daemon.warned || daemon.warnings.is_empty() {
            return;
        }
        let message = daemon.warnings.join("; ");
        daemon.warned = true;
        if let Stage::Editing(editor) = &mut self.stage {
            editor.set_status(message);
        }
    }
}

impl eframe::App for BettershotApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _ = frame;
        let ctx = ui.ctx().clone();

        if self.report_first_frame {
            self.report_first_frame = false;
            // The number a user cares about is "how long until I could see
            // it", so this is logged at the first frame rather than at window
            // creation. Run with -v to see it.
            if let Some(elapsed) = crate::since_start() {
                log::info!("first frame after {:.1}ms", elapsed.as_secs_f64() * 1000.0);
            }
        }

        if self.applied_theme != Some(self.config.theme) {
            match self.config.theme {
                Theme::System => ctx.set_theme(egui::ThemePreference::System),
                Theme::Light => ctx.set_theme(egui::ThemePreference::Light),
                Theme::Dark => ctx.set_theme(egui::ThemePreference::Dark),
            }
            self.applied_theme = Some(self.config.theme);
        }

        // Hotkeys and the tray stay live while the editor or overlay is up, so
        // their events must be drained in every stage. Draining only while
        // idle let them queue: a tray "Quit" looked ignored, then fired the
        // moment the editor closed.
        let pending = self.take_trigger();

        match &mut self.stage {
            Stage::Idle => {
                // Keep waking up: a hidden window receives no input of its own.
                ctx.request_repaint_after(IDLE_POLL);
                self.act_on(&ctx, pending);
            }
            Stage::Capturing(rx) => {
                match rx.try_recv() {
                    Ok(result) => self.finish_capture(&ctx, result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // Still working. Keep checking, but leave the UI
                        // thread free so the tray stays responsive.
                        ctx.request_repaint_after(IDLE_POLL);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // The worker died without sending, which means it
                        // panicked. Do not wait forever for a result that is
                        // never coming.
                        log::error!("the capture thread stopped without a result");
                        self.finish_or_idle(&ctx);
                    }
                }
                // Quit must still work while a capture is in flight.
                if pending == Some(Trigger::Quit) {
                    self.stage = Stage::Done;
                    self.finished = true;
                }
            }
            Stage::Selecting(overlay) => {
                if pending == Some(Trigger::Quit) {
                    self.stage = Stage::Done;
                    self.finished = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                overlay.draw(ui);
                match overlay.outcome() {
                    Selection::Confirmed(region) => self.enter_editor(&ctx, region),
                    // Backing out returns to waiting in daemon mode rather
                    // than killing the process.
                    Selection::Cancelled => self.finish_or_idle(&ctx),
                    Selection::Pending => {}
                }
            }
            Stage::Editing(editor) => {
                // The editor owns a clone of the configuration, so anything
                // changed in its settings window has to be taken back or the
                // next capture in daemon mode silently reverts it.
                if let Some(updated) = editor.take_config_change() {
                    // Re-grab the keys when the bindings change, rather than
                    // making the user restart to find out whether the one they
                    // just typed works. Replacing the old `Hotkeys` releases
                    // its grabs on drop, so the previous key stops being held.
                    let rebind = updated.daemon.hotkeys != self.config.daemon.hotkeys;
                    self.config = updated;
                    // Disjoint fields: `self.stage` is already borrowed by the
                    // match above, so this takes the two fields it needs
                    // rather than `&mut self`.
                    if rebind {
                        if let Some(daemon) = self.daemon.as_mut() {
                            rebind_hotkeys(daemon, &self.config);
                        }
                    }
                }
                if pending == Some(Trigger::Quit) {
                    self.stage = Stage::Done;
                    self.finished = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                editor.draw(ui);
                if editor.outcome() == Outcome::Quit {
                    self.finish_or_idle(&ctx);
                }
                self.report_warnings();
            }
            Stage::Settings(window) => {
                // Closing the window is how you leave this stage; there is
                // nothing else on screen to interact with.
                let still_open = {
                    window.show(&ctx, &mut self.config, &self.history);
                    window.open
                };
                if !still_open {
                    self.finish_or_idle(&ctx);
                }
            }
            Stage::Done => self.finished = true,
        }

        if self.finished {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Re-register the global hotkeys after the user edited them.
///
/// Takes the two fields it needs rather than `&mut self`, because the caller is
/// inside a `match &mut self.stage` and the borrow checker rightly refuses a
/// second whole-`self` borrow.
///
/// Reports the outcome the way startup does: "did that key take?" is exactly as
/// invisible here as it is at launch, and the user has just typed something
/// another application may well already hold.
fn rebind_hotkeys(daemon: &mut Daemon, config: &Config) {
    let hotkeys = Hotkeys::register(&config.daemon);
    log::info!("rebound hotkeys: {}", hotkeys.summary());

    let (_, body) = hotkeys.announcement();
    let failures = hotkeys.failures().to_vec();
    let title = if failures.is_empty() {
        "Hotkeys updated"
    } else {
        "A hotkey could not be registered"
    };

    // Assigning drops the previous set, and its `Drop` releases the old grabs —
    // otherwise the key the user just replaced would stay held for the rest of
    // the session and reach nothing.
    daemon.hotkeys = hotkeys;
    daemon.warnings = failures;
    daemon.warned = false;

    crate::notify::notify(config, title, &body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::config::DaemonConfig;

    fn image() -> RgbaImage {
        RgbaImage::from_pixel(64, 48, image::Rgba([1, 2, 3, 255]))
    }

    /// A daemon config that can be built headlessly: no tray, no hotkeys.
    fn inert_daemon() -> Config {
        Config {
            daemon: DaemonConfig {
                enabled: true,
                tray: false,
                hotkeys: Vec::new(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn settings_from_the_tray_do_not_invent_a_blank_screenshot() {
        // Showing a fake dark rectangle as though it were a capture would be a
        // confusing thing to put in front of someone who asked for settings.
        // This calls the real transition; the previous version of this test
        // built a `Stage::Settings` by hand and asserted it was one, which
        // could not have caught anything.
        let mut app = BettershotApp::editing(Config::default(), image());
        let ctx = egui::Context::default();
        app.show_settings(&ctx);

        assert!(
            matches!(app.stage, Stage::Settings(_)),
            "the tray's Settings entry must not open an editor over a fake capture"
        );
    }

    #[test]
    fn the_capture_history_outlives_any_one_editor() {
        // Daemon mode rebuilds the editor per capture, so history kept inside
        // the editor would be lost between shots.
        let app = BettershotApp::editing(Config::default(), image());
        app.history.borrow_mut().push(crate::history::Entry {
            png: vec![1, 2, 3],
            width: 10,
            height: 10,
            label: "earlier capture".to_owned(),
        });
        assert_eq!(app.history.borrow().len(), 1);

        // Swapping in a new editor keeps it.
        let same = std::rc::Rc::clone(&app.history);
        drop(app);
        assert_eq!(same.borrow().len(), 1);
    }

    #[test]
    fn a_file_capture_starts_in_the_editor_and_is_not_resident() {
        let app = BettershotApp::editing(Config::default(), image());
        assert!(!app.starts_hidden());
        assert!(!app.starts_fullscreen());
        assert!(app.daemon.is_none());
    }

    #[test]
    fn the_selection_overlay_starts_fullscreen_and_visible() {
        let overlay = Overlay::new(
            image(),
            &[],
            &[],
            bettershot_core::math::Vec2D::ZERO,
            CaptureMode::Region,
            true,
        );
        let app = BettershotApp::selecting(Config::default(), overlay);
        assert!(app.starts_fullscreen());
        assert!(!app.starts_hidden());
    }

    #[test]
    fn a_daemon_with_no_way_to_be_triggered_refuses_to_start() {
        // It used to start anyway and sit invisible forever, polling every
        // 80 ms with no window, no tray and no way to quit — which is the
        // *normal* outcome of `--daemon` on Wayland without the tray feature,
        // because global hotkey registration always fails there.
        let error = BettershotApp::idle(inert_daemon())
            .err()
            .expect("a daemon nothing can drive must not start");
        assert!(error.contains("invisibly"), "{error}");
        assert!(
            error.contains("--capture region"),
            "the error should say what to do instead: {error}"
        );
    }

    #[test]
    fn a_finished_capture_is_routed_by_whether_it_needs_selection() {
        // `finish_capture` is the join point for the worker thread, so it is
        // what actually decides the next stage.
        let ctx = egui::Context::default();

        let mut app = BettershotApp::editing(Config::default(), image());
        app.finish_capture(
            &ctx,
            Ok(Acquired {
                image: image(),
                monitors: Vec::new(),
                windows: Vec::new(),
                origin: bettershot_core::math::Vec2D::ZERO,
                needs_selection: true,
                mode: CaptureMode::Region,
            }),
        );
        assert!(matches!(app.stage, Stage::Selecting(_)));

        let mut app = BettershotApp::editing(Config::default(), image());
        app.finish_capture(
            &ctx,
            Ok(Acquired {
                image: image(),
                monitors: Vec::new(),
                windows: Vec::new(),
                origin: bettershot_core::math::Vec2D::ZERO,
                needs_selection: false,
                mode: CaptureMode::Monitor,
            }),
        );
        assert!(matches!(app.stage, Stage::Editing(_)));
    }

    #[test]
    fn a_failed_capture_does_not_leave_the_app_stuck_capturing() {
        // A one-shot run exits; a daemon returns to waiting. Either way it
        // must not sit in `Capturing` forever.
        let ctx = egui::Context::default();
        let mut app = BettershotApp::editing(Config::default(), image());
        app.finish_capture(&ctx, Err("no backend".to_owned()));
        assert!(matches!(app.stage, Stage::Done));
        assert!(app.finished);
    }

    #[test]
    fn a_capture_worker_that_dies_without_a_result_is_not_waited_on_forever() {
        // If the worker panics the channel disconnects; polling must notice
        // rather than repainting every 80 ms until the heat death of the
        // universe.
        let (tx, rx) = std::sync::mpsc::channel::<Result<Acquired, String>>();
        drop(tx);
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn a_capture_that_needs_no_selection_goes_straight_to_the_editor() {
        let acquired = Acquired {
            image: image(),
            monitors: Vec::new(),
            windows: Vec::new(),
            origin: bettershot_core::math::Vec2D::ZERO,
            needs_selection: false,
            mode: CaptureMode::Monitor,
        };
        let app = BettershotApp::stage_for(&Config::default(), acquired);
        assert!(matches!(app.stage, Stage::Editing(_)));
        assert!(!app.starts_fullscreen());
    }

    #[test]
    fn a_region_capture_goes_to_the_overlay_first() {
        let acquired = Acquired {
            image: image(),
            monitors: Vec::new(),
            windows: Vec::new(),
            origin: bettershot_core::math::Vec2D::ZERO,
            needs_selection: true,
            mode: CaptureMode::Region,
        };
        let app = BettershotApp::stage_for(&Config::default(), acquired);
        assert!(matches!(app.stage, Stage::Selecting(_)));
        assert!(app.starts_fullscreen());
    }

    #[test]
    fn the_editor_window_is_sized_to_the_image() {
        let app = BettershotApp::editing(Config::default(), image());
        let size = app.initial_size();
        assert!(size.x >= 640.0 && size.y >= 640.0, "{size:?}");
    }
}
