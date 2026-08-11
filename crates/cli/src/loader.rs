//! Layering defaults, the config file and the command line into one [`Config`].
//!
//! The order is fixed: [`Config::default()`], then the config file, then the
//! arguments. Each layer only touches what it mentions, so the result is
//! "everything the user asked for, defaults for the rest" — see the crate docs
//! for why the boolean flags are `Option<bool>`.

use std::path::{Path, PathBuf};

use bettershot_core::Config;
use directories::ProjectDirs;

use crate::args::Args;
use crate::error::CliError;

/// The config file bettershot reads when `--config` is not given.
///
/// * Linux/BSD: `$XDG_CONFIG_HOME/bettershot/config.toml`, else
///   `~/.config/bettershot/config.toml`
/// * Windows: `%APPDATA%\bettershot\config.toml`
/// * macOS: `~/Library/Application Support/bettershot/config.toml`
///
/// `None` only when the platform has no home directory to speak of, in which
/// case bettershot runs on defaults plus flags.
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = xdg_config_path() {
        return Some(path);
    }
    let dirs = ProjectDirs::from("", "", "bettershot")?;
    Some(config_dir(&dirs).join("config.toml"))
}

/// `directories` already honours `XDG_CONFIG_HOME`, but reading it directly
/// keeps the Linux path predictable and explicit, and matches what people
/// expect from every other tool that documents an XDG path.
#[cfg(unix)]
fn xdg_config_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME")?;
    if xdg.is_empty() {
        return None;
    }
    Some(PathBuf::from(xdg).join("bettershot").join("config.toml"))
}

#[cfg(not(unix))]
fn xdg_config_path() -> Option<PathBuf> {
    None
}

/// `directories` puts Windows configuration in
/// `%APPDATA%\bettershot\config\`, one level deeper than the documented
/// `%APPDATA%\bettershot\config.toml`. Step back up when that extra component
/// is present so the path matches what the docs, the installer and users
/// coming from Satty expect.
fn config_dir(dirs: &ProjectDirs) -> PathBuf {
    let dir = dirs.config_dir();
    if dir.file_name().is_some_and(|name| name == "config") {
        if let Some(parent) = dir.parent() {
            return parent.to_path_buf();
        }
    }
    dir.to_path_buf()
}

/// Load the configuration for this invocation: defaults, then the config file,
/// then the arguments, then a validity check.
pub fn load_config(args: &Args) -> Result<Config, CliError> {
    load_config_with(args, config_path().as_deref())
}

/// [`load_config`] with the discovered config path supplied by the caller,
/// which is what makes the discovery rules testable without a home directory.
///
/// A `discovered` file that does not exist is not an error — most people never
/// write one. A file named by `--config` that does not exist is an error, and
/// so is one that fails to parse, reported with its path.
pub fn load_config_with(args: &Args, discovered: Option<&Path>) -> Result<Config, CliError> {
    let mut config = Config::default();

    if args.no_config {
        log::debug!("--no-config: skipping the config file");
    } else if let Some(path) = args.config.as_deref() {
        let source = std::fs::read_to_string(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                CliError::MissingConfig {
                    path: path.to_path_buf(),
                }
            } else {
                CliError::ReadConfig {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        merge(&mut config, &source, path)?;
    } else if let Some(path) = discovered {
        match std::fs::read_to_string(path) {
            Ok(source) => merge(&mut config, &source, path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("no config file at {}, using defaults", path.display());
            }
            Err(source) => {
                return Err(CliError::ReadConfig {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }

    apply_args(&mut config, args);
    config.validate()?;
    Ok(config)
}

fn merge(config: &mut Config, source: &str, path: &Path) -> Result<(), CliError> {
    log::debug!("reading config from {}", path.display());
    config
        .merge_toml(source)
        .map_err(|source| CliError::BadConfig {
            path: path.to_path_buf(),
            source,
        })
}

/// Fold the command line over `config`.
///
/// Pure, and deliberately separate from the file handling: precedence is the
/// part worth testing, and it should not need a filesystem to test.
///
/// Every field is applied only when the user actually gave it. For the
/// booleans that means checking `Option<bool>` rather than "is it true?" — a
/// flag left off must not reset a `true` that came from the config file.
pub fn apply_args(config: &mut Config, args: &Args) {
    macro_rules! apply {
        ($($field:ident),* $(,)?) => {
            $(if let Some(value) = args.$field {
                config.$field = value;
            })*
        };
    }

    apply!(
        initial_tool,
        initial_size,
        annotation_size_factor,
        default_fill_shapes,
        default_round_caps,
        obscure,
        fullscreen,
        hide_toolbars,
        no_window_decoration,
        save_format,
        save_after_copy,
        disable_notifications,
        action_on_enter,
        action_on_escape,
        early_exit,
        theme,
        crash_reports,
        history_size,
    );

    // `Option`-valued in `Config` too, so "not given" and "given" have to be
    // told apart by hand rather than by the macro.
    if let Some(color) = args.initial_color {
        config.initial_color = Some(color);
    }
    if let Some(name) = &args.output_filename {
        config.output_filename = Some(name.clone());
    }
    if let Some(command) = &args.copy_command {
        config.copy_command = Some(command.clone());
    }

    // Replacing the palette leaves the user's own mixed colours alone: they
    // are a separate list, and dropping them would be a surprise.
    if let Some(palette) = &args.color_palette {
        config.color_palette.palette = palette.clone();
    }

    if let Some(family) = &args.font_family {
        config.font.family = Some(family.clone());
    }
    if let Some(path) = &args.font_path {
        config.font.path = Some(path.clone());
    }

    // Capture settings live in a nested table.
    if let Some(mode) = args.capture {
        config.capture.mode = mode;
    }
    if let Some(delay) = args.delay {
        config.capture.delay_seconds = delay;
    }
    if let Some(include) = args.include_cursor {
        config.capture.include_cursor = include;
    }
    if let Some(snap) = args.snap_to_windows {
        config.capture.snap_to_windows = snap;
    }

    // Daemon settings likewise.
    if let Some(enabled) = args.daemon {
        config.daemon.enabled = enabled;
    }
    if let Some(tray) = args.tray {
        config.daemon.tray = tray;
    }
    if let Some(language) = &args.language {
        config.language = language.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::config::{Action, CaptureMode, SaveFormat};
    use bettershot_core::style::{Color, Size};
    use bettershot_core::tools::{ObscureKind, Tools};
    use clap::Parser as _;
    use std::io::Write as _;

    fn args(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("should parse")
    }

    /// A config file on disk, plus the arguments to point at it.
    fn config_file(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&path).expect("should create");
        file.write_all(contents.as_bytes()).expect("should write");
        path
    }

    #[test]
    fn the_discovered_path_ends_in_the_expected_place() {
        // The exact prefix depends on the machine; the tail is ours to
        // guarantee, and must never grow the extra `config/` component
        // `directories` uses on Windows.
        if let Some(path) = config_path() {
            assert!(path.ends_with("bettershot/config.toml"), "got {path:?}");
        }
    }

    #[test]
    fn applying_no_arguments_changes_nothing() {
        let mut config = Config::default();
        apply_args(&mut config, &args(&["bettershot"]));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn arguments_override_the_matching_config_values() {
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                initial-tool = "arrow"
                initial-size = "small"
                annotation-size-factor = 2.0
                action-on-enter = "exit"
                save-format = "png"
                output-filename = "/from/file.png"
                copy-command = "xclip"
                obscure = "blur"

                [capture]
                mode = "window"
                delay-seconds = 1
                "##,
            )
            .expect("should parse");

        apply_args(
            &mut config,
            &args(&[
                "bettershot",
                "--initial-tool",
                "brush",
                "--initial-size",
                "large",
                "--annotation-size-factor",
                "3",
                "--action-on-enter",
                "save-to-file",
                "--save-format",
                "webp",
                "--output-filename",
                "/from/cli.png",
                "--copy-command",
                "wl-copy",
                "--obscure",
                "pixelate",
                "--capture",
                "monitor",
                "--delay",
                "7",
            ]),
        );

        assert_eq!(config.initial_tool, Tools::Brush);
        assert_eq!(config.initial_size, Size::Large);
        assert_eq!(config.annotation_size_factor, 3.0);
        assert_eq!(config.action_on_enter, Action::SaveToFile);
        assert_eq!(config.save_format, SaveFormat::Webp);
        assert_eq!(config.output_filename.as_deref(), Some("/from/cli.png"));
        assert_eq!(config.copy_command.as_deref(), Some("wl-copy"));
        assert_eq!(config.obscure, ObscureKind::Pixelate);
        assert_eq!(config.capture.mode, CaptureMode::Monitor);
        assert_eq!(config.capture.delay_seconds, 7);
    }

    #[test]
    fn arguments_that_were_not_given_leave_the_config_alone() {
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                initial-tool = "arrow"
                output-filename = "/from/file.png"
                copy-command = "xclip"
                action-on-escape = "none"

                [capture]
                mode = "window"
                delay-seconds = 4
                "##,
            )
            .expect("should parse");

        // One unrelated flag, to prove the others are untouched rather than
        // the whole call being a no-op.
        apply_args(
            &mut config,
            &args(&["bettershot", "--initial-size", "small"]),
        );

        assert_eq!(config.initial_size, Size::Small);
        assert_eq!(config.initial_tool, Tools::Arrow);
        assert_eq!(config.output_filename.as_deref(), Some("/from/file.png"));
        assert_eq!(config.copy_command.as_deref(), Some("xclip"));
        assert_eq!(config.action_on_escape, Action::None);
        assert_eq!(config.capture.mode, CaptureMode::Window);
        assert_eq!(config.capture.delay_seconds, 4);
    }

    #[test]
    fn an_absent_boolean_flag_never_clobbers_a_true_from_the_config() {
        // The classic clap bug: `fullscreen: bool` would be `false` here and
        // silently undo every one of these.
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                fullscreen = true
                hide-toolbars = true
                no-window-decoration = true
                default-fill-shapes = true
                save-after-copy = true
                disable-notifications = true
                early-exit = true

                [capture]
                include-cursor = true
                "##,
            )
            .expect("should parse");

        apply_args(&mut config, &args(&["bettershot"]));

        assert!(config.fullscreen);
        assert!(config.hide_toolbars);
        assert!(config.no_window_decoration);
        assert!(config.default_fill_shapes);
        assert!(config.save_after_copy);
        assert!(config.disable_notifications);
        assert!(config.early_exit);
        assert!(config.capture.include_cursor);
    }

    #[test]
    fn a_boolean_flag_can_turn_a_config_value_back_off() {
        let mut config = Config::default();
        config
            .merge_toml("fullscreen = true\ndefault-round-caps = true\n")
            .expect("should parse");

        apply_args(
            &mut config,
            &args(&[
                "bettershot",
                "--fullscreen=false",
                "--default-round-caps",
                "false",
            ]),
        );

        assert!(!config.fullscreen);
        assert!(!config.default_round_caps);
    }

    #[test]
    fn a_boolean_flag_turns_a_config_value_on() {
        let mut config = Config::default();
        config
            .merge_toml("fullscreen = false\n")
            .expect("should parse");
        apply_args(&mut config, &args(&["bettershot", "--fullscreen"]));
        assert!(config.fullscreen);
    }

    #[test]
    fn the_palette_from_the_command_line_keeps_custom_colours() {
        let mut config = Config::default();
        config
            .merge_toml(
                r##"
                [color-palette]
                palette = ["#ff0000"]
                custom = ["#123456"]
                "##,
            )
            .expect("should parse");

        apply_args(
            &mut config,
            &args(&["bettershot", "--color-palette", "#00ff00,#0000ff"]),
        );

        assert_eq!(
            config.color_palette.palette,
            vec![Color::rgb(0, 255, 0), Color::rgb(0, 0, 255)]
        );
        assert_eq!(
            config.color_palette.custom,
            vec![Color::rgb(0x12, 0x34, 0x56)]
        );
    }

    #[test]
    fn the_font_is_assembled_from_whichever_flags_were_given() {
        let mut config = Config::default();
        config
            .merge_toml("[font]\nfamily = \"Serif\"\n")
            .expect("should parse");
        apply_args(&mut config, &args(&["bettershot", "--font-path", "/f.ttf"]));
        assert_eq!(config.font.family.as_deref(), Some("Serif"));
        assert_eq!(config.font.path.as_deref(), Some("/f.ttf"));
    }

    #[test]
    fn the_full_stack_layers_defaults_then_file_then_arguments() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(
            &dir,
            r##"
            initial-tool = "arrow"
            initial-color = "#00ff00"
            fullscreen = true
            hide-toolbars = true
            output-filename = "/from/file.png"
            action-on-enter = "save-to-file"

            [capture]
            mode = "window"
            delay-seconds = 2
            "##,
        );

        let config = load_config(&args(&[
            "bettershot",
            "--config",
            path.to_str().expect("utf-8 tempdir"),
            "--initial-tool",
            "brush",
            "--hide-toolbars=false",
            "--delay",
            "5",
        ]))
        .expect("should load");

        // From the arguments.
        assert_eq!(config.initial_tool, Tools::Brush);
        assert!(!config.hide_toolbars);
        assert_eq!(config.capture.delay_seconds, 5);
        // From the file.
        assert_eq!(config.initial_color, Some(Color::rgb(0, 255, 0)));
        assert!(config.fullscreen);
        assert_eq!(config.output_filename.as_deref(), Some("/from/file.png"));
        assert_eq!(config.action_on_enter, Action::SaveToFile);
        assert_eq!(config.capture.mode, CaptureMode::Window);
        // From the defaults.
        assert_eq!(config.action_on_escape, Action::Exit);
        assert_eq!(config.annotation_size_factor, 1.0);
        assert_eq!(config.color_palette, Config::default().color_palette);
    }

    #[test]
    fn a_missing_discovered_config_is_not_an_error() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let missing = dir.path().join("never-written.toml");
        let config =
            load_config_with(&args(&["bettershot"]), Some(&missing)).expect("should be fine");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_missing_explicit_config_is_an_error_naming_the_path() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let missing = dir.path().join("never-written.toml");
        let error = load_config(&args(&[
            "bettershot",
            "--config",
            missing.to_str().expect("utf-8 tempdir"),
        ]))
        .expect_err("should fail");

        assert!(matches!(error, CliError::MissingConfig { .. }));
        assert!(error.to_string().contains("never-written.toml"));
    }

    #[test]
    fn a_malformed_config_reports_the_path_and_the_parse_error() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(&dir, "initial-tool = \n");
        let error = load_config(&args(&[
            "bettershot",
            "--config",
            path.to_str().expect("utf-8 tempdir"),
        ]))
        .expect_err("should fail");

        assert!(matches!(error, CliError::BadConfig { .. }));
        let message = error.to_string();
        assert!(message.contains("config.toml"), "got: {message}");
        assert!(
            !format!("{:?}", std::error::Error::source(&error)).is_empty(),
            "the parse detail must survive"
        );
    }

    #[test]
    fn an_unknown_key_in_the_config_is_rejected() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(&dir, "fulscreen = true\n");
        assert!(matches!(
            load_config(&args(&[
                "bettershot",
                "--config",
                path.to_str().expect("utf-8 tempdir")
            ])),
            Err(CliError::BadConfig { .. })
        ));
    }

    #[test]
    fn no_config_skips_the_file_layer_entirely() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(&dir, "fullscreen = true\ninitial-tool = \"arrow\"\n");

        // Discovery would have found it; --no-config means it is not read.
        let config = load_config_with(&args(&["bettershot", "--no-config"]), Some(&path))
            .expect("should load");
        assert_eq!(config, Config::default());

        // And it still layers the arguments on the defaults.
        let config = load_config_with(
            &args(&["bettershot", "--no-config", "--initial-tool", "brush"]),
            Some(&path),
        )
        .expect("should load");
        assert_eq!(config.initial_tool, Tools::Brush);
        assert!(!config.fullscreen, "the file must not have been read");
    }

    #[test]
    fn a_discovered_config_is_read_when_no_flag_points_elsewhere() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(&dir, "initial-tool = \"arrow\"\n");
        let config = load_config_with(&args(&["bettershot"]), Some(&path)).expect("should load");
        assert_eq!(config.initial_tool, Tools::Arrow);
    }

    #[test]
    fn an_explicit_config_wins_over_the_discovered_one() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let discovered = config_file(&dir, "initial-tool = \"arrow\"\n");
        let explicit = dir.path().join("other.toml");
        std::fs::write(&explicit, "initial-tool = \"line\"\n").expect("should write");

        let config = load_config_with(
            &args(&[
                "bettershot",
                "--config",
                explicit.to_str().expect("utf-8 tempdir"),
            ]),
            Some(&discovered),
        )
        .expect("should load");
        assert_eq!(config.initial_tool, Tools::Line);
    }

    #[test]
    fn the_resolved_config_is_validated_after_the_arguments_are_applied() {
        // Valid on its own, made invalid by a flag: the check has to come last.
        let error = load_config(&args(&[
            "bettershot",
            "--no-config",
            "--annotation-size-factor",
            "0",
        ]))
        .expect_err("should fail");
        assert!(matches!(error, CliError::InvalidConfig(_)));
        assert!(error.to_string().contains("annotation-size-factor"));
    }

    #[test]
    fn the_config_file_is_validated_too() {
        let dir = tempfile::tempdir().expect("should create tempdir");
        let path = config_file(&dir, "color-palette = []\n");
        assert!(matches!(
            load_config(&args(&[
                "bettershot",
                "--config",
                path.to_str().expect("utf-8 tempdir")
            ])),
            Err(CliError::InvalidConfig(_))
        ));
    }
}

#[cfg(test)]
mod daemon_tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn the_daemon_flag_turns_residency_on() {
        let mut config = Config::default();
        assert!(!config.daemon.enabled);
        let args = Args::try_parse_from(["bettershot", "--daemon"]).unwrap();
        apply_args(&mut config, &args);
        assert!(config.daemon.enabled);
    }

    #[test]
    fn the_daemon_flag_can_also_turn_a_configured_daemon_off() {
        let mut config = Config::default();
        config.merge_toml("[daemon]\nenabled = true").unwrap();
        assert!(config.daemon.enabled);

        let args = Args::try_parse_from(["bettershot", "--daemon", "false"]).unwrap();
        apply_args(&mut config, &args);
        assert!(!config.daemon.enabled, "an explicit false must win");
    }

    #[test]
    fn an_absent_daemon_flag_leaves_the_config_alone() {
        let mut config = Config::default();
        config
            .merge_toml("[daemon]\nenabled = true\ntray = false")
            .unwrap();
        let args = Args::try_parse_from(["bettershot"]).unwrap();
        apply_args(&mut config, &args);
        assert!(config.daemon.enabled);
        assert!(!config.daemon.tray);
    }

    #[test]
    fn the_tray_can_be_disabled_from_the_command_line() {
        let mut config = Config::default();
        assert!(config.daemon.tray, "on by default");
        let args = Args::try_parse_from(["bettershot", "--tray", "false"]).unwrap();
        apply_args(&mut config, &args);
        assert!(!config.daemon.tray);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    use bettershot_core::config::Theme;
    use clap::Parser as _;

    #[test]
    fn the_theme_flag_overrides_the_config_file() {
        let mut config = Config::default();
        config.merge_toml("theme = \"light\"").unwrap();
        assert_eq!(config.theme, Theme::Light);

        let args = Args::try_parse_from(["bettershot", "--theme", "dark"]).unwrap();
        apply_args(&mut config, &args);
        assert_eq!(config.theme, Theme::Dark);
    }

    #[test]
    fn an_absent_theme_flag_leaves_the_config_value_alone() {
        let mut config = Config::default();
        config.merge_toml("theme = \"light\"").unwrap();
        let args = Args::try_parse_from(["bettershot"]).unwrap();
        apply_args(&mut config, &args);
        assert_eq!(config.theme, Theme::Light);
    }

    #[test]
    fn a_bogus_theme_is_rejected_with_a_helpful_message() {
        let err = Args::try_parse_from(["bettershot", "--theme", "neon"]).unwrap_err();
        assert!(err.to_string().contains("neon"), "{err}");
    }
}
