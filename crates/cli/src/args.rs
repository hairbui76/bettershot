//! The clap definition of `bettershot`'s command line.
//!
//! Long flag names deliberately match [Satty](https://github.com/Satty-org/Satty)
//! wherever an equivalent option exists, so a Satty invocation keeps working
//! after swapping the binary. Options Satty cannot have — anything about
//! capturing, which it delegates to `grim`/`spectacle` — are new.
//!
//! Every option is `Option`-shaped, including the booleans, because "absent"
//! has to stay distinguishable from "explicitly false" for the precedence rule
//! in [`crate::loader::apply_args`] to hold.

use std::path::PathBuf;

use bettershot_core::config::{Action, CaptureMode, SaveFormat};
use bettershot_core::style::{Color, Size};
use bettershot_core::tools::{ObscureKind, Tools};
use clap::{ArgAction, CommandFactory, Parser};
use clap_complete::Shell;

/// The name the binary is installed as. Used for completions and the manpage
/// title, and as the clap command name so `--help` reads correctly however the
/// library happens to be linked.
pub const BIN_NAME: &str = "bettershot";

/// Capture, annotate and share a screenshot.
#[derive(Parser, Debug, Clone, Default, PartialEq)]
#[command(name = BIN_NAME, version, about, long_about = None)]
pub struct Args {
    // --- input ------------------------------------------------------------
    /// Image to annotate, or `-` to read one from stdin.
    ///
    /// Omit it to capture the screen instead; that is the default.
    #[arg(short, long, value_name = "PATH", conflicts_with = "capture")]
    pub filename: Option<PathBuf>,

    /// Capture the screen instead of opening an existing image.
    ///
    /// [possible values: region, window, monitor, all]
    #[arg(short, long, value_name = "MODE")]
    pub capture: Option<CaptureMode>,

    /// Seconds to wait before capturing, for grabbing menus and hover states.
    #[arg(long, value_name = "SECONDS")]
    pub delay: Option<u64>,

    /// Include the mouse cursor in the capture. Supported on X11 (needs the
    /// XFixes extension) and Windows; the Wayland screenshot portal offers no
    /// cursor control at all, and macOS does not read it yet.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub include_cursor: Option<bool>,

    /// Highlight and snap to window edges while dragging out a region.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub snap_to_windows: Option<bool>,

    // --- output -----------------------------------------------------------
    /// Where a save action writes.
    ///
    /// May contain strftime placeholders and a leading `~`, e.g.
    /// `~/shots/%Y%m%d-%H%M%S.png`. See
    /// <https://docs.rs/chrono/latest/chrono/format/strftime/index.html>.
    /// Omit to disable saving to a file.
    #[arg(short, long, value_name = "PATH")]
    pub output_filename: Option<String>,

    /// Image format for saved files.
    ///
    /// [possible values: png, jpeg, webp]
    #[arg(long, value_name = "FORMAT", value_parser = parse_save_format)]
    pub save_format: Option<SaveFormat>,

    /// Pipe the image to this command instead of using the built-in clipboard,
    /// for example `wl-copy`.
    #[arg(long, value_name = "CMD")]
    pub copy_command: Option<String>,

    /// Also write the image to the output file after copying it.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub save_after_copy: Option<bool>,

    /// Do not show a desktop notification after saving or copying.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub disable_notifications: Option<bool>,

    // --- behaviour --------------------------------------------------------
    /// Quit as soon as a save action succeeds.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub early_exit: Option<bool>,

    /// UI language code, or "system" to follow the desktop.
    #[arg(long, value_name = "LANG")]
    pub language: Option<String>,

    /// How many recent captures to keep for re-copying (0 disables it).
    #[arg(long, value_name = "N")]
    pub history_size: Option<usize>,

    /// Write a local crash report if bettershot panics.
    ///
    /// Reports contain no image data and are never transmitted.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub crash_reports: Option<bool>,

    /// Stay running after the capture, so a hotkey or the tray can start the
    /// next one.
    ///
    /// Global hotkeys cannot be registered on Wayland; bind your compositor to
    /// `bettershot --capture region` there instead.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub daemon: Option<bool>,

    /// Show a system tray icon while resident.
    ///
    /// Requires a build with the `tray` feature, which is opt-in on Linux.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub tray: Option<bool>,

    /// Colour scheme for the editor.
    ///
    /// [possible values: system, light, dark]
    #[arg(long, value_name = "THEME")]
    pub theme: Option<bettershot_core::config::Theme>,

    /// What Enter does.
    ///
    /// [possible values: save-to-clipboard, save-to-file, save-as, exit, none]
    #[arg(long, value_name = "ACTION")]
    pub action_on_enter: Option<Action>,

    /// What Escape does.
    ///
    /// [possible values: save-to-clipboard, save-to-file, save-as, exit, none]
    #[arg(long, value_name = "ACTION")]
    pub action_on_escape: Option<Action>,

    // --- window -----------------------------------------------------------
    /// Open the editor fullscreen.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub fullscreen: Option<bool>,

    /// Drop the title bar and borders. The compositor has the final say.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub no_window_decoration: Option<bool>,

    /// Keep the window above other windows, like the Windows Snipping Tool.
    ///
    /// On by default. Pass `--always-on-top false` for an ordinary window.
    /// Wayland compositors may ignore it.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub always_on_top: Option<bool>,

    /// Start with the toolbars hidden.
    #[arg(
        long,
        visible_alias = "default-hide-toolbars",
        num_args = 0..=1,
        default_missing_value = "true",
        value_name = "BOOL",
    )]
    pub hide_toolbars: Option<bool>,

    // --- editor defaults --------------------------------------------------
    /// Tool selected on startup.
    ///
    /// [possible values: pointer, crop, line, arrow, rectangle, ellipse, text,
    /// marker, brush, highlight, blur]
    #[arg(long, visible_alias = "init-tool", value_name = "TOOL")]
    pub initial_tool: Option<Tools>,

    /// Colour selected on startup, e.g. `#ff0000`. Defaults to the first
    /// palette entry.
    #[arg(long, value_name = "COLOR")]
    pub initial_color: Option<Color>,

    /// Annotation size selected on startup.
    ///
    /// [possible values: small, medium, large]
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub initial_size: Option<Size>,

    /// Scale every annotation dimension by this factor.
    #[arg(long, value_name = "FACTOR")]
    pub annotation_size_factor: Option<f32>,

    /// Draw shapes filled rather than outlined.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub default_fill_shapes: Option<bool>,

    /// Draw lines with round caps and joins.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    pub default_round_caps: Option<bool>,

    /// How the blur tool obscures what it covers.
    ///
    /// [possible values: blur, pixelate]
    #[arg(long, value_name = "KIND", value_parser = parse_obscure)]
    pub obscure: Option<ObscureKind>,

    /// Replace the toolbar colour palette, e.g. `#ff0000,#00ff00`.
    #[arg(long, value_name = "COLORS", value_delimiter = ',')]
    pub color_palette: Option<Vec<Color>>,

    /// Font family for text annotations.
    #[arg(long, value_name = "FAMILY")]
    pub font_family: Option<String>,

    /// Font file for text annotations. Wins over --font-family.
    #[arg(long, value_name = "PATH")]
    pub font_path: Option<String>,

    // --- config -----------------------------------------------------------
    /// Read this config file instead of the discovered one.
    #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Ignore the config file entirely and use defaults plus these flags.
    #[arg(long)]
    pub no_config: bool,

    // --- meta -------------------------------------------------------------
    /// Print the manpage and exit. Pipe it to `man -l -`.
    #[arg(long, exclusive = true)]
    pub man: bool,

    /// Print the licence and exit.
    #[arg(long, exclusive = true)]
    pub license: bool,

    /// Print a shell completion script and exit.
    #[arg(long, value_name = "SHELL", exclusive = true)]
    pub completions: Option<Shell>,

    /// Log more. Repeat for more still: -v info, -vv debug, -vvv trace.
    #[arg(short, long, action = ArgAction::Count)]
    pub verbose: u8,
}

impl Args {
    /// Log level implied by the number of `-v`s.
    pub fn log_level(&self) -> log::LevelFilter {
        match self.verbose {
            0 => log::LevelFilter::Warn,
            1 => log::LevelFilter::Info,
            2 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        }
    }

    /// Whether one of `--man`, `--license` or `--completions` was given, i.e.
    /// whether the process should print something and exit instead of opening
    /// an editor.
    pub fn is_meta_request(&self) -> bool {
        self.man || self.license || self.completions.is_some()
    }
}

/// The clap [`clap::Command`], for `build.rs` completion and manpage
/// generation and for anything else that wants the parser without parsing.
pub fn command() -> clap::Command {
    Args::command()
}

/// Render the manpage as roff.
pub fn render_manpage() -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    clap_mangen::Man::new(command())
        .title(BIN_NAME)
        .render(&mut buffer)?;
    Ok(buffer)
}

/// Write a completion script for `shell`.
pub fn render_completions(shell: Shell, out: &mut dyn std::io::Write) {
    clap_complete::generate(shell, &mut command(), BIN_NAME, out);
}

// `Size`, `SaveFormat` and `ObscureKind` are serde enums in core without a
// `FromStr`, so clap gets explicit parsers rather than core gaining an impl
// only the CLI would use. The error strings list the alternatives, which is
// what makes a typo recoverable.
fn parse_size(raw: &str) -> Result<Size, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "small" => Ok(Size::Small),
        "medium" => Ok(Size::Medium),
        "large" => Ok(Size::Large),
        other => Err(format!(
            "unknown size `{other}`, expected one of: small, medium, large"
        )),
    }
}

fn parse_save_format(raw: &str) -> Result<SaveFormat, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "png" => Ok(SaveFormat::Png),
        "jpg" | "jpeg" => Ok(SaveFormat::Jpeg),
        "webp" => Ok(SaveFormat::Webp),
        other => Err(format!(
            "unknown save format `{other}`, expected one of: png, jpeg, webp"
        )),
    }
}

fn parse_obscure(raw: &str) -> Result<ObscureKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "blur" => Ok(ObscureKind::Blur),
        "pixelate" => Ok(ObscureKind::Pixelate),
        other => Err(format!(
            "unknown obscure kind `{other}`, expected one of: blur, pixelate"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("should parse")
    }

    fn error(argv: &[&str]) -> String {
        Args::try_parse_from(argv)
            .expect_err("should have been rejected")
            .to_string()
    }

    #[test]
    fn the_command_definition_is_internally_consistent() {
        // Catches duplicate flags, bad aliases and broken defaults, which are
        // otherwise only found at runtime.
        command().debug_assert();
        let command = command();
        assert_eq!(command.get_name(), "bettershot");
        // `--version` and `--help` come from the Cargo manifest; an empty
        // `about` means the crate's `description` was dropped.
        assert_eq!(
            command.get_version(),
            Some(env!("CARGO_PKG_VERSION")),
            "--version should report the crate version"
        );
        assert!(
            command
                .get_about()
                .is_some_and(|about| !about.to_string().is_empty()),
            "--help should have an about line"
        );
    }

    #[test]
    fn no_arguments_means_no_opinions() {
        let args = parse(&["bettershot"]);
        assert_eq!(args, Args::default(), "an empty argv must not set anything");
        assert!(!args.is_meta_request());
        assert_eq!(args.log_level(), log::LevelFilter::Warn);
    }

    #[test]
    fn the_full_long_flag_surface_parses() {
        let args = parse(&[
            "bettershot",
            "--capture",
            "window",
            "--delay",
            "3",
            "--include-cursor",
            "--snap-to-windows",
            "false",
            "--output-filename",
            "~/shots/%Y.png",
            "--save-format",
            "webp",
            "--copy-command",
            "wl-copy",
            "--save-after-copy",
            "--disable-notifications",
            "--early-exit",
            "--action-on-enter",
            "save-to-file",
            "--action-on-escape",
            "none",
            "--fullscreen",
            "--no-window-decoration",
            "--hide-toolbars",
            "--initial-tool",
            "arrow",
            "--initial-color",
            "#00ff00",
            "--initial-size",
            "large",
            "--annotation-size-factor",
            "1.5",
            "--default-fill-shapes",
            "--default-round-caps",
            "false",
            "--obscure",
            "pixelate",
            "--color-palette",
            "#ff0000,#0000ff",
            "--font-family",
            "Inter",
            "--font-path",
            "/usr/share/fonts/x.ttf",
            "--no-config",
        ]);

        assert_eq!(args.capture, Some(CaptureMode::Window));
        assert_eq!(args.delay, Some(3));
        assert_eq!(args.include_cursor, Some(true));
        assert_eq!(args.snap_to_windows, Some(false));
        assert_eq!(args.output_filename.as_deref(), Some("~/shots/%Y.png"));
        assert_eq!(args.save_format, Some(SaveFormat::Webp));
        assert_eq!(args.copy_command.as_deref(), Some("wl-copy"));
        assert_eq!(args.save_after_copy, Some(true));
        assert_eq!(args.disable_notifications, Some(true));
        assert_eq!(args.early_exit, Some(true));
        assert_eq!(args.action_on_enter, Some(Action::SaveToFile));
        assert_eq!(args.action_on_escape, Some(Action::None));
        assert_eq!(args.fullscreen, Some(true));
        assert_eq!(args.no_window_decoration, Some(true));
        assert_eq!(args.hide_toolbars, Some(true));
        assert_eq!(args.initial_tool, Some(Tools::Arrow));
        assert_eq!(args.initial_color, Some(Color::rgb(0, 255, 0)));
        assert_eq!(args.initial_size, Some(Size::Large));
        assert_eq!(args.annotation_size_factor, Some(1.5));
        assert_eq!(args.default_fill_shapes, Some(true));
        assert_eq!(args.default_round_caps, Some(false));
        assert_eq!(args.obscure, Some(ObscureKind::Pixelate));
        assert_eq!(
            args.color_palette,
            Some(vec![Color::rgb(255, 0, 0), Color::rgb(0, 0, 255)])
        );
        assert_eq!(args.font_family.as_deref(), Some("Inter"));
        assert_eq!(args.font_path.as_deref(), Some("/usr/share/fonts/x.ttf"));
        assert!(args.no_config);
    }

    #[test]
    fn short_flags_match_satty() {
        let args = parse(&["bettershot", "-f", "in.png", "-o", "out.png", "-vv"]);
        assert_eq!(
            args.filename.as_deref(),
            Some(std::path::Path::new("in.png"))
        );
        assert_eq!(args.output_filename.as_deref(), Some("out.png"));
        assert_eq!(args.verbose, 2);
        assert_eq!(args.log_level(), log::LevelFilter::Debug);

        let args = parse(&["bettershot", "-c", "region"]);
        assert_eq!(args.capture, Some(CaptureMode::Region));
    }

    #[test]
    fn verbosity_counts_up_and_saturates_at_trace() {
        assert_eq!(parse(&["bettershot"]).log_level(), log::LevelFilter::Warn);
        assert_eq!(
            parse(&["bettershot", "-v"]).log_level(),
            log::LevelFilter::Info
        );
        assert_eq!(
            parse(&["bettershot", "-vvvvv"]).log_level(),
            log::LevelFilter::Trace
        );
    }

    #[test]
    fn a_boolean_flag_takes_an_optional_explicit_value() {
        // Three spellings of the same thing, plus the negation that a bare
        // `bool` field could not express.
        assert_eq!(
            parse(&["bettershot", "--fullscreen"]).fullscreen,
            Some(true)
        );
        assert_eq!(
            parse(&["bettershot", "--fullscreen=true"]).fullscreen,
            Some(true)
        );
        assert_eq!(
            parse(&["bettershot", "--fullscreen", "false"]).fullscreen,
            Some(false)
        );
        assert_eq!(
            parse(&["bettershot"]).fullscreen,
            None,
            "absent stays absent"
        );
    }

    #[test]
    fn a_valueless_boolean_flag_does_not_swallow_the_next_flag() {
        let args = parse(&["bettershot", "--fullscreen", "--initial-tool", "brush"]);
        assert_eq!(args.fullscreen, Some(true));
        assert_eq!(args.initial_tool, Some(Tools::Brush));
    }

    #[test]
    fn satty_compatible_aliases_still_work() {
        assert_eq!(
            parse(&["bettershot", "--init-tool", "text"]).initial_tool,
            Some(Tools::Text)
        );
        assert_eq!(
            parse(&["bettershot", "--default-hide-toolbars"]).hide_toolbars,
            Some(true)
        );
    }

    #[test]
    fn filename_and_capture_conflict() {
        let message = error(&["bettershot", "--filename", "a.png", "--capture", "region"]);
        assert!(
            message.contains("cannot be used with"),
            "expected a conflict error, got: {message}"
        );
    }

    #[test]
    fn config_and_no_config_conflict() {
        let message = error(&["bettershot", "--config", "c.toml", "--no-config"]);
        assert!(
            message.contains("cannot be used with"),
            "expected a conflict error, got: {message}"
        );
    }

    #[test]
    fn meta_flags_are_exclusive_and_recognisable() {
        assert!(parse(&["bettershot", "--man"]).is_meta_request());
        assert!(parse(&["bettershot", "--license"]).is_meta_request());
        let args = parse(&["bettershot", "--completions", "bash"]);
        assert_eq!(args.completions, Some(Shell::Bash));
        assert!(args.is_meta_request());
        assert!(Args::try_parse_from(["bettershot", "--man", "--fullscreen"]).is_err());
    }

    #[test]
    fn a_bad_tool_is_rejected_by_name() {
        let message = error(&["bettershot", "--initial-tool", "wobble"]);
        assert!(message.contains("wobble"), "got: {message}");
        assert!(message.contains("unknown tool"), "got: {message}");
    }

    #[test]
    fn a_bad_enum_value_lists_the_alternatives() {
        for (flag, value, needle) in [
            ("--initial-size", "huge", "small, medium, large"),
            ("--obscure", "smudge", "blur, pixelate"),
            ("--save-format", "gif", "png, jpeg, webp"),
            ("--capture", "everything", "region, window, monitor, all"),
            ("--action-on-enter", "explode", "save-to-clipboard"),
        ] {
            let message = error(&["bettershot", flag, value]);
            assert!(
                message.contains(needle) && message.contains(value),
                "`{flag} {value}` should explain itself, got: {message}"
            );
        }
    }

    #[test]
    fn a_bad_colour_and_a_bad_number_are_rejected() {
        assert!(error(&["bettershot", "--initial-color", "chartreuse"]).contains("invalid color"));
        assert!(Args::try_parse_from(["bettershot", "--annotation-size-factor", "big"]).is_err());
        assert!(Args::try_parse_from(["bettershot", "--delay", "-1"]).is_err());
        assert!(Args::try_parse_from(["bettershot", "--fullscreen", "maybe"]).is_err());
    }

    #[test]
    fn unknown_flags_are_rejected() {
        assert!(Args::try_parse_from(["bettershot", "--wobble"]).is_err());
    }

    #[test]
    fn the_manpage_and_completions_render() {
        let man = String::from_utf8(render_manpage().expect("manpage should render"))
            .expect("roff is utf-8");
        // roff escapes hyphens, so compare against the unescaped text.
        let man = man.replace('\\', "");
        assert!(man.contains("bettershot"));
        assert!(
            man.contains("output-filename"),
            "flags should be documented"
        );
        assert!(
            man.contains("capture"),
            "capture modes should be documented"
        );

        let mut script = Vec::new();
        render_completions(Shell::Bash, &mut script);
        let script = String::from_utf8(script).expect("script is utf-8");
        assert!(script.contains("bettershot"));
        assert!(script.contains("--capture"));
    }
}
