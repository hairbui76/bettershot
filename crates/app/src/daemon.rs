//! Staying resident: global hotkeys and the system tray.
//!
//! In daemon mode bettershot keeps running after a capture instead of exiting,
//! so a hotkey or a tray click can start the next one without paying process
//! startup again.
//!
//! # Global hotkeys are a privilege, not a right
//!
//! Registering one can fail, and on Wayland it *always* fails: the protocol
//! deliberately refuses to let an application grab a key for itself, because a
//! program that could would also be a keylogger. That is a supported
//! configuration, not an error — the user binds their compositor to
//! `bettershot --capture region` instead (see `docs/platform-setup.md`). So a
//! failed registration is recorded and reported once, never treated as fatal.
//!
//! # The tray is behind a feature
//!
//! `tray-icon` needs GTK 3 and libayatana-appindicator development packages on
//! Linux. Requiring those to build a screenshot tool that does not otherwise
//! link GTK would be a poor trade, so the tray lives behind the `tray` feature.
//! It is enabled by default on Windows and macOS, where it costs nothing, and
//! is opt-in on Linux.

use bettershot_core::config::{CaptureMode, DaemonConfig};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::collections::HashMap;
use std::str::FromStr;

/// What the resident process has been asked to do.
///
/// `Settings` and `Quit` can only originate from the tray menu, so in a build
/// without the `tray` feature nothing constructs them.
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Start a capture in this mode.
    Capture(CaptureMode),
    /// Open the settings window.
    Settings,
    /// Leave daemon mode and exit.
    Quit,
}

/// Registered global hotkeys.
///
/// The manager must outlive the registrations, so it is kept even when every
/// individual binding failed.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    /// Hotkey id to the mode it captures.
    bindings: HashMap<u32, CaptureMode>,
    /// The registered keys, kept so they can be released explicitly.
    registered: Vec<HotKey>,
    /// Human-readable reasons individual bindings did not take, so the editor
    /// can tell the user once rather than silently doing nothing.
    failures: Vec<String>,
}

impl Hotkeys {
    /// Register every configured binding, keeping whichever succeed.
    pub fn register(config: &DaemonConfig) -> Self {
        let mut bindings = HashMap::new();
        let mut failures = Vec::new();

        let mut registered = Vec::new();

        if config.hotkeys.is_empty() {
            return Self {
                manager: None,
                bindings,
                registered,
                failures,
            };
        }

        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(e) => {
                // The usual cause is Wayland, where no application may grab a
                // global key.
                failures.push(format!(
                    "global hotkeys are unavailable on this session ({e}); \
                     bind your compositor to `bettershot --capture region` instead"
                ));
                return Self {
                    manager: None,
                    bindings,
                    registered,
                    failures,
                };
            }
        };

        for binding in &config.hotkeys {
            let hotkey = match HotKey::from_str(binding.key.trim()) {
                Ok(hotkey) => hotkey,
                Err(e) => {
                    failures.push(format!("`{}` is not a valid hotkey: {e}", binding.key));
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => {
                    bindings.insert(hotkey.id(), binding.mode);
                    registered.push(hotkey);
                }
                // Almost always "another application already has this key".
                Err(e) => failures.push(format!("could not register `{}`: {e}", binding.key)),
            }
        }

        Self {
            manager: Some(manager),
            bindings,
            registered,
            failures,
        }
    }

    /// True when at least one hotkey is live.
    pub fn is_active(&self) -> bool {
        !self.bindings.is_empty()
    }

    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// How many bindings actually took effect.
    pub fn registered_count(&self) -> usize {
        self.bindings.len()
    }

    /// A one-line summary for the log, so a user running with `-v` can see
    /// whether their hotkeys were accepted.
    pub fn summary(&self) -> String {
        match (self.registered_count(), self.failures.len()) {
            (0, 0) => "no hotkeys configured".to_owned(),
            (n, 0) => format!("{n} global hotkey(s) registered"),
            (0, f) => format!("no global hotkeys available ({f} failed)"),
            (n, f) => format!("{n} global hotkey(s) registered, {f} failed"),
        }
    }

    /// Drain pending hotkey events, returning the first capture to start.
    ///
    /// Only key-down is acted on; acting on both edges would fire twice per
    /// press. Any remaining events are drained so a burst cannot queue up a
    /// backlog of captures.
    pub fn poll(&self) -> Option<CaptureMode> {
        if self.bindings.is_empty() {
            return None;
        }
        let mut triggered = None;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if triggered.is_none() {
                triggered = self.bindings.get(&event.id).copied();
            }
        }
        triggered
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        // A grab left behind would stop that key reaching anything else for the
        // rest of the session, so release them explicitly rather than trusting
        // the manager's own teardown.
        if let (Some(manager), false) = (&self.manager, self.registered.is_empty()) {
            if let Err(e) = manager.unregister_all(&self.registered) {
                log::warn!("could not release global hotkeys: {e}");
            }
        }
    }
}

/// Describes the tray menu, independent of the toolkit that renders it.
///
/// Keeping the menu's shape here means the mapping from menu entry to action
/// stays testable on a machine where `tray-icon` itself cannot be built — which
/// is every Linux box without the GTK 3 development packages.
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
pub fn tray_menu_items(config: &DaemonConfig) -> Vec<(String, Trigger)> {
    let mut items = vec![
        (
            "Capture region".to_owned(),
            Trigger::Capture(CaptureMode::Region),
        ),
        (
            "Capture window".to_owned(),
            Trigger::Capture(CaptureMode::Window),
        ),
        (
            "Capture monitor".to_owned(),
            Trigger::Capture(CaptureMode::Monitor),
        ),
        (
            "Capture everything".to_owned(),
            Trigger::Capture(CaptureMode::All),
        ),
        ("Settings…".to_owned(), Trigger::Settings),
        ("Quit bettershot".to_owned(), Trigger::Quit),
    ];
    if !config.tray {
        items.clear();
    }
    items
}

#[cfg(feature = "tray")]
mod tray {
    use super::{Trigger, tray_menu_items};
    use bettershot_core::config::DaemonConfig;
    use std::collections::HashMap;
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder};

    /// A live tray icon and the mapping from its menu entries to actions.
    pub struct Tray {
        /// Held so the icon stays on screen; dropping it removes the icon.
        _icon: TrayIcon,
        actions: HashMap<MenuId, Trigger>,
    }

    impl Tray {
        pub fn new(config: &DaemonConfig) -> Result<Self, String> {
            let menu = Menu::new();
            let mut actions = HashMap::new();

            for (label, trigger) in tray_menu_items(config) {
                let item = MenuItem::new(&label, true, None);
                actions.insert(item.id().clone(), trigger);
                menu.append(&item).map_err(|e| e.to_string())?;
            }

            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("bettershot")
                .with_icon(icon()?)
                .build()
                .map_err(|e| e.to_string())?;

            Ok(Self {
                _icon: icon,
                actions,
            })
        }

        /// Drain pending menu events, returning the first recognised action.
        pub fn poll(&self) -> Option<Trigger> {
            let mut triggered = None;
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if triggered.is_none() {
                    triggered = self.actions.get(&event.id).copied();
                }
            }
            triggered
        }
    }

    /// A simple camera-shutter glyph, generated rather than shipped as a file
    /// so the binary stays self-contained.
    fn icon() -> Result<tray_icon::Icon, String> {
        const SIZE: u32 = 32;
        let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let centre = (SIZE as f32 - 1.0) / 2.0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (dx, dy) = (x as f32 - centre, y as f32 - centre);
                let distance = (dx * dx + dy * dy).sqrt();
                // A filled ring: opaque body, transparent outside and centre.
                let inside = distance <= 13.0;
                let lens = distance <= 6.0;
                let (r, g, b, a) = match (inside, lens) {
                    (true, true) => (30, 30, 34, 255),
                    (true, false) => (235, 235, 240, 255),
                    _ => (0, 0, 0, 0),
                };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| e.to_string())
    }
}

#[cfg(feature = "tray")]
pub use tray::Tray;

/// Stand-in used when the `tray` feature is off, so the rest of the app does
/// not need `cfg` branches at every call site.
#[cfg(not(feature = "tray"))]
pub struct Tray;

#[cfg(not(feature = "tray"))]
impl Tray {
    pub fn new(_config: &DaemonConfig) -> Result<Self, String> {
        Err("this build was compiled without the `tray` feature".to_owned())
    }

    pub fn poll(&self) -> Option<Trigger> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::config::HotkeyBinding;

    #[test]
    fn the_tray_menu_offers_every_capture_mode_plus_settings_and_quit() {
        let config = DaemonConfig::default();
        let items = tray_menu_items(&config);

        for mode in CaptureMode::ALL {
            assert!(
                items.iter().any(|(_, t)| *t == Trigger::Capture(mode)),
                "no tray entry captures {mode}"
            );
        }
        assert!(items.iter().any(|(_, t)| *t == Trigger::Settings));
        assert!(items.iter().any(|(_, t)| *t == Trigger::Quit));
        assert!(items.iter().all(|(label, _)| !label.is_empty()));
    }

    #[test]
    fn disabling_the_tray_produces_no_menu() {
        let config = DaemonConfig {
            tray: false,
            ..Default::default()
        };
        assert!(tray_menu_items(&config).is_empty());
    }

    #[test]
    fn registering_no_hotkeys_is_not_an_error() {
        let config = DaemonConfig {
            hotkeys: Vec::new(),
            ..Default::default()
        };
        let hotkeys = Hotkeys::register(&config);
        assert!(!hotkeys.is_active());
        assert!(hotkeys.failures().is_empty(), "nothing was asked for");
        assert_eq!(hotkeys.poll(), None);
        assert_eq!(hotkeys.summary(), "no hotkeys configured");
    }

    #[test]
    fn an_unavailable_or_invalid_binding_is_reported_rather_than_fatal() {
        // On this headless box no grab can succeed, and the second binding is
        // nonsense regardless of platform. Either way `register` must return.
        let config = DaemonConfig {
            hotkeys: vec![
                HotkeyBinding::new("Ctrl+Shift+F13", CaptureMode::Region),
                HotkeyBinding::new("NotAKey+++", CaptureMode::Window),
            ],
            ..Default::default()
        };
        let hotkeys = Hotkeys::register(&config);
        assert!(
            !hotkeys.failures().is_empty(),
            "an unusable binding should be explained"
        );
        // And polling a dead registration is harmless.
        assert_eq!(hotkeys.poll(), None);
        assert_eq!(hotkeys.registered_count(), hotkeys.bindings.len());
        assert!(!hotkeys.summary().is_empty());
    }

    #[test]
    fn the_tray_stub_reports_the_missing_feature_when_disabled() {
        // Only meaningful in a build without the feature, which is the default
        // on Linux.
        #[cfg(not(feature = "tray"))]
        {
            match Tray::new(&DaemonConfig::default()) {
                Ok(_) => panic!("a build without the feature must not make a tray"),
                Err(e) => assert!(e.contains("tray"), "{e}"),
            }
        }
    }
}
