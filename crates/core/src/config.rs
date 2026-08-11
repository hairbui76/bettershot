//! User configuration.
//!
//! Precedence is **CLI argument > config file > built-in default**, the same
//! rule Satty uses, so an existing Satty config translates almost key for key.
//! The schema lives in core (rather than next to the argument parser) because
//! both the CLI and the editor need it, and because it is worth testing without
//! either.
//!
//! Every field is optional in the file: a config that sets one key must not
//! have to restate the rest. That is what [`ConfigFile`] is for — it is the
//! partial, all-`Option` mirror of [`Config`], and [`Config::merge_file`]
//! folds it in.

use serde::{Deserialize, Serialize};

use crate::style::{Color, Size};
use crate::tools::{ObscureKind, Tools};

/// What Enter or Escape should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Copy the annotated image to the clipboard.
    SaveToClipboard,
    /// Write it to the configured output file.
    SaveToFile,
    /// Open the system save dialog.
    SaveAs,
    /// Quit without saving.
    Exit,
    /// Do nothing.
    None,
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::SaveToClipboard => "save-to-clipboard",
            Action::SaveToFile => "save-to-file",
            Action::SaveAs => "save-as",
            Action::Exit => "exit",
            Action::None => "none",
        }
    }

    pub const ALL: [Action; 5] = [
        Action::SaveToClipboard,
        Action::SaveToFile,
        Action::SaveAs,
        Action::Exit,
        Action::None,
    ];

    /// Whether performing this action produces output worth exiting after.
    pub fn is_save(&self) -> bool {
        matches!(
            self,
            Action::SaveToClipboard | Action::SaveToFile | Action::SaveAs
        )
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "unknown action `{0}`, expected one of: save-to-clipboard, save-to-file, save-as, exit, none"
)]
pub struct ActionParseError(String);

impl std::str::FromStr for Action {
    type Err = ActionParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        Action::ALL
            .into_iter()
            .find(|a| a.name() == needle)
            .ok_or_else(|| ActionParseError(s.to_owned()))
    }
}

/// Image format for saved files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl SaveFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            SaveFormat::Png => "png",
            SaveFormat::Jpeg => "jpg",
            SaveFormat::Webp => "webp",
        }
    }

    /// Infer the format from a filename extension, falling back to PNG.
    pub fn from_path(path: &str) -> SaveFormat {
        match path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg" | "jpeg") => SaveFormat::Jpeg,
            Some("webp") => SaveFormat::Webp,
            _ => SaveFormat::Png,
        }
    }

    /// Whether this format can store an alpha channel. Cropping or annotating
    /// never introduces transparency today, but the region overlay can, and
    /// silently flattening it would surprise people.
    pub fn supports_alpha(&self) -> bool {
        matches!(self, SaveFormat::Png | SaveFormat::Webp)
    }
}

/// Which colour scheme the editor uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// Follow the desktop's light/dark setting.
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn name(&self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown theme `{0}`, expected one of: system, light, dark")]
pub struct ThemeParseError(String);

impl std::str::FromStr for Theme {
    type Err = ThemeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        Theme::ALL
            .into_iter()
            .find(|t| t.name() == needle)
            .ok_or_else(|| ThemeParseError(s.to_owned()))
    }
}

/// How the region-selection overlay is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    /// Drag out a region.
    #[default]
    Region,
    /// Pick a window.
    Window,
    /// Grab the monitor under the pointer.
    Monitor,
    /// Grab every monitor, stitched together.
    All,
}

impl CaptureMode {
    pub const ALL: [CaptureMode; 4] = [
        CaptureMode::Region,
        CaptureMode::Window,
        CaptureMode::Monitor,
        CaptureMode::All,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            CaptureMode::Region => "region",
            CaptureMode::Window => "window",
            CaptureMode::Monitor => "monitor",
            CaptureMode::All => "all",
        }
    }
}

impl std::fmt::Display for CaptureMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown capture mode `{0}`, expected one of: region, window, monitor, all")]
pub struct CaptureModeParseError(String);

impl std::str::FromStr for CaptureMode {
    type Err = CaptureModeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        CaptureMode::ALL
            .into_iter()
            .find(|m| m.name() == needle)
            .ok_or_else(|| CaptureModeParseError(s.to_owned()))
    }
}

/// The schema version written into every config file.
///
/// It exists so that a future release which has to change the meaning of a key
/// can migrate old files instead of silently misreading them, and so that an
/// older bettershot refuses a file from a newer one rather than dropping the
/// keys it does not recognise.
pub const CONFIG_VERSION: u32 = 1;

/// One global-hotkey binding: a key combination and what it captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HotkeyBinding {
    /// A combination such as `Ctrl+Shift+PrintScreen`. Parsed by the platform
    /// layer, because what counts as a valid key name is platform-specific.
    pub key: String,
    pub mode: CaptureMode,
}

impl HotkeyBinding {
    pub fn new(key: impl Into<String>, mode: CaptureMode) -> Self {
        Self {
            key: key.into(),
            mode,
        }
    }
}

/// Staying resident so a hotkey or tray icon can trigger a capture.
///
/// Global hotkeys are a **platform privilege**, not a given: Wayland
/// deliberately refuses to let an application grab one for itself, so on those
/// sessions registration fails and the user binds their compositor instead.
/// That is why a failed registration is a warning, never a fatal error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DaemonConfig {
    /// Stay running after the first capture.
    #[serde(default)]
    pub enabled: bool,
    /// Show a system tray icon while resident.
    #[serde(default = "default_true")]
    pub tray: bool,
    #[serde(default = "default_hotkeys")]
    pub hotkeys: Vec<HotkeyBinding>,
}

fn default_hotkeys() -> Vec<HotkeyBinding> {
    vec![
        HotkeyBinding::new("PrintScreen", CaptureMode::Region),
        HotkeyBinding::new("Shift+PrintScreen", CaptureMode::Window),
        HotkeyBinding::new("Ctrl+PrintScreen", CaptureMode::Monitor),
    ]
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tray: true,
            hotkeys: default_hotkeys(),
        }
    }
}

impl DaemonConfig {
    /// Reject bindings that could never work, before the platform layer tries
    /// to register them.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen: Vec<&str> = Vec::new();
        for binding in &self.hotkeys {
            let key = binding.key.trim();
            if key.is_empty() {
                return Err(ConfigError::Invalid(
                    "a hotkey binding has an empty key".into(),
                ));
            }
            if seen.contains(&key) {
                return Err(ConfigError::Invalid(format!(
                    "hotkey `{key}` is bound more than once"
                )));
            }
            seen.push(key);
        }
        Ok(())
    }
}

/// The colours offered in the bottom toolbar, in order. Digits 1-9 and 0 pick
/// the first ten.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColorPalette {
    pub palette: Vec<Color>,
    /// Custom colours the user has mixed, appended after the palette.
    #[serde(default)]
    pub custom: Vec<Color>,
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self {
            palette: vec![
                Color::red(),
                Color::green(),
                Color::blue(),
                Color::orange(),
                Color::pink(),
                Color::cove(),
                Color::white(),
                Color::black(),
            ],
            custom: Vec::new(),
        }
    }
}

/// Accepts either a bare array of colours — `color-palette = ["#f00", "#0f0"]`
/// — or the fuller table form with a separate list of user-mixed colours:
///
/// ```toml
/// [color-palette]
/// palette = ["#f00", "#0f0"]
/// custom  = ["#123456"]
/// ```
impl<'de> Deserialize<'de> for ColorPalette {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct Table {
            palette: Vec<Color>,
            #[serde(default)]
            custom: Vec<Color>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Spec {
            List(Vec<Color>),
            Table(Table),
        }

        Ok(match Spec::deserialize(deserializer)? {
            Spec::List(palette) => ColorPalette {
                palette,
                custom: Vec::new(),
            },
            Spec::Table(t) => ColorPalette {
                palette: t.palette,
                custom: t.custom,
            },
        })
    }
}

impl ColorPalette {
    /// Every selectable colour, palette first then custom.
    pub fn all(&self) -> Vec<Color> {
        self.palette
            .iter()
            .chain(self.custom.iter())
            .copied()
            .collect()
    }

    /// The colour for a 1-based palette shortcut (`1`..`9`, `0` = 10th).
    /// Out-of-range indices return `None`, which the editor treats as "open
    /// the custom colour picker", matching Satty.
    pub fn nth(&self, index: usize) -> Option<Color> {
        self.all().get(index).copied()
    }

    pub fn first(&self) -> Color {
        self.palette.first().copied().unwrap_or_else(Color::red)
    }
}

/// Font used by the text tool and numbered markers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FontConfig {
    /// Family name, resolved through the platform's font system.
    pub family: Option<String>,
    /// Explicit font file, which wins over `family`. Useful on systems with no
    /// working fontconfig, and in tests.
    pub path: Option<String>,
}

/// Capture-specific settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CaptureConfig {
    #[serde(default)]
    pub mode: CaptureMode,
    /// Seconds to wait before grabbing, for capturing menus and hover states.
    #[serde(default)]
    pub delay_seconds: u64,
    /// Include the mouse cursor in the capture.
    ///
    /// **Not implemented.** No backend consumes this yet: the Wayland portal
    /// decides for itself, and the X11 and Windows paths would each need their
    /// own compositing step. The key is accepted so configs do not break when
    /// it lands, and the UI marks it as unavailable rather than offering a
    /// control that silently does nothing.
    #[serde(default)]
    pub include_cursor: bool,
    /// Highlight and snap to window bounds while dragging a region.
    #[serde(default = "default_true")]
    pub snap_to_windows: bool,
}

fn default_true() -> bool {
    true
}

/// The fully-resolved configuration the editor runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    // --- editor -----------------------------------------------------------
    pub initial_tool: Tools,
    pub initial_color: Option<Color>,
    pub initial_size: Size,
    pub annotation_size_factor: f32,
    pub default_fill_shapes: bool,
    pub default_round_caps: bool,
    pub obscure: ObscureKind,
    pub color_palette: ColorPalette,
    pub font: FontConfig,

    // --- window -----------------------------------------------------------
    pub fullscreen: bool,
    pub hide_toolbars: bool,
    pub no_window_decoration: bool,
    pub theme: Theme,
    /// UI language code, or "system" to follow the desktop. Only English ships
    /// today; see `crates/app/src/i18n.rs`.
    pub language: String,

    // --- output -----------------------------------------------------------
    /// Output path, which may contain strftime placeholders.
    pub output_filename: Option<String>,
    pub save_format: SaveFormat,
    /// Shell command receiving the image on stdin, instead of the built-in
    /// clipboard (e.g. `wl-copy`). Needed where no clipboard API works.
    pub copy_command: Option<String>,
    pub save_after_copy: bool,
    pub disable_notifications: bool,
    /// Write a local crash report on panic. Off by default, and never sent
    /// anywhere; see `crates/app/src/crash.rs` for what a report may contain.
    pub crash_reports: bool,
    /// How many recent captures to keep in memory for re-copying. Zero
    /// disables the history entirely.
    pub history_size: usize,

    // --- behaviour --------------------------------------------------------
    pub action_on_enter: Action,
    pub action_on_escape: Action,
    /// Quit immediately after a save action succeeds.
    pub early_exit: bool,

    // --- capture ----------------------------------------------------------
    pub capture: CaptureConfig,
    pub daemon: DaemonConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            initial_tool: Tools::default(),
            initial_color: None,
            initial_size: Size::default(),
            annotation_size_factor: 1.0,
            default_fill_shapes: false,
            default_round_caps: true,
            obscure: ObscureKind::default(),
            color_palette: ColorPalette::default(),
            font: FontConfig::default(),

            fullscreen: false,
            hide_toolbars: false,
            no_window_decoration: false,
            theme: Theme::default(),
            language: "system".to_owned(),

            output_filename: None,
            save_format: SaveFormat::default(),
            copy_command: None,
            save_after_copy: false,
            disable_notifications: false,
            crash_reports: false,
            history_size: 5,

            action_on_enter: Action::SaveToClipboard,
            action_on_escape: Action::Exit,
            early_exit: false,

            capture: CaptureConfig {
                snap_to_windows: true,
                ..Default::default()
            },
            daemon: DaemonConfig::default(),
        }
    }
}

impl Config {
    /// The style a freshly created tool starts with.
    pub fn initial_style(&self) -> crate::style::Style {
        crate::style::Style {
            color: self
                .initial_color
                .unwrap_or_else(|| self.color_palette.first()),
            size: self.initial_size,
            fill: self.default_fill_shapes,
            round_caps: self.default_round_caps,
            annotation_size_factor: self.annotation_size_factor,
        }
    }

    /// Fold a parsed config file over the defaults. Absent keys keep whatever
    /// value they already had, so this is also how the CLI layer is applied.
    pub fn merge_file(&mut self, file: ConfigFile) {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(v) = file.$field { self.$field = v; })*
            };
        }
        set!(
            initial_tool,
            initial_size,
            annotation_size_factor,
            default_fill_shapes,
            default_round_caps,
            obscure,
            font,
            fullscreen,
            hide_toolbars,
            no_window_decoration,
            theme,
            save_format,
            save_after_copy,
            disable_notifications,
            crash_reports,
            history_size,
            action_on_enter,
            action_on_escape,
            early_exit,
        );
        // These are Option-valued in Config itself, so `Some(None)` has to be
        // distinguishable from "not mentioned".
        if let Some(v) = file.initial_color {
            self.initial_color = Some(v);
        }
        if let Some(v) = file.output_filename {
            self.output_filename = Some(v);
        }
        if let Some(v) = file.copy_command {
            self.copy_command = Some(v);
        }
        if let Some(v) = file.language {
            self.language = v;
        }
        if let Some(v) = file.color_palette {
            self.color_palette = v;
        }
        if let Some(v) = file.capture {
            self.capture = v;
        }
        if let Some(v) = file.daemon {
            self.daemon = v;
        }
    }

    /// Parse a TOML document, migrate it if needed, and merge it in.
    pub fn merge_toml(&mut self, source: &str) -> Result<(), ConfigError> {
        let file: ConfigFile =
            toml::from_str(source).map_err(|e| ConfigError::Parse(e.to_string()))?;
        self.merge_file(migrate(file)?);
        Ok(())
    }

    /// Project back to the on-disk shape, so a settings UI can write the
    /// current state out as a config file.
    ///
    /// Every key is emitted explicitly rather than only the ones that differ
    /// from the defaults: a written config should be a complete, readable
    /// record of what the program is doing, and should not silently change
    /// meaning if a default is revised later.
    pub fn to_file(&self) -> ConfigFile {
        ConfigFile {
            version: Some(CONFIG_VERSION),
            initial_tool: Some(self.initial_tool),
            initial_color: self.initial_color,
            initial_size: Some(self.initial_size),
            annotation_size_factor: Some(self.annotation_size_factor),
            default_fill_shapes: Some(self.default_fill_shapes),
            default_round_caps: Some(self.default_round_caps),
            obscure: Some(self.obscure),
            color_palette: Some(self.color_palette.clone()),
            font: Some(self.font.clone()),

            fullscreen: Some(self.fullscreen),
            hide_toolbars: Some(self.hide_toolbars),
            no_window_decoration: Some(self.no_window_decoration),
            theme: Some(self.theme),
            language: Some(self.language.clone()),

            output_filename: self.output_filename.clone(),
            save_format: Some(self.save_format),
            copy_command: self.copy_command.clone(),
            save_after_copy: Some(self.save_after_copy),
            disable_notifications: Some(self.disable_notifications),
            crash_reports: Some(self.crash_reports),
            history_size: Some(self.history_size),

            action_on_enter: Some(self.action_on_enter),
            action_on_escape: Some(self.action_on_escape),
            early_exit: Some(self.early_exit),

            capture: Some(self.capture.clone()),
            daemon: Some(self.daemon.clone()),
        }
    }

    /// Render the current settings as a TOML document.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(&self.to_file()).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Reject values that would break rendering rather than failing later in
    /// an obscure way.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // The upper bound is not fussiness. Text size scales with this, and a
        // glyph bitmap costs memory proportional to its area, so a factor in
        // the thousands asks the rasterizer for gigabytes. The lower bound
        // matters just as much: below about 0.05 a stroke is thinner than the
        // rasterizer's sub-pixel grid and annotations render as nothing at all,
        // silently, while still sitting in the undo stack.
        const MIN_SIZE_FACTOR: f32 = 0.05;
        const MAX_SIZE_FACTOR: f32 = 20.0;
        if !self.annotation_size_factor.is_finite()
            || self.annotation_size_factor < MIN_SIZE_FACTOR
            || self.annotation_size_factor > MAX_SIZE_FACTOR
        {
            return Err(ConfigError::Invalid(format!(
                "annotation-size-factor must be between {MIN_SIZE_FACTOR} and \
                 {MAX_SIZE_FACTOR}, got {}",
                self.annotation_size_factor
            )));
        }
        if self.color_palette.palette.is_empty() && self.color_palette.custom.is_empty() {
            return Err(ConfigError::Invalid(
                "the colour palette must contain at least one colour".into(),
            ));
        }
        self.daemon.validate()?;
        Ok(())
    }
}

/// Bring a parsed config file up to the current schema.
///
/// There is only one version so far, so the only thing this can do today is
/// refuse a file from the future. That refusal is the point: silently ignoring
/// keys a newer release introduced would give the user a program that does not
/// do what their config says.
pub fn migrate(file: ConfigFile) -> Result<ConfigFile, ConfigError> {
    let version = file.version.unwrap_or(CONFIG_VERSION);
    if version > CONFIG_VERSION {
        return Err(ConfigError::Invalid(format!(
            "this configuration is version {version}, but this bettershot only \
             understands up to version {CONFIG_VERSION}; upgrade bettershot or \
             remove the `version` key to read it anyway"
        )));
    }
    // Version 1 is current: nothing to transform yet. Future migrations chain
    // from here, each stepping the version by one.
    Ok(file)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not parse configuration: {0}")]
    Parse(String),
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// The on-disk shape of the config file: every key optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ConfigFile {
    /// Schema version. Absent means "written before versioning existed",
    /// which is treated as version 1.
    pub version: Option<u32>,
    pub initial_tool: Option<Tools>,
    pub initial_color: Option<Color>,
    pub initial_size: Option<Size>,
    pub annotation_size_factor: Option<f32>,
    pub default_fill_shapes: Option<bool>,
    pub default_round_caps: Option<bool>,
    pub obscure: Option<ObscureKind>,
    pub color_palette: Option<ColorPalette>,
    pub font: Option<FontConfig>,

    pub fullscreen: Option<bool>,
    pub hide_toolbars: Option<bool>,
    pub no_window_decoration: Option<bool>,
    pub theme: Option<Theme>,
    pub language: Option<String>,

    pub output_filename: Option<String>,
    pub save_format: Option<SaveFormat>,
    pub copy_command: Option<String>,
    pub save_after_copy: Option<bool>,
    pub disable_notifications: Option<bool>,
    pub crash_reports: Option<bool>,
    pub history_size: Option<usize>,

    pub action_on_enter: Option<Action>,
    pub action_on_escape: Option<Action>,
    pub early_exit: Option<bool>,

    pub capture: Option<CaptureConfig>,
    pub daemon: Option<DaemonConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable_and_valid() {
        let config = Config::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.action_on_enter, Action::SaveToClipboard);
        assert_eq!(config.action_on_escape, Action::Exit);
        assert_eq!(config.initial_style().color, Color::red());
    }

    #[test]
    fn a_partial_config_only_overrides_what_it_mentions() {
        let mut config = Config::default();
        config
            .merge_toml("initial-tool = \"arrow\"\n")
            .expect("should parse");
        assert_eq!(config.initial_tool, Tools::Arrow);
        // Everything else is untouched.
        assert_eq!(config.action_on_escape, Action::Exit);
        assert_eq!(config.annotation_size_factor, 1.0);
    }

    #[test]
    fn parses_a_representative_config() {
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                # a comment
                initial-tool = "brush"
                initial-color = "#00ff00"
                annotation-size-factor = 1.5
                default-fill-shapes = true
                action-on-enter = "save-to-file"
                output-filename = "/tmp/shot.png"
                color-palette = ["#ff0000", "#00ff00", "#0000ff"]

                [capture]
                mode = "window"
                delay-seconds = 3
                snap-to-windows = false
                "##,
            )
            .expect("should parse");

        assert_eq!(config.initial_tool, Tools::Brush);
        assert_eq!(config.initial_color, Some(Color::rgb(0, 255, 0)));
        assert_eq!(config.annotation_size_factor, 1.5);
        assert!(config.default_fill_shapes);
        assert_eq!(config.action_on_enter, Action::SaveToFile);
        assert_eq!(config.output_filename.as_deref(), Some("/tmp/shot.png"));
        assert_eq!(config.color_palette.palette.len(), 3);
        assert_eq!(config.capture.mode, CaptureMode::Window);
        assert_eq!(config.capture.delay_seconds, 3);
        assert!(!config.capture.snap_to_windows);
    }

    #[test]
    fn the_initial_style_follows_the_configured_defaults() {
        let mut config = Config::default();
        config
            .merge_toml(
                "initial-color = \"#123456\"\ndefault-fill-shapes = true\nannotation-size-factor = 2.0\n",
            )
            .unwrap();
        let style = config.initial_style();
        assert_eq!(style.color, Color::rgb(0x12, 0x34, 0x56));
        assert!(style.fill);
        assert_eq!(style.annotation_size_factor, 2.0);
        assert_eq!(style.line_width(), Size::Medium.to_line_width(2.0));
    }

    #[test]
    fn an_unset_initial_colour_falls_back_to_the_first_palette_entry() {
        let mut config = Config::default();
        config.merge_toml("color-palette = [\"#abcdef\"]").unwrap();
        assert_eq!(config.initial_style().color, Color::rgb(0xab, 0xcd, 0xef));
    }

    #[test]
    fn unknown_keys_and_sections_are_rejected_loudly() {
        let mut config = Config::default();
        assert!(config.merge_toml("wobble = 3").is_err());
        assert!(config.merge_toml("[nonsense]\nx = 1").is_err());
    }

    #[test]
    fn malformed_values_are_rejected() {
        let mut config = Config::default();
        assert!(config.merge_toml("initial-tool = \"nonexistent\"").is_err());
        assert!(
            config
                .merge_toml("initial-color = \"not-a-colour\"")
                .is_err()
        );
        assert!(config.merge_toml("fullscreen = \"yes\"").is_err());
        assert!(config.merge_toml("this is not toml").is_err());
    }

    #[test]
    fn validation_rejects_a_nonsense_size_factor() {
        // Below the lower bound annotations render as literally nothing;
        // above the upper bound a single glyph can ask for gigabytes.
        for factor in [0.0, -1.0, f32::NAN, f32::INFINITY, 0.01, 0.03, 1000.0] {
            let config = Config {
                annotation_size_factor: factor,
                ..Default::default()
            };
            assert!(
                config.validate().is_err(),
                "a size factor of {factor} should be rejected"
            );
        }
    }

    #[test]
    fn validation_rejects_an_empty_palette() {
        let config = Config {
            color_palette: ColorPalette {
                palette: Vec::new(),
                custom: Vec::new(),
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn palette_shortcuts_index_from_zero_and_run_out_gracefully() {
        let palette = ColorPalette {
            palette: vec![Color::red(), Color::green()],
            custom: vec![Color::blue()],
        };
        assert_eq!(palette.nth(0), Some(Color::red()));
        assert_eq!(
            palette.nth(2),
            Some(Color::blue()),
            "custom follows palette"
        );
        assert_eq!(palette.nth(9), None, "out of range opens the picker");
        assert_eq!(palette.all().len(), 3);
    }

    #[test]
    fn the_theme_defaults_to_following_the_desktop() {
        let mut config = Config::default();
        assert_eq!(config.theme, Theme::System);
        config.merge_toml("theme = \"dark\"").unwrap();
        assert_eq!(config.theme, Theme::Dark);
        assert!(config.merge_toml("theme = \"neon\"").is_err());
    }

    #[test]
    fn actions_and_modes_round_trip_through_strings() {
        for theme in Theme::ALL {
            assert_eq!(theme.name().parse::<Theme>().unwrap(), theme);
        }
        for action in Action::ALL {
            assert_eq!(action.name().parse::<Action>().unwrap(), action);
        }
        for mode in CaptureMode::ALL {
            assert_eq!(mode.name().parse::<CaptureMode>().unwrap(), mode);
        }
        assert!("nope".parse::<Action>().is_err());
        assert!("nope".parse::<CaptureMode>().is_err());
    }

    #[test]
    fn save_format_is_inferred_from_the_extension() {
        assert_eq!(SaveFormat::from_path("a.png"), SaveFormat::Png);
        assert_eq!(SaveFormat::from_path("a.JPG"), SaveFormat::Jpeg);
        assert_eq!(SaveFormat::from_path("a.jpeg"), SaveFormat::Jpeg);
        assert_eq!(SaveFormat::from_path("a.webp"), SaveFormat::Webp);
        // Unknown or missing extensions default to PNG rather than failing.
        assert_eq!(SaveFormat::from_path("a.tiff"), SaveFormat::Png);
        assert_eq!(SaveFormat::from_path("noextension"), SaveFormat::Png);
        assert!(!SaveFormat::Jpeg.supports_alpha());
        assert!(SaveFormat::Png.supports_alpha());
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let mut config = Config::default();
        config
            .merge_toml("\n# leading comment\n\nfullscreen = true # trailing\n\n")
            .unwrap();
        assert!(config.fullscreen);
    }

    // --- schema versioning --------------------------------------------------

    #[test]
    fn a_usable_range_of_size_factors_is_accepted() {
        for factor in [0.05, 0.5, 1.0, 2.0, 4.0, 20.0] {
            let config = Config {
                annotation_size_factor: factor,
                ..Default::default()
            };
            assert!(config.validate().is_ok(), "{factor} should be allowed");
        }
    }

    #[test]
    fn the_capture_history_has_a_sensible_default_and_can_be_disabled() {
        assert_eq!(Config::default().history_size, 5);
        let mut config = Config::default();
        config.merge_toml("history-size = 0").unwrap();
        assert_eq!(config.history_size, 0, "zero must disable it");
    }

    #[test]
    fn crash_reporting_is_opt_in() {
        assert!(
            !Config::default().crash_reports,
            "a tool that holds screen contents must not report by default"
        );
        let mut config = Config::default();
        config.merge_toml("crash-reports = true").unwrap();
        assert!(config.crash_reports);
    }

    #[test]
    fn a_config_without_a_version_is_read_as_the_current_one() {
        let mut config = Config::default();
        config
            .merge_toml("initial-tool = \"arrow\"")
            .expect("should parse");
        assert_eq!(config.initial_tool, Tools::Arrow);
    }

    #[test]
    fn a_config_from_the_future_is_refused_with_an_actionable_message() {
        let mut config = Config::default();
        let err = config
            .merge_toml(&format!(
                "version = {}\ninitial-tool = \"arrow\"",
                CONFIG_VERSION + 1
            ))
            .unwrap_err()
            .to_string();
        assert!(err.contains("upgrade bettershot"), "{err}");
        assert_eq!(
            config.initial_tool,
            Tools::default(),
            "a refused config must not be half-applied"
        );
    }

    #[test]
    fn the_current_version_is_accepted() {
        let mut config = Config::default();
        config
            .merge_toml(&format!(
                "version = {CONFIG_VERSION}\ninitial-tool = \"brush\""
            ))
            .expect("the current version should be readable");
        assert_eq!(config.initial_tool, Tools::Brush);
    }

    #[test]
    fn what_we_write_carries_the_version_and_reads_back() {
        let written = Config::default().to_toml().unwrap();
        assert!(written.contains("version"), "{written}");
        let mut reloaded = Config::default();
        reloaded
            .merge_toml(&written)
            .expect("our own output must parse");
    }

    #[test]
    fn a_config_round_trips_through_toml() {
        let mut original = Config::default();
        original
            .merge_toml(
                "initial-tool = \"arrow\"\ntheme = \"dark\"\nannotation-size-factor = 1.75\noutput-filename = \"/tmp/x.png\"\n",
            )
            .unwrap();

        let written = original.to_toml().expect("should serialise");
        let mut reloaded = Config::default();
        reloaded
            .merge_toml(&written)
            .unwrap_or_else(|e| panic!("the config we wrote did not parse: {e}\n{written}"));

        assert_eq!(reloaded, original, "round trip changed the configuration");
    }

    #[test]
    fn a_written_config_records_every_key() {
        let written = Config::default().to_toml().unwrap();
        for key in [
            "initial-tool",
            "theme",
            "action-on-enter",
            "action-on-escape",
            "annotation-size-factor",
            "color-palette",
            "[capture]",
        ] {
            assert!(written.contains(key), "`{key}` missing from:\n{written}");
        }
    }

    // --- daemon -------------------------------------------------------------

    #[test]
    fn the_daemon_is_off_by_default_but_has_sensible_bindings_ready() {
        let config = Config::default();
        assert!(!config.daemon.enabled, "must be opt-in");
        assert!(config.daemon.tray);
        assert_eq!(config.daemon.hotkeys.len(), 3);
        assert_eq!(config.daemon.hotkeys[0].mode, CaptureMode::Region);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn daemon_settings_can_be_configured() {
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                [daemon]
                enabled = true
                tray = false
                hotkeys = [
                  { key = "Ctrl+Alt+S", mode = "region" },
                  { key = "Ctrl+Alt+W", mode = "window" },
                ]
                "##,
            )
            .expect("should parse");

        assert!(config.daemon.enabled);
        assert!(!config.daemon.tray);
        assert_eq!(config.daemon.hotkeys.len(), 2);
        assert_eq!(config.daemon.hotkeys[1].key, "Ctrl+Alt+W");
        assert_eq!(config.daemon.hotkeys[1].mode, CaptureMode::Window);
    }

    #[test]
    fn duplicate_or_empty_hotkeys_are_rejected() {
        let mut config = Config::default();
        config.daemon.hotkeys = vec![
            HotkeyBinding::new("Ctrl+A", CaptureMode::Region),
            HotkeyBinding::new("Ctrl+A", CaptureMode::Window),
        ];
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("Ctrl+A"), "{err}");

        config.daemon.hotkeys = vec![HotkeyBinding::new("  ", CaptureMode::Region)];
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_daemon_with_no_hotkeys_is_valid_since_the_tray_can_still_drive_it() {
        let mut config = Config::default();
        config.daemon.hotkeys.clear();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn merging_twice_applies_the_later_config_last() {
        let mut config = Config::default();
        config.merge_toml("initial-tool = \"arrow\"").unwrap();
        config.merge_toml("initial-tool = \"brush\"").unwrap();
        assert_eq!(config.initial_tool, Tools::Brush, "last write wins");
    }
}

#[cfg(test)]
mod readme_tests {
    use super::*;

    /// The config example in README.md must be a config the program accepts.
    ///
    /// Unknown keys are a hard error naming the key, so this catches a
    /// documented option that was renamed or never existed — and it catches the
    /// reverse drift too, where a feature ships and the example never learns
    /// about it. Documentation falling behind the code has been this project's
    /// most common defect by some margin.
    #[test]
    fn the_readme_config_example_is_one_the_loader_accepts() {
        let readme = include_str!("../../../README.md");
        let block = readme
            .split("```toml")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("README.md should contain a ```toml example");

        let mut config = Config::default();
        config.merge_toml(block).unwrap_or_else(|e| {
            panic!("the README's config example is not loadable: {e}\n{block}")
        });
    }

    /// Every `[capture]` key the program understands should appear in that
    /// example, so the README stays a complete reference rather than a
    /// selection someone forgot to extend.
    #[test]
    fn the_readme_documents_every_capture_option() {
        let readme = include_str!("../../../README.md");
        for key in ["mode", "delay-seconds", "snap-to-windows", "include-cursor"] {
            assert!(
                readme.contains(&format!("{key} =")),
                "README.md's example config never mentions the `{key}` capture option"
            );
        }
    }
}
